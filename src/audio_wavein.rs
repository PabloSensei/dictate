//! waveIn capture (Windows): 16 kHz mono PCM16 straight from the audio mapper.

use std::ptr::{null, null_mut};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering, fence};
use std::sync::mpsc;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Media::Audio::{
    CALLBACK_EVENT, HWAVEIN, WAVE_FORMAT_PCM, WAVE_MAPPER, WAVEFORMATEX, WAVEHDR, WAVEINCAPSW, WHDR_DONE,
    waveInAddBuffer, waveInClose, waveInGetDevCapsW, waveInGetNumDevs, waveInMessage, waveInOpen, waveInPrepareHeader,
    waveInReset, waveInStart,
    waveInUnprepareHeader,
};
use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
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

/// Short waveIn name (the API cuts it at 31 characters).
fn short_name(i: u32) -> String {
    let mut caps: WAVEINCAPSW = unsafe { std::mem::zeroed() };
    let r = unsafe { waveInGetDevCapsW(i as usize, &mut caps, std::mem::size_of::<WAVEINCAPSW>() as u32) };
    if r != 0 {
        return String::new();
    }
    let name = caps.szPname;
    let n = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    String::from_utf16_lossy(&name[..n])
}

/// The full name Windows shows ("Microphone (DJI MIC MINI)"): the device's endpoint
/// id from the driver, then its description and interface name from the registry.
fn full_name(i: u32) -> Option<String> {
    const DRV_QUERYFUNCTIONINSTANCEIDSIZE: u32 = 0x0812;
    const DRV_QUERYFUNCTIONINSTANCEID: u32 = 0x0811; // DRV_RESERVED + 17; the size query is + 18
    let h = i as usize as HWAVEIN;
    let mut size = 0u32;
    if unsafe { waveInMessage(h, DRV_QUERYFUNCTIONINSTANCEIDSIZE, &mut size as *mut u32 as usize, 0) } != 0 || size < 4 {
        return None;
    }
    let mut buf = vec![0u16; size as usize / 2];
    if unsafe { waveInMessage(h, DRV_QUERYFUNCTIONINSTANCEID, buf.as_mut_ptr() as usize, size as usize) } != 0 {
        return None;
    }
    let id = String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(buf.len())]);
    // "{0.0.1.00000000}.{guid}": the endpoint's key is the second part
    let guid = id.rsplit_once("}.").map(|(_, g)| g)?;
    let key = format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture\{guid}\Properties");
    let desc = reg_string(&key, "{a45c254e-df1c-4efd-8020-67d146a850e0},2")?; // PKEY_Device_DeviceDesc
    Some(match reg_string(&key, "{b3f8fa53-0004-438e-9003-51a46e139bfc},6") {
        // PKEY_DeviceInterface_FriendlyName
        Some(iface) if !iface.is_empty() => format!("{desc} ({iface})"),
        _ => desc,
    })
}

fn reg_string(key: &str, value: &str) -> Option<String> {
    let (k, v) = (crate::wide(key), crate::wide(value));
    let mut buf = vec![0u16; 512];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(HKEY_LOCAL_MACHINE, k.as_ptr(), v.as_ptr(), RRF_RT_REG_SZ, null_mut(), buf.as_mut_ptr().cast(), &mut size)
    };
    if r != 0 {
        return None;
    }
    let n = (size as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..n]))
}

/// Microphone names, by waveIn device id: the full name Windows shows, or the
/// shortened waveIn name if that can't be read.
pub(super) fn devices() -> Vec<String> {
    (0..unsafe { waveInGetNumDevs() }).map(|i| full_name(i).unwrap_or_else(|| short_name(i))).collect()
}

/// The waveIn id for a device name; the system default (the mapper) when it's empty or gone.
fn device_id(name: &str) -> u32 {
    if name.is_empty() {
        return WAVE_MAPPER;
    }
    let n = unsafe { waveInGetNumDevs() };
    let by_full = devices().iter().position(|d| d == name);
    match by_full.or_else(|| (0..n as usize).find(|&i| short_name(i as u32) == name)) {
        Some(i) => i as u32,
        None => {
            crate::log::write(&format!("microphone \"{name}\" not found, using the default one"));
            WAVE_MAPPER
        }
    }
}

pub(super) fn capture(
    stop: Arc<AtomicBool>,
    ready: mpsc::Sender<Result<(), String>>,
    max_samples: usize,
    device: &str,
) -> CaptureResult {
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
    let r = unsafe { waveInOpen(&mut dev.hwi, device_id(device), &fmt, event as usize, 0, CALLBACK_EVENT) };
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
