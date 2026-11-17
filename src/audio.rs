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

