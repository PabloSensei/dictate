//! Types text into the focused window as synthetic key events carrying Unicode
//! characters, so the keyboard layout doesn't matter and the clipboard is never touched.
//!
//! - Windows: SendInput(KEYEVENTF_UNICODE). Windows running as administrator ignore
//!   input from a non-elevated process (UIPI); the call then reports an error.
//! - macOS: CGEventKeyboardSetUnicodeString. Needs the Accessibility permission.
//! - Linux (X11): XTest key presses on spare keycodes that are temporarily mapped to
//!   the characters being typed (the xdotool technique).

#[cfg(windows)]
mod imp {
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
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::c_void;
    use std::time::Duration;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventCreateKeyboardEvent(source: *const c_void, key: u16, down: bool) -> *mut c_void;
        fn CGEventKeyboardSetUnicodeString(ev: *mut c_void, len: usize, s: *const u16);
        fn CGEventSetFlags(ev: *mut c_void, flags: u64);
        fn CGEventPost(tap: u32, ev: *mut c_void);
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(p: *const c_void);
    }
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }

    const HID_EVENT_TAP: u32 = 0; // kCGHIDEventTap
    /// Some apps read at most 20 UTF-16 units from one event.
    const CHUNK: usize = 20;

    pub fn mask_alt_menu() {}

    /// Allowed to post keyboard events (System Settings > Privacy & Security > Accessibility)?
    pub fn trusted() -> bool {
        unsafe { AXIsProcessTrusted() }
    }

    pub fn type_text(text: &str) -> Result<(), String> {
        if !trusted() {
            return Err("macOS doesn't let dictate type yet: turn it on in System Settings > Privacy & Security > \
                        Accessibility, then restart dictate"
                .into());
        }
        let units: Vec<u16> = text.replace(['\n', '\r'], " ").encode_utf16().collect();
        let mut i = 0;
        while i < units.len() {
            let mut end = (i + CHUNK).min(units.len());
            // never split a surrogate pair
            if end < units.len() && (0xDC00..0xE000).contains(&units[end]) {
                end -= 1;
            }
            let chunk = &units[i..end];
            for down in [true, false] {
                unsafe {
                    let ev = CGEventCreateKeyboardEvent(std::ptr::null(), 0, down);
                    if ev.is_null() {
                        return Err("CGEventCreateKeyboardEvent failed".into());
                    }
                    // the hotkey (Option...) may still be held: type plain characters
                    CGEventSetFlags(ev, 0);
                    CGEventKeyboardSetUnicodeString(ev, chunk.len(), chunk.as_ptr());
                    CGEventPost(HID_EVENT_TAP, ev);
                    CFRelease(ev);
                }
            }
            i = end;
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::Duration;

    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::ConnectionExt as _;
    use x11rb::protocol::xtest::ConnectionExt as _;
    use x11rb::rust_connection::RustConnection;
    use x11rb::wrapper::ConnectionExt as _;

    const KEY_PRESS: u8 = 2;
    const KEY_RELEASE: u8 = 3;
    /// Pause after remapping keycodes, so clients pick up the new mapping before the
    /// key events arrive, and after typing, before the keycodes are remapped again.
    const SETTLE: Duration = Duration::from_millis(25);

    /// Keycodes borrowed for typing right now (the key poller ignores them).
    static SCRATCH: Mutex<Vec<u8>> = Mutex::new(Vec::new());

    pub fn mask_alt_menu() {}

    pub fn is_scratch(code: u8) -> bool {
        SCRATCH.lock().map(|v| v.contains(&code)).unwrap_or(false)
    }

    /// X keysym for a character: Latin-1 maps directly, everything else is 0x0100_0000 + code point.
    pub fn keysym(c: char) -> u32 {
        let u = c as u32;
        if (0x20..=0x7E).contains(&u) || (0xA0..=0xFF).contains(&u) { u } else { 0x0100_0000 + u }
    }

    pub fn type_text(text: &str) -> Result<(), String> {
        let text = text.replace(['\n', '\r'], " ");
        if text.is_empty() {
            return Ok(());
        }
        let (conn, screen) = RustConnection::connect(None).map_err(|e| format!("X11: {e}"))?;
        let root = conn.setup().roots[screen].root;
        let (min, max) = (conn.setup().min_keycode, conn.setup().max_keycode);
        let e = |e: &dyn std::fmt::Display| format!("X11 typing: {e}");
        let map = conn
            .get_keyboard_mapping(min, max - min + 1)
            .map_err(|x| e(&x))?
            .reply()
            .map_err(|x| e(&x))?;
        let per = map.keysyms_per_keycode.max(1) as usize;
        let free: Vec<u8> = (min..=max)
            .filter(|&k| map.keysyms[(k - min) as usize * per..][..per].iter().all(|&s| s == 0))
            .collect();
        if free.is_empty() {
            return Err("X11 typing: no spare keycode to type with".into());
        }
        *SCRATCH.lock().unwrap() = free.clone();
        let chars: Vec<char> = text.chars().collect();
        let res = type_chars(&conn, root, &free, &chars);
        // give the keycodes back
        for &k in &free {
            let _ = conn.change_keyboard_mapping(1, k, 1, &[0]);
        }
        let _ = conn.flush();
        SCRATCH.lock().unwrap().clear();
        res
    }

    fn type_chars(conn: &RustConnection, root: u32, free: &[u8], chars: &[char]) -> Result<(), String> {
        let e = |e: &dyn std::fmt::Display| format!("X11 typing: {e}");
        let mut i = 0;
        while i < chars.len() {
            // the longest run of text whose distinct characters fit in the spare keycodes
            let mut slot: HashMap<char, u8> = HashMap::new();
            let mut end = i;
            while end < chars.len() {
                let c = chars[end];
                if !slot.contains_key(&c) {
                    if slot.len() == free.len() {
                        break;
                    }
                    slot.insert(c, free[slot.len()]);
                }
                end += 1;
            }
            for (&c, &k) in &slot {
                let s = keysym(c);
                // the same symbol at both levels of every group (core order: G1L1 G1L2 G2L1 G2L2 ...),
                // so it types whatever layout (us, ru...) is active
                conn.change_keyboard_mapping(1, k, 8, &[s; 8]).map_err(|x| e(&x))?;
            }
            conn.sync().map_err(|x| e(&x))?;
            std::thread::sleep(SETTLE);
            for &c in &chars[i..end] {
                let k = slot[&c];
                conn.xtest_fake_input(KEY_PRESS, k, 0, root, 0, 0, 0).map_err(|x| e(&x))?;
                conn.xtest_fake_input(KEY_RELEASE, k, 0, root, 0, 0, 0).map_err(|x| e(&x))?;
            }
            conn.sync().map_err(|x| e(&x))?;
            std::thread::sleep(SETTLE);
            i = end;
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn keysyms() {
            assert_eq!(super::keysym('a'), 0x61);
            assert_eq!(super::keysym('é'), 0xE9);
            assert_eq!(super::keysym('ж'), 0x0100_0436);
            assert_eq!(super::keysym('😀'), 0x0101_F600);
        }
    }
}

pub use imp::*;
