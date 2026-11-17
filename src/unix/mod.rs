//! Shared by the macOS and Linux UIs: single-instance lock, command line, the
//! bubble animation state, and small helpers.

pub mod anim;

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use dictate::{config, log};

/// Held for the life of the process: an exclusive lock on `dictate.lock` in the data folder.
pub struct InstanceLock {
    _file: File,
}

fn lock_path() -> PathBuf {
    log::data_dir().unwrap_or_else(std::env::temp_dir).join("dictate.lock")
}

/// `Ok(None)` when another copy is running.
pub fn lock_instance() -> Result<Option<InstanceLock>, String> {
    let p = lock_path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&p)
        .map_err(|e| format!("{}: {e}", p.display()))?;
    match f.try_lock() {
        Ok(()) => {
            let _ = f.set_len(0);
            let _ = write!(f, "{}", std::process::id());
            let _ = f.flush();
            Ok(Some(InstanceLock { _file: f }))
        }
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(format!("{}: {e}", p.display())),
    }
}

/// The process id the running copy wrote into the lock file.
pub fn running_pid() -> Option<u32> {
    let mut s = String::new();
    File::open(lock_path()).ok()?.read_to_string(&mut s).ok()?;
    s.trim().parse().ok()
}

pub const USAGE: &str = "dictate: offline push-to-talk dictation

usage: dictate [--quit | --version | --help]
  (no options)  start in the tray, or say that a copy is already running
  --quit        stop the running copy

Settings live in dictate.ini, models in models/ next to it:";

pub enum Cli {
    Run,
    Done,
}

/// Handles --help, --version, --quit, and a second launch. `Cli::Run` means: start the app.
pub fn command_line(locked: bool) -> Cli {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |a: &str| args.iter().any(|x| x == a);
    if has("--help") || has("-h") {
        println!("{USAGE}\n  {}", config::default_path().display());
        return Cli::Done;
    }
    if has("--version") || has("-V") {
        println!("dictate {}", env!("CARGO_PKG_VERSION"));
        return Cli::Done;
    }
    if has("--quit") {
        match (locked, running_pid()) {
            (false, Some(pid)) => {
                let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
                println!("dictate: stopped (pid {pid})");
            }
            _ => println!("dictate: not running"),
        }
        return Cli::Done;
    }
    if !locked {
        let pid = running_pid().map(|p| format!(" (pid {p})")).unwrap_or_default();
        eprintln!("dictate is already running{pid}. Use its tray icon, or `dictate --quit` to stop it.");
        return Cli::Done;
    }
    Cli::Run
}

/// Opens a file or folder with the default app.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn open_path(p: &Path) {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    if let Err(e) = std::process::Command::new(opener).arg(p).spawn() {
        log::write(&format!("{opener} {}: {e}", p.display()));
    }
}

/// Explains what to do when the model file is not there yet.
pub fn missing_model_hint(model: &Path) -> Option<String> {
    if model.exists() {
        return None;
    }
    let script = if cfg!(target_os = "macos") {
        "\"/Applications/Dictate.app/Contents/Resources/download-models.sh\"".to_string()
    } else {
        std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|d| d.join("download-models.sh")))
            .filter(|p| p.exists())
            .map(|p| format!("\"{}\"", p.display()))
            .unwrap_or_else(|| "./download-models.sh".into())
    };
    Some(format!(
        "Speech model not found: {}\nDownload it (about 0.5 GB) in a terminal:\n  {script}\nthen restart dictate.",
        model.display()
    ))
}

/// History entries for a menu: newest first, one line, shortened.
pub fn menu_entries(n: usize) -> Vec<(String, String)> {
    let all = crate::app::history().entries();
    all.iter()
        .rev()
        .take(n)
        .map(|e| {
            let one_line = e.text.split_whitespace().collect::<Vec<_>>().join(" ");
            let label = if one_line.chars().count() > 60 {
                format!("{}…", one_line.chars().take(59).collect::<String>())
            } else {
                one_line
            };
            (label, e.text.clone())
        })
        .collect()
}
