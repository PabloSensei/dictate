//! Offline push-to-talk dictation for Windows.
//!
//! No network code anywhere: audio goes microphone -> RAM -> whisper.cpp -> SendInput.
//! Nothing is written to disk except the config and a log of timings (never the text),
//! unless you turn on `history = disk`. The clipboard is used only if you ask for it.

pub mod audio;
pub mod autostart;
pub mod bubble;
pub mod clipboard;
pub mod config;
pub mod engine;
pub mod history;
pub mod i18n;
pub mod paint;
pub mod inject;
pub mod log;
pub mod metrics;
pub mod replace;
pub mod surface;
pub mod trigger;
pub mod wav;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
