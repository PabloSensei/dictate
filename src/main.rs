//! Tray app: hold the hotkey, speak, release -> text is typed into the focused window.
//!
//! `app` is the shared core (threads, state); each platform module is the UI shell:
//! tray icon, menu, dictation bubble, and the main loop.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod mac;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod win;

fn main() {
    #[cfg(windows)]
    win::main();
    #[cfg(target_os = "linux")]
    linux::main();
    #[cfg(target_os = "macos")]
    mac::main();
}
