//! Types text into the focused window with SendInput(KEYEVENTF_UNICODE).
//! The clipboard is never touched. Windows running as administrator ignore
//! input from a non-elevated process (UIPI); the call then reports an error.

use std::mem::size_of;

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, SendInput,
};

/// Taps an unassigned virtual key (0xE8). Sent while Alt is held, it makes
/// Windows treat Alt as part of a combination, so releasing it doesn't
/// activate the focused app's menu bar (the AutoHotkey "menu mask" trick).
pub fn mask_alt_menu() {
    let ev = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: MASK_VK, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    };
    let inputs = [ev(0), ev(KEYEVENTF_KEYUP)];
    unsafe { SendInput(2, inputs.as_ptr(), size_of::<INPUT>() as i32) };
}

pub const MASK_VK: u16 = 0xE8;

pub fn type_text(text: &str) -> Result<(), String> {
    let mut inputs: Vec<INPUT> = Vec::with_capacity(text.len() * 4);
    for unit in text.encode_utf16() {
        let unit = if unit == '\n' as u16 || unit == '\r' as u16 { ' ' as u16 } else { unit };
        for flags in [KEYEVENTF_UNICODE, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP] {
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT { wVk: 0, wScan: unit, dwFlags: flags, time: 0, dwExtraInfo: 0 },
                },
            });
        }
    }
    if inputs.is_empty() {
        return Ok(());
    }
    let sent = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), size_of::<INPUT>() as i32) };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        Err(format!(
            "input blocked: sent {sent} of {} events (is the target window running as administrator?)",
            inputs.len()
        ))
    }
}
