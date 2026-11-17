//! Start at sign-in, per user, no admin rights.
//!
//! - macOS: a LaunchAgent, `~/Library/LaunchAgents/io.github.pablosensei.dictate.plist`.
//!   For an app bundle it runs `open -a Dictate.app`, so the app starts the normal way.
//! - Linux: an XDG autostart entry, `~/.config/autostart/dictate.desktop`.
//!
//! Like on Windows, the switch is on only if the entry points at this very copy.

use std::path::{Path, PathBuf};

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(target_os = "macos")]
fn entry_path() -> Option<PathBuf> {
    Some(home()?.join("Library/LaunchAgents/io.github.pablosensei.dictate.plist"))
}

#[cfg(not(target_os = "macos"))]
fn entry_path() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(v) => PathBuf::from(v),
        None => home()?.join(".config"),
    };
    Some(base.join("autostart/dictate.desktop"))
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// `/Applications/Dictate.app` for `/Applications/Dictate.app/Contents/MacOS/dictate`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")))
    .then(|| bundle.to_path_buf())
}

#[cfg(target_os = "macos")]
fn entry_for(exe: &Path) -> String {
    let args: Vec<String> = match bundle_of(exe) {
        Some(b) => vec!["/usr/bin/open".into(), "-a".into(), b.display().to_string()],
        None => vec![exe.display().to_string()],
    };
    let args: String = args.iter().map(|a| format!("\n        <string>{}</string>", xml_escape(a))).collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>io.github.pablosensei.dictate</string>
    <key>ProgramArguments</key>
    <array>{args}
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
"#
    )
}

/// Desktop Entry `Exec` quoting: double quotes, with `"`, `` ` ``, `$` and `\` escaped.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn exec_quote(p: &Path) -> String {
    let mut s = String::from("\"");
    for c in p.display().to_string().chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            s.push('\\');
        }
        s.push(c);
    }
    s.push('"');
    s
}

#[cfg(not(target_os = "macos"))]
fn entry_for(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Dictate\nComment=Offline push-to-talk dictation\nExec={}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        exec_quote(exe)
    )
}

pub fn is_enabled(exe: &Path) -> bool {
    entry_path().and_then(|p| std::fs::read_to_string(p).ok()).is_some_and(|s| s == entry_for(exe))
}

pub fn set(enabled: bool, exe: &Path) -> Result<(), String> {
    let p = entry_path().ok_or("autostart: HOME is not set")?;
    let err = |e: std::io::Error| format!("autostart: {}: {e}", p.display());
    if enabled {
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(err)?;
        }
        std::fs::write(&p, entry_for(exe)).map_err(err)
    } else {
        match std::fs::remove_file(&p) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(err(e)),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries() {
        assert_eq!(exec_quote(Path::new("/opt/my app/dic$tate")), r#""/opt/my app/dic\$tate""#);
        assert_eq!(
            bundle_of(Path::new("/Applications/Dictate.app/Contents/MacOS/dictate")),
            Some(PathBuf::from("/Applications/Dictate.app"))
        );
        assert_eq!(bundle_of(Path::new("/home/u/dictate/dictate")), None);
        let e = entry_for(Path::new("/opt/dictate/dictate"));
        assert!(e.contains("/opt/dictate/dictate"));
    }

    /// Writes the real autostart entry, so it is opt-in: `cargo test autostart -- --ignored`.
    #[test]
    #[ignore]
    fn roundtrip() {
        let exe = Path::new("/does-not-exist/dictate");
        set(true, exe).unwrap();
        assert!(is_enabled(exe));
        assert!(!is_enabled(Path::new("/other/dictate")));
        set(false, exe).unwrap();
        assert!(!is_enabled(exe));
        set(false, exe).unwrap();
    }
}
