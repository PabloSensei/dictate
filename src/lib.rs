//! Offline push-to-talk dictation for Windows, macOS and Linux (X11).
//!
//! No network code anywhere: audio goes microphone -> RAM -> whisper.cpp -> typed keys.
//! Nothing is written to disk except the config and a log of timings (never the text),
//! unless you turn on `history = disk`. The clipboard is used only if you ask for it.

pub mod audio;
#[cfg(windows)]
pub mod autostart;
#[cfg(not(windows))]
#[path = "autostart_unix.rs"]
pub mod autostart;
pub mod bubble;
#[cfg(not(windows))]
pub mod caption;
#[cfg(windows)]
pub mod clipboard;
#[cfg(not(windows))]
#[path = "clipboard_unix.rs"]
pub mod clipboard;
pub mod config;
pub mod engine;
pub mod history;
pub mod i18n;
pub mod icon;
pub mod inject;
pub mod keys;
pub mod log;
pub mod metrics;
pub mod paint;
pub mod replace;
#[cfg(windows)]
pub mod surface;
pub mod trigger;
pub mod wav;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
