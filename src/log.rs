//! Tiny append-only log in %LOCALAPPDATA%\dictate\dictate.log.
//! Only timings and errors go here, never recognized text.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static LOCK: Mutex<()> = Mutex::new(());

/// The app's data folder: %LOCALAPPDATA%\dictate (log, saved history).
pub fn data_dir() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(base).join("dictate"))
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
