//! Microphone capture via the classic waveIn API: 16 kHz mono PCM16.
//! Windows' audio mapper does the resampling, so no DSP code lives here.
//! The device is opened only while the hotkey is held, so the mic indicator
//! is on exactly when we are listening.

use std::ptr::{null, null_mut};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering, fence};
use std::sync::{Mutex, mpsc};
use std::thread::{self, JoinHandle};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Media::Audio::{
    CALLBACK_EVENT, HWAVEIN, WAVE_FORMAT_PCM, WAVE_MAPPER, WAVEFORMATEX, WAVEHDR, WHDR_DONE,
    waveInAddBuffer, waveInClose, waveInOpen, waveInPrepareHeader, waveInReset, waveInStart,
    waveInUnprepareHeader,
};
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

pub const SAMPLE_RATE: u32 = 16_000;

/// Loudness of the latest 50 ms buffer, 0..1 (f32 bits), for the on-screen meter.
pub static LEVEL: AtomicU32 = AtomicU32::new(0);

pub fn level() -> f32 {
    f32::from_bits(LEVEL.load(Ordering::Relaxed))
}

/// RMS mapped from -55..-10 dBFS onto 0..1.
fn loudness(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&s| (s as f64 / 32768.0).powi(2)).sum();
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

fn live_reset() {
    if let Ok(mut v) = LIVE.lock() {
        v.clear();
        v.shrink_to(LIVE_KEEP);
    }
    LIVE_TOTAL.store(0, Ordering::Release);
}

fn live_push(s: &[f32]) {
    if let Ok(mut v) = LIVE.lock() {
        v.extend_from_slice(s);
        if v.len() > LIVE_KEEP + SAMPLE_RATE as usize * 5 {
            let cut = v.len() - LIVE_KEEP;
            v.drain(..cut);
        }
    }
    LIVE_TOTAL.fetch_add(s.len(), Ordering::AcqRel);
}

const BUF_SAMPLES: usize = 800; // 50 ms per buffer: responsive level meter
const N_BUFS: usize = 16;
const HDR_SIZE: u32 = std::mem::size_of::<WAVEHDR>() as u32;

type CaptureResult = Result<Vec<f32>, String>;

pub struct Recorder {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<CaptureResult>>,
}

impl Recorder {
    /// Opens the default microphone and starts capturing. Returns once audio is flowing.
    pub fn start(max_seconds: u32) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::channel();
        let stop2 = stop.clone();
        let max_samples = (max_seconds.max(1) * SAMPLE_RATE) as usize;
        let handle = thread::Builder::new()
            .name("audio".into())
            .spawn(move || capture(stop2, ready_tx, max_samples))
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

struct Device {
    hwi: HWAVEIN,
    event: HANDLE,
}

impl Drop for Device {
    fn drop(&mut self) {
        unsafe {
            if !self.hwi.is_null() {
                waveInReset(self.hwi);
                waveInClose(self.hwi);
            }
            if !self.event.is_null() {
                CloseHandle(self.event);
            }
        }
    }
}

fn capture(stop: Arc<AtomicBool>, ready: mpsc::Sender<Result<(), String>>, max_samples: usize) -> CaptureResult {
    let fail = |ready: &mpsc::Sender<Result<(), String>>, msg: String| {
        let _ = ready.send(Err(msg.clone()));
        Err(msg)
    };

    // Buffers and headers must not move or be freed while the driver owns them:
    // allocated once, never resized, and declared before `dev` so they drop after it.
    let mut bufs: Box<[[i16; BUF_SAMPLES]]> = vec![[0i16; BUF_SAMPLES]; N_BUFS].into_boxed_slice();
    let mut hdrs: Box<[WAVEHDR]> = (0..N_BUFS).map(|_| WAVEHDR::default()).collect();

    let event = unsafe { CreateEventW(null(), 0, 0, null()) };
    if event.is_null() {
        return fail(&ready, "CreateEventW failed".into());
    }
    let mut dev = Device { hwi: null_mut(), event };

    let fmt = WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_PCM as u16,
        nChannels: 1,
        nSamplesPerSec: SAMPLE_RATE,
        nAvgBytesPerSec: SAMPLE_RATE * 2,
        nBlockAlign: 2,
        wBitsPerSample: 16,
        cbSize: 0,
    };
    let r = unsafe { waveInOpen(&mut dev.hwi, WAVE_MAPPER, &fmt, event as usize, 0, CALLBACK_EVENT) };
    if r != 0 {
        dev.hwi = null_mut();
        return fail(&ready, format!("microphone unavailable (waveInOpen = {r})"));
    }

    for i in 0..N_BUFS {
        hdrs[i].lpData = bufs[i].as_mut_ptr() as *mut u8;
        hdrs[i].dwBufferLength = (BUF_SAMPLES * 2) as u32;
        unsafe {
            if waveInPrepareHeader(dev.hwi, &mut hdrs[i], HDR_SIZE) != 0
                || waveInAddBuffer(dev.hwi, &mut hdrs[i], HDR_SIZE) != 0
            {
                return fail(&ready, "waveIn buffer setup failed".into());
            }
        }
    }
    live_reset();
    if unsafe { waveInStart(dev.hwi) } != 0 {
        return fail(&ready, "waveInStart failed".into());
    }
    let _ = ready.send(Ok(()));

    let mut out: Vec<f32> = Vec::with_capacity(SAMPLE_RATE as usize * 20);
    let mut next = 0usize; // buffers complete in submission order
    let take = |h: &WAVEHDR, buf: &[i16; BUF_SAMPLES], out: &mut Vec<f32>| {
        let n = (h.dwBytesRecorded as usize / 2).min(BUF_SAMPLES);
        let room = max_samples.saturating_sub(out.len());
        let start = out.len();
        out.extend(buf[..n.min(room)].iter().map(|&s| s as f32 / 32768.0));
        live_push(&out[start..]);
        LEVEL.store(loudness(&buf[..n]).to_bits(), Ordering::Relaxed);
    };

    loop {
        let stopping = stop.load(Ordering::Acquire);
        if stopping {
            // Returns every queued buffer to us, marked done (possibly partially filled).
            unsafe { waveInReset(dev.hwi) };
        } else {
            unsafe { WaitForSingleObject(dev.event, 50) };
        }
        for _ in 0..N_BUFS {
            // WAVEHDR is packed(1) and the driver writes dwFlags from another thread:
            // read it through a raw pointer, unaligned, never via a cached reference.
            let flags = unsafe { std::ptr::addr_of!((*hdrs.as_ptr().add(next)).dwFlags).read_unaligned() };
            if flags & WHDR_DONE == 0 {
                break;
            }
            fence(Ordering::Acquire);
            take(&hdrs[next], &bufs[next], &mut out);
            if stopping {
                hdrs[next].dwFlags &= !WHDR_DONE;
            } else if unsafe { waveInAddBuffer(dev.hwi, &mut hdrs[next], HDR_SIZE) } != 0 {
                return Err("waveInAddBuffer failed".into());
            }
            next = (next + 1) % N_BUFS;
        }
        if stopping {
            break;
        }
    }

    LEVEL.store(0, Ordering::Relaxed);
    live_reset();
    for h in hdrs.iter_mut() {
        unsafe { waveInUnprepareHeader(dev.hwi, h, HDR_SIZE) };
    }
    drop(dev);
    Ok(out)
}
