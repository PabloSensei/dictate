//! Copying text to the clipboard: only when `clipboard = true` (each dictation) or
//! when you press Copy in the history. The app never reads the clipboard.
//!
//! The copy is marked so that Windows doesn't sync it to other devices through the
//! cloud clipboard ("CanUploadToCloudClipboard" = 0); local Win+V history still works.

use std::ptr::copy_nonoverlapping;
#[cfg(test)]
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{GlobalFree, HANDLE, HWND};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};

use crate::wide;

const CF_UNICODETEXT: u32 = 13;

/// Moves `bytes` into a new movable global block (what SetClipboardData wants).
unsafe fn global_block(bytes: &[u8]) -> Option<HANDLE> {
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1));
        if h.is_null() {
            return None;
        }
        let p = GlobalLock(h) as *mut u8;
        if p.is_null() {
            GlobalFree(h);
            return None;
        }
        copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        GlobalUnlock(h);
        Some(h)
    }
}

/// Puts `text` on the clipboard. `owner` must be a window of this process (Windows
/// refuses clipboard data from a clipboard opened without one). The handle is only
/// passed on to OpenClipboard, never dereferenced.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn set_text(owner: HWND, text: &str) -> Result<(), String> {
    let utf16: Vec<u8> = wide(text).iter().flat_map(|u| u.to_le_bytes()).collect();
    unsafe {
        // Another app may hold the clipboard for a moment.
        let mut opened = false;
        for _ in 0..10 {
            if OpenClipboard(owner) != 0 {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if !opened {
            return Err("clipboard is busy".into());
        }
        let result = (|| {
            if EmptyClipboard() == 0 {
                return Err("EmptyClipboard failed".to_string());
            }
            let h = global_block(&utf16).ok_or("out of memory")?;
            if SetClipboardData(CF_UNICODETEXT, h).is_null() {
                GlobalFree(h);
                return Err("SetClipboardData failed".to_string());
            }
            let no_cloud = RegisterClipboardFormatW(wide("CanUploadToCloudClipboard").as_ptr());
            if no_cloud != 0
                && let Some(h) = global_block(&0u32.to_le_bytes())
                && SetClipboardData(no_cloud, h).is_null()
            {
                GlobalFree(h);
            }
            Ok(())
        })();
        CloseClipboard();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::DataExchange::{GetClipboardData, IsClipboardFormatAvailable};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, HWND_MESSAGE};

    fn read_text(owner: HWND) -> Option<String> {
        unsafe {
            if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 || OpenClipboard(owner) == 0 {
                return None;
            }
            let h = GetClipboardData(CF_UNICODETEXT);
            let p = GlobalLock(h) as *const u16;
            let s = (!p.is_null()).then(|| {
                let len = (0..).take_while(|&i| *p.add(i) != 0).count();
                String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
            });
            GlobalUnlock(h);
            CloseClipboard();
            s
        }
    }

    /// Uses the real clipboard, so it is opt-in: `.\build.ps1 test --lib clipboard '--' --ignored`.
    /// Whatever text was on the clipboard is put back afterwards (and never printed).
    #[test]
    #[ignore]
    fn copies_unicode_text() {
        unsafe {
            let class = wide("STATIC");
            let owner = CreateWindowExW(0, class.as_ptr(), null(), 0, 0, 0, 0, 0, HWND_MESSAGE, null_mut(), GetModuleHandleW(null()), null());
            assert!(!owner.is_null());
            let before = read_text(owner);
            let sample = "dictate test: Привет, Zażółć gęślą jaźń 😀\nsecond line";
            set_text(owner, sample).unwrap();
            let got = read_text(owner);
            let no_cloud = RegisterClipboardFormatW(wide("CanUploadToCloudClipboard").as_ptr());
            let flagged = IsClipboardFormatAvailable(no_cloud) != 0;
            if let Some(b) = before {
                set_text(owner, &b).unwrap();
            }
            DestroyWindow(owner);
            assert_eq!(got.as_deref(), Some(sample));
            assert!(flagged, "cloud clipboard opt-out is missing");
        }
    }
}
