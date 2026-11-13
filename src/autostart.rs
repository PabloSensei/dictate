//! Start at sign-in: the per-user `Run` key (no admin rights needed).
//!
//! Task Manager's "Startup apps" can disable an entry without deleting it; that
//! state lives in `Explorer\StartupApproved\Run` (first byte 2 = on, 3 = off).
//! Enabling here clears such a "disabled" mark so the switch means what it says.

use std::path::Path;
use std::ptr::null_mut;

use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};

use crate::wide;

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const NAME: &str = "Dictate";

fn read_string(subkey: &str, name: &str) -> Option<String> {
    let (k, n) = (wide(subkey), wide(name));
    let mut buf = vec![0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr(), RRF_RT_REG_SZ, null_mut(), buf.as_mut_ptr() as *mut _, &mut size)
    };
    if r != 0 {
        return None;
    }
    let len = (size as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// Task Manager switched this entry off?
fn disabled_in_task_manager() -> bool {
    let (k, n) = (wide(APPROVED), wide(NAME));
    let mut buf = [0u8; 16];
    let mut size = buf.len() as u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr(), RRF_RT_REG_BINARY, null_mut(), buf.as_mut_ptr() as *mut _, &mut size)
    };
    r == 0 && size > 0 && buf[0] & 1 == 1
}

fn command_for(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

/// On only if the entry exists, points at this very exe and isn't disabled in Task Manager.
pub fn is_enabled(exe: &Path) -> bool {
    read_string(RUN, NAME).is_some_and(|v| v.eq_ignore_ascii_case(&command_for(exe))) && !disabled_in_task_manager()
}

pub fn set(enabled: bool, exe: &Path) -> Result<(), String> {
    let (k, n) = (wide(RUN), wide(NAME));
    let r = if enabled {
        let cmd = wide(&command_for(exe));
        unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr(), REG_SZ, cmd.as_ptr() as *const _, (cmd.len() * 2) as u32) }
    } else {
        match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr()) } {
            2 => 0, // ERROR_FILE_NOT_FOUND: already off
            e => e,
        }
    };
    if r != 0 {
        return Err(format!("autostart: registry error {r}"));
    }
    // Forget any "disabled" mark from Task Manager either way (absent = enabled).
    let a = wide(APPROVED);
    unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, a.as_ptr(), n.as_ptr()) };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Touches the real HKCU Run key, so it is opt-in: `.\build.ps1 test --lib autostart '--' --ignored`.
    #[test]
    #[ignore]
    fn roundtrip_in_registry() {
        let exe = Path::new(r"C:\does-not-exist\dictate.exe");
        let was = read_string(RUN, NAME);
        set(true, exe).unwrap();
        assert!(is_enabled(exe));
        assert!(!is_enabled(Path::new(r"C:\other\dictate.exe")), "another copy's entry is not ours");
        set(false, exe).unwrap();
        assert!(!is_enabled(exe));
        set(false, exe).unwrap(); // already off is fine
        if let Some(v) = was {
            let (k, n, d) = (wide(RUN), wide(NAME), wide(&v));
            unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr(), REG_SZ, d.as_ptr() as *const _, (d.len() * 2) as u32) };
        }
    }
}
