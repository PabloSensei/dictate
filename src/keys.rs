//! Polling the keyboard: is the hotkey down, is any other key down?
//!
//! Hotkeys are identified by Windows virtual-key codes everywhere (that is what
//! dictate.ini stores); each platform maps them to its own key codes.
//! No hooks and no event taps: the state is sampled every 15 ms.

/// Esc cancels a dictation in toggle mode.
pub const VK_ESCAPE: i32 = 0x1B;

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

    pub fn down(vk: i32) -> bool {
        (unsafe { GetAsyncKeyState(vk) } as u16) & 0x8000 != 0
    }

    /// Not counted: generic Ctrl/Shift/Alt (they mirror the left/right codes),
    /// VK_PACKET and the menu mask key (our own SendInput), and for Right Alt the
    /// left Ctrl that AltGr layouts (Polish...) press along with it.
    pub fn other_down(hotkey: i32) -> bool {
        let altgr_ctrl = |k: i32| hotkey == 0xA5 && k == 0xA2;
        (0x01..=0xFE).any(|k| {
            k != hotkey
                && !matches!(k, 0x07 | 0x0A | 0x0B | 0x10..=0x12 | 0xE7 | 0xFF)
                && k != crate::inject::MASK_VK as i32
                && !altgr_ctrl(k)
                && down(k)
        })
    }

    pub fn supported(_vk: i32) -> bool {
        true
    }
}

#[cfg(target_os = "macos")]
mod imp {
    //! A key counts as down when either the HID key state or the device-dependent
    //! modifier bits (left and right are separate there) say so.

    const HID_SYSTEM_STATE: i32 = 1; // kCGEventSourceStateHIDSystemState

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventSourceFlagsState(state: i32) -> u64;
        fn CGEventSourceKeyState(state: i32, key: u16) -> bool;
    }

    /// (Windows VK, macOS keycode kVK_*, device-dependent flag bit NX_DEVICE*KEYMASK)
    const MODIFIERS: [(i32, u16, u64); 8] = [
        (0xA2, 0x3B, 0x0000_0001), // left control
        (0xA3, 0x3E, 0x0000_2000), // right control
        (0xA0, 0x38, 0x0000_0002), // left shift
        (0xA1, 0x3C, 0x0000_0004), // right shift
        (0xA4, 0x3A, 0x0000_0020), // left option
        (0xA5, 0x3D, 0x0000_0040), // right option
        (0x5B, 0x37, 0x0000_0008), // left command
        (0x5C, 0x36, 0x0000_0010), // right command
    ];
    const ALL_MODIFIER_BITS: u64 = 0x2000 | 0x1 | 0x2 | 0x4 | 0x20 | 0x40 | 0x8 | 0x10;
    const CAPS_LOCK: u16 = 0x39;
    const FN: u16 = 0x3F;

    fn keycode(vk: i32) -> Option<u16> {
        const F: [u16; 20] = [
            0x7A, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6D, 0x67, 0x6F, 0x69, 0x6B, 0x71, 0x6A, 0x40, 0x4F,
            0x50, 0x5A,
        ];
        if let Some(m) = MODIFIERS.iter().find(|m| m.0 == vk) {
            return Some(m.1);
        }
        match vk {
            0x70..=0x83 => Some(F[(vk - 0x70) as usize]), // F1..F20
            0x1B => Some(0x35),                           // escape
            _ => None,
        }
    }

    fn flag_bit(vk: i32) -> u64 {
        MODIFIERS.iter().find(|m| m.0 == vk).map_or(0, |m| m.2)
    }

    fn key_state(k: u16) -> bool {
        unsafe { CGEventSourceKeyState(HID_SYSTEM_STATE, k) }
    }

    fn flags() -> u64 {
        unsafe { CGEventSourceFlagsState(HID_SYSTEM_STATE) }
    }

    pub fn down(vk: i32) -> bool {
        let bit = flag_bit(vk);
        (bit != 0 && flags() & bit != 0) || keycode(vk).is_some_and(key_state)
    }

    pub fn other_down(hotkey: i32) -> bool {
        if flags() & ALL_MODIFIER_BITS & !flag_bit(hotkey) != 0 {
            return true;
        }
        let own = keycode(hotkey);
        (0u16..0x80).filter(|&k| Some(k) != own && k != CAPS_LOCK && k != FN).any(key_state)
    }

    pub fn supported(vk: i32) -> bool {
        keycode(vk).is_some()
    }
}

#[cfg(target_os = "linux")]
mod imp {
    //! X11 only: XQueryKeymap gives the state of every key in one round trip.
    //! Key codes are the standard evdev ones + 8, the same on every Xorg setup.

    use std::sync::Mutex;
    use x11rb::protocol::xproto::ConnectionExt;
    use x11rb::rust_connection::RustConnection;

    static CONN: Mutex<Option<RustConnection>> = Mutex::new(None);

    fn keycode(vk: i32) -> Option<u8> {
        Some(match vk {
            0xA2 => 37,  // Control_L
            0xA3 => 105, // Control_R
            0xA0 => 50,  // Shift_L
            0xA1 => 62,  // Shift_R
            0xA4 => 64,  // Alt_L
            0xA5 => 108, // Alt_R / ISO_Level3_Shift
            0x5B => 133, // Super_L
            0x5C => 134, // Super_R
            0x5D => 135, // Menu
            0x13 => 127, // Pause
            0x91 => 78,  // Scroll_Lock
            0x2D => 118, // Insert
            0x1B => 9,   // Escape
            0x70..=0x79 => 67 + (vk - 0x70) as u8, // F1..F10
            0x7A => 95,  // F11
            0x7B => 96,  // F12
            0x7C..=0x87 => 191 + (vk - 0x7C) as u8, // F13..F24
            _ => return None,
        })
    }

    fn keymap() -> Option<[u8; 32]> {
        let mut g = CONN.lock().ok()?;
        if g.is_none() {
            *g = RustConnection::connect(None).ok().map(|(c, _)| c);
        }
        let conn = g.as_ref()?;
        match conn.query_keymap().ok().and_then(|c| c.reply().ok()) {
            Some(r) => Some(r.keys),
            None => {
                *g = None; // reconnect next time
                None
            }
        }
    }

    fn bit(map: &[u8; 32], code: u8) -> bool {
        map[(code / 8) as usize] & (1 << (code % 8)) != 0
    }

    pub fn down(vk: i32) -> bool {
        match (keycode(vk), keymap()) {
            (Some(k), Some(m)) => bit(&m, k),
            _ => false,
        }
    }

    pub fn other_down(hotkey: i32) -> bool {
        let Some(m) = keymap() else { return false };
        let own = keycode(hotkey);
        (8u8..=255).any(|k| Some(k) != own && bit(&m, k) && !crate::inject::is_scratch(k))
    }

    pub fn supported(vk: i32) -> bool {
        keycode(vk).is_some()
    }

    /// For the UI: whether an X11 display is reachable at all.
    pub fn available() -> bool {
        keymap().is_some()
    }
}

pub use imp::*;
