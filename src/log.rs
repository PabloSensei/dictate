//! Tiny append-only log, dictate.log in the app's data folder.
//! Only timings and errors go here, never recognized text.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static LOCK: Mutex<()> = Mutex::new(());

/// The app's data folder (log, saved history; on macOS and Linux also dictate.ini and
/// models): %LOCALAPPDATA%\dictate, ~/Library/Application Support/dictate, or
/// $XDG_DATA_HOME/dictate (~/.local/share/dictate).
pub fn data_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let base = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
    #[cfg(target_os = "macos")]
    let base = PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support");
    #[cfg(all(unix, not(target_os = "macos")))]
    let base = match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(v) => PathBuf::from(v),
        None => PathBuf::from(std::env::var_os("HOME")?).join(".local/share"),
    };
    Some(base.join("dictate"))
}

pub fn path() -> Option<PathBuf> {
    Some(data_dir()?.join("dictate.log"))
}

pub fn write(msg: &str) {
    let _g = LOCK.lock();
    let Some(p) = path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Keep the log small: start over past 1 MB.
    if std::fs::metadata(&p).map(|m| m.len() > 1 << 20).unwrap_or(false) {
        let _ = std::fs::remove_file(&p);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let _ = writeln!(f, "{secs} {msg}");
    }
}
