//! waveIn capture (Windows): 16 kHz mono PCM16 straight from the audio mapper.

use std::ptr::{null, null_mut};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering, fence};
use std::sync::mpsc;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Media::Audio::{
    CALLBACK_EVENT, HWAVEIN, WAVE_FORMAT_PCM, WAVE_MAPPER, WAVEFORMATEX, WAVEHDR, WHDR_DONE,
    waveInAddBuffer, waveInClose, waveInOpen, waveInPrepareHeader, waveInReset, waveInStart,
    waveInUnprepareHeader,
};
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use super::{CaptureResult, LEVEL, SAMPLE_RATE, live_push, live_reset, loudness};

const BUF_SAMPLES: usize = 800; // 50 ms per buffer: responsive level meter
const N_BUFS: usize = 16;
const HDR_SIZE: u32 = std::mem::size_of::<WAVEHDR>() as u32;

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

pub(super) fn capture(stop: Arc<AtomicBool>, ready: mpsc::Sender<Result<(), String>>, max_samples: usize) -> CaptureResult {
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
        let level = loudness(&buf[..n].iter().map(|&s| s as f32 / 32768.0).collect::<Vec<_>>());
        LEVEL.store(level.to_bits(), Ordering::Relaxed);
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
