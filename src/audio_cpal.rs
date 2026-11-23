//! cpal capture (macOS: CoreAudio, Linux: ALSA, which PulseAudio and PipeWire also
//! serve): the chosen (or default) input device in its own format, mixed down to mono and
//! resampled to 16 kHz here. The stream is created and dropped on the capture thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

use super::{CaptureResult, LEVEL, SAMPLE_RATE, live_push, live_reset, loudness};

fn device_name(d: &cpal::Device) -> Option<String> {
    d.description().ok().map(|desc| desc.name().to_string())
}

/// The input devices worth offering, with their names. On Linux, ALSA lists every
/// card under many aliases (hw, plughw, front, dsnoop...) next to virtual devices;
/// one entry per card (`sysdefault:CARD=...`) is kept. The system default goes through
/// PipeWire or PulseAudio, which follow the input chosen in the system settings.
fn listed(host: &cpal::Host) -> Vec<(String, cpal::Device)> {
    let Ok(it) = host.input_devices() else { return Vec::new() };
    it.filter(|d| {
        cfg!(target_os = "macos")
            || d.description().ok().and_then(|x| x.driver().map(|p| p.starts_with("sysdefault:CARD="))).unwrap_or(false)
    })
    .filter_map(|d| device_name(&d).map(|n| (n, d)))
    .collect()
}

pub(super) fn devices() -> Vec<String> {
    listed(&cpal::default_host()).into_iter().map(|(n, _)| n).collect()
}

/// The device with this name; the system default when the name is empty or the device is gone.
fn open_device(host: &cpal::Host, name: &str) -> Option<cpal::Device> {
    if !name.is_empty() {
        if let Some((_, d)) = listed(host).into_iter().find(|(n, _)| n == name) {
            return Some(d);
        }
        crate::log::write(&format!("microphone \"{name}\" not found, using the default one"));
    }
    host.default_input_device()
}

pub(super) fn capture(
    stop: Arc<AtomicBool>,
    ready: mpsc::Sender<Result<(), String>>,
    max_samples: usize,
    device: &str,
) -> CaptureResult {
    let fail = |msg: String| {
        let _ = ready.send(Err(msg.clone()));
        Err(msg)
    };
    let host = cpal::default_host();
    let Some(dev) = open_device(&host, device) else {
        return fail("no microphone found".into());
    };
    let cfg = match dev.default_input_config() {
        Ok(c) => c,
        Err(e) => return fail(format!("microphone unavailable: {e}")),
    };
    let channels = cfg.channels().max(1) as usize;
    let rate = cfg.sample_rate();
    let (tx, rx) = mpsc::channel::<Vec<f32>>();
    let stream = match cfg.sample_format() {
        SampleFormat::F32 => build::<f32>(&dev, cfg.into(), channels, tx),
        SampleFormat::I16 => build::<i16>(&dev, cfg.into(), channels, tx),
        SampleFormat::I32 => build::<i32>(&dev, cfg.into(), channels, tx),
        SampleFormat::U16 => build::<u16>(&dev, cfg.into(), channels, tx),
        SampleFormat::U8 => build::<u8>(&dev, cfg.into(), channels, tx),
        SampleFormat::F64 => build::<f64>(&dev, cfg.into(), channels, tx),
        f => return fail(format!("microphone: unsupported sample format {f}")),
    };
    let stream = match stream {
        Ok(s) => s,
        Err(e) => return fail(format!("microphone unavailable: {e}")),
    };
    live_reset();
    if let Err(e) = stream.play() {
        return fail(format!("microphone: {e}"));
    }
    let _ = ready.send(Ok(()));

    let mut rs = Resampler::new(rate, SAMPLE_RATE);
    let mut out: Vec<f32> = Vec::with_capacity(SAMPLE_RATE as usize * 20);
    let mut chunk = Vec::new();
    let mut take = |mono: &[f32], out: &mut Vec<f32>| {
        chunk.clear();
        rs.feed(mono, &mut chunk);
        let room = max_samples.saturating_sub(out.len());
        let start = out.len();
        out.extend_from_slice(&chunk[..chunk.len().min(room)]);
        live_push(&out[start..]);
        LEVEL.store(loudness(mono).to_bits(), Ordering::Relaxed);
    };
    while !stop.load(Ordering::Acquire) {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(mono) => take(&mono, &mut out),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    drop(stream);
    while let Ok(mono) = rx.try_recv() {
        take(&mono, &mut out);
    }
    LEVEL.store(0, Ordering::Relaxed);
    live_reset();
    Ok(out)
}

fn build<T>(dev: &cpal::Device, cfg: cpal::StreamConfig, channels: usize, tx: mpsc::Sender<Vec<f32>>) -> Result<cpal::Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    dev.build_input_stream(
        cfg,
        move |data: &[T], _: &_| {
            let mono: Vec<f32> = data
                .chunks(channels)
                .map(|frame| frame.iter().map(|&s| s.to_sample::<f32>()).sum::<f32>() / frame.len() as f32)
                .collect();
            let _ = tx.send(mono);
        },
        |e| crate::log::write(&format!("microphone stream: {e}")),
        None,
    )
    .map_err(|e| e.to_string())
}

/// Low-pass FIR (below the new Nyquist) followed by linear interpolation.
/// Good enough for speech going into Whisper, which itself works at 16 kHz.
pub struct Resampler {
    /// Input samples per output sample.
    step: f64,
    pos: f64,
    taps: Vec<f32>,
    delay: Vec<f32>,
    filtered: Vec<f32>,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Self {
        let step = from as f64 / to as f64;
        let taps = if step > 1.0 { lowpass(0.45 / step, 63) } else { vec![1.0] };
        Self { step, pos: 0.0, delay: vec![0.0; taps.len()], taps, filtered: Vec::new() }
    }

    pub fn feed(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let n = self.taps.len();
        for &x in input {
            self.delay.rotate_right(1);
            self.delay[0] = x;
            let y = if n == 1 { x } else { self.delay.iter().zip(&self.taps).map(|(a, b)| a * b).sum() };
            self.filtered.push(y);
        }
        let f = &self.filtered;
        while self.pos + 1.0 < f.len() as f64 {
            let i = self.pos as usize;
            let frac = (self.pos - i as f64) as f32;
            out.push(f[i] * (1.0 - frac) + f[i + 1] * frac);
            self.pos += self.step;
        }
        let used = (self.pos as usize).min(self.filtered.len());
        self.filtered.drain(..used);
        self.pos -= used as f64;
    }
}

/// Hamming-windowed sinc, cutoff as a fraction of the input sample rate, unity DC gain.
fn lowpass(cutoff: f64, n: usize) -> Vec<f32> {
    let m = (n - 1) as f64 / 2.0;
    let mut t: Vec<f64> = (0..n)
        .map(|i| {
            let x = i as f64 - m;
            let sinc = if x == 0.0 { 2.0 * cutoff } else { (2.0 * std::f64::consts::PI * cutoff * x).sin() / (std::f64::consts::PI * x) };
            sinc * (0.54 - 0.46 * (2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64).cos())
        })
        .collect();
    let sum: f64 = t.iter().sum();
    t.iter_mut().for_each(|v| *v /= sum);
    t.into_iter().map(|v| v as f32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, hz: f32, secs: f32) -> Vec<f32> {
        (0..(rate as f32 * secs) as usize).map(|i| (i as f32 / rate as f32 * hz * std::f32::consts::TAU).sin()).collect()
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt()
    }

    #[test]
    fn resamples_48k_and_44k_to_16k() {
        for rate in [48_000, 44_100, 16_000] {
            let mut rs = Resampler::new(rate, 16_000);
            let mut out = Vec::new();
            let input = tone(rate, 440.0, 1.0);
            for part in input.chunks(480) {
                rs.feed(part, &mut out); // in callback-sized pieces
            }
            assert!((out.len() as i64 - 16_000).abs() < 40, "{rate}: {} samples", out.len());
            let r = rms(&out[200..]);
            assert!((r - 0.707).abs() < 0.03, "{rate}: 440 Hz passes, rms {r}");
        }
        // 12 kHz is above the new Nyquist (8 kHz) and must be filtered out, not aliased
        let mut rs = Resampler::new(48_000, 16_000);
        let mut out = Vec::new();
        rs.feed(&tone(48_000, 12_000.0, 0.5), &mut out);
        assert!(rms(&out[200..]) < 0.05, "aliasing: rms {}", rms(&out[200..]));
    }
}
