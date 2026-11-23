//! Microphone capture: 16 kHz mono samples in [-1, 1].
//! Windows: the classic waveIn API; its audio mapper does the resampling.
//! macOS and Linux: cpal (CoreAudio, ALSA) at the device's own rate, resampled here.
//! The device is opened only while the hotkey is held, so the mic indicator
//! is on exactly when we are listening.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, mpsc};
use std::thread::{self, JoinHandle};

#[cfg(windows)]
#[path = "audio_wavein.rs"]
mod backend;
#[cfg(not(windows))]
#[path = "audio_cpal.rs"]
mod backend;
use backend::capture;

/// Names of the microphones the system has now, in its own order. The names are
/// what `microphone =` in dictate.ini takes. Empty if listing fails.
pub fn input_devices() -> Vec<String> {
    let mut v = backend::devices();
    let mut seen = std::collections::HashSet::new();
    v.retain(|n| !n.trim().is_empty() && seen.insert(n.clone()));
    v
}

pub const SAMPLE_RATE: u32 = 16_000;

/// Loudness of the latest 50 ms buffer, 0..1 (f32 bits), for the on-screen meter.
pub static LEVEL: AtomicU32 = AtomicU32::new(0);

pub fn level() -> f32 {
    f32::from_bits(LEVEL.load(Ordering::Relaxed))
}

/// RMS mapped from -55..-10 dBFS onto 0..1.
pub(crate) fn loudness(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&s| (s as f64).powi(2)).sum();
    let rms = (sum / samples.len() as f64).sqrt().max(1e-9);
    ((20.0 * rms.log10() + 55.0) / 45.0).clamp(0.0, 1.0) as f32
}
/// Tail of the recording in progress, for the live preview (at most `LIVE_KEEP` samples).
static LIVE: Mutex<Vec<f32>> = Mutex::new(Vec::new());
/// Samples captured so far in the current recording; 0 when not recording.
static LIVE_TOTAL: AtomicUsize = AtomicUsize::new(0);
const LIVE_KEEP: usize = SAMPLE_RATE as usize * 20;

/// The last ≤20 s of the recording in progress and the total captured so far.
pub fn live_snapshot() -> (Vec<f32>, usize) {
    let v = LIVE.lock().map(|v| v.clone()).unwrap_or_default();
    (v, LIVE_TOTAL.load(Ordering::Acquire))
}

pub fn live_total() -> usize {
    LIVE_TOTAL.load(Ordering::Acquire)
}

pub(crate) fn live_reset() {
    if let Ok(mut v) = LIVE.lock() {
        v.clear();
        v.shrink_to(LIVE_KEEP);
    }
    LIVE_TOTAL.store(0, Ordering::Release);
}

pub(crate) fn live_push(s: &[f32]) {
    if let Ok(mut v) = LIVE.lock() {
        v.extend_from_slice(s);
        if v.len() > LIVE_KEEP + SAMPLE_RATE as usize * 5 {
            let cut = v.len() - LIVE_KEEP;
            v.drain(..cut);
        }
    }
    LIVE_TOTAL.fetch_add(s.len(), Ordering::AcqRel);
}

pub(crate) type CaptureResult = Result<Vec<f32>, String>;

/// For end-to-end tests without a microphone (CI, VMs): `DICTATE_FAKE_MIC=<16 kHz wav>`
/// "records" that file in real time, then silence, until stopped.
fn fake_capture(wav: &std::path::Path, stop: Arc<AtomicBool>, ready: mpsc::Sender<Result<(), String>>, max_samples: usize) -> CaptureResult {
    let pcm = match crate::wav::read_16k_mono(wav) {
        Ok(p) => p,
        Err(e) => {
            let _ = ready.send(Err(e.clone()));
            return Err(e);
        }
    };
    live_reset();
    let _ = ready.send(Ok(()));
    let mut out = Vec::new();
    let block = SAMPLE_RATE as usize / 20;
    let mut pos = 0;
    while !stop.load(Ordering::Acquire) && out.len() < max_samples {
        let chunk: Vec<f32> = (pos..pos + block).map(|i| pcm.get(i).copied().unwrap_or(0.0)).collect();
        pos += block;
        live_push(&chunk);
        LEVEL.store(loudness(&chunk).to_bits(), Ordering::Relaxed);
        out.extend_from_slice(&chunk);
        thread::sleep(std::time::Duration::from_millis(50));
    }
    LEVEL.store(0, Ordering::Relaxed);
    live_reset();
    Ok(out)
}

pub struct Recorder {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<CaptureResult>>,
}

impl Recorder {
    /// Opens the microphone named `device` (empty: the system default; one that isn't
    /// connected falls back to the default) and starts capturing. Returns once audio is flowing.
    pub fn start(max_seconds: u32, device: &str) -> Result<Self, String> {
        let device = device.trim().to_string();
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::channel();
        let stop2 = stop.clone();
        let max_samples = (max_seconds.max(1) * SAMPLE_RATE) as usize;
        let handle = thread::Builder::new()
            .name("audio".into())
            .spawn(move || match std::env::var_os("DICTATE_FAKE_MIC") {
                Some(wav) => fake_capture(std::path::Path::new(&wav), stop2, ready_tx, max_samples),
                None => capture(stop2, ready_tx, max_samples, &device),
            })
            .map_err(|e| format!("audio thread: {e}"))?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self { stop, handle: Some(handle) }),
            Ok(Err(e)) => {
                let _ = handle.join();
                Err(e)
            }
            Err(_) => Err(match handle.join() {
                Ok(Err(e)) => e,
                _ => "audio thread exited".into(),
            }),
        }
    }

    /// Stops capturing and returns 16 kHz mono samples in [-1, 1].
    pub fn stop(mut self) -> CaptureResult {
        self.stop.store(true, Ordering::Release);
        match self.handle.take().map(|h| h.join()) {
            Some(Ok(r)) => r,
            _ => Err("audio thread panicked".into()),
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            self.stop.store(true, Ordering::Release);
            let _ = h.join();
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    /// Opens every microphone for half a second (the mic indicator blinks), so it is
    /// opt-in: `cargo test --lib records_from_each_microphone -- --ignored`.
    #[test]
    #[ignore]
    fn records_from_each_microphone() {
        let devices = input_devices();
        assert!(!devices.is_empty(), "no microphones");
        for name in devices.iter().map(String::as_str).chain(["", "no such microphone"]) {
            let rec = Recorder::start(5, name).unwrap_or_else(|e| panic!("{name:?}: {e}"));
            thread::sleep(std::time::Duration::from_millis(500));
            let pcm = rec.stop().unwrap();
            println!("{name:?}: {} samples", pcm.len());
            assert!(pcm.len() > SAMPLE_RATE as usize / 4, "{name:?}: {} samples", pcm.len());
        }
    }
}
