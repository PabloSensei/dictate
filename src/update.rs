//! Update check and one-click update.
//!
//! dictate has no networking code of its own: it runs the system's curl (built into
//! Windows 10+, macOS and Linux) to ask GitHub for the list of releases and to download
//! the archive of this build. Only that request leaves the computer, never audio or text,
//! and `check_updates = false` turns it off.
//!
//! Installing: the archive is checked against its .sha256 file, unpacked next to the app,
//! and a small script waits for the app to quit, copies the new files over the old ones
//! (dictate.ini and models stay), and starts the new version.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use sha2::{Digest, Sha256};

pub const REPO: &str = "PabloSensei/dictate";

/// This build's version. `DICTATE_FAKE_VERSION` pretends to be another one, to test
/// updating from an older release.
pub fn current_version() -> String {
    std::env::var("DICTATE_FAKE_VERSION").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    /// Without the leading "v".
    pub version: String,
    pub prerelease: bool,
    pub page: String,
    /// This build's archive: (name, url, size in bytes).
    pub asset: Option<(String, String, u64)>,
    pub sha_url: Option<String>,
}

/// "1.2.3" / "1.2.3-beta.4" as comparable parts. A release sorts after its pre-releases.
fn version_key(v: &str) -> Option<(u64, u64, u64, bool, String, u64)> {
    let v = v.trim().trim_start_matches('v');
    let (core, pre) = match v.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (v, None),
    };
    let mut nums = core.split('.').map(|n| n.parse::<u64>().ok());
    let (a, b, c) = (nums.next()??, nums.next()??, nums.next().flatten().unwrap_or(0));
    let (label, n) = match pre {
        None => (String::new(), 0),
        Some(p) => match p.rsplit_once('.') {
            Some((l, n)) if n.parse::<u64>().is_ok() => (l.to_string(), n.parse().unwrap_or(0)),
            _ => (p.to_string(), 0),
        },
    };
    Some((a, b, c, pre.is_none(), label, n))
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (version_key(candidate), version_key(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

pub fn is_prerelease(v: &str) -> bool {
    v.contains('-')
}

/// The end of the archive name for this very build.
pub fn asset_suffix() -> &'static str {
    if cfg!(windows) {
        if cfg!(feature = "cuda") { "-windows-x64-cuda.zip" } else { "-windows-x64-cpu.zip" }
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") { "-macos-arm64.zip" } else { "-macos-x86_64.zip" }
    } else {
        "-linux-x86_64.tar.gz"
    }
}

fn curl() -> Command {
    let mut c = Command::new(if cfg!(windows) { "curl.exe" } else { "curl" });
    c.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

/// Asks GitHub for the newest release this copy can update to. Pre-releases count only
/// when this copy is a pre-release itself. `None` = up to date.
pub fn check(current: &str) -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases?per_page=20");
    let out = curl()
        .args(["-fsSL", "--max-time", "20", "-H", "Accept: application/vnd.github+json", "-A", "dictate-update-check"])
        .arg(&url)
        .output()
        .map_err(|e| format!("update check: can't run curl: {e}"))?;
    if !out.status.success() {
        return Err(format!("update check failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let releases = parse_releases(&String::from_utf8_lossy(&out.stdout))?;
    Ok(pick(releases, current))
}

pub fn parse_releases(json: &str) -> Result<Vec<Release>, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("update check: {e}"))?;
    let arr = v.as_array().ok_or("update check: unexpected answer from GitHub")?;
    let suffix = asset_suffix();
    Ok(arr
        .iter()
        .filter(|r| !r["draft"].as_bool().unwrap_or(false))
        .filter_map(|r| {
            let tag = r["tag_name"].as_str()?.to_string();
            let assets = r["assets"].as_array().cloned().unwrap_or_default();
            let find = |pred: &dyn Fn(&str) -> bool| {
                assets.iter().find(|a| a["name"].as_str().is_some_and(pred)).cloned()
            };
            let asset = find(&|n: &str| n.ends_with(suffix));
            let asset = asset.and_then(|a| {
                Some((a["name"].as_str()?.to_string(), a["browser_download_url"].as_str()?.to_string(), a["size"].as_u64()?))
            });
            let sha_url = asset.as_ref().and_then(|(name, _, _)| {
                let want = format!("{name}.sha256");
                find(&|n: &str| n == want).and_then(|a| a["browser_download_url"].as_str().map(str::to_string))
            });
            Some(Release {
                version: tag.trim_start_matches('v').to_string(),
                tag,
                prerelease: r["prerelease"].as_bool().unwrap_or(false),
                page: r["html_url"].as_str().unwrap_or_default().to_string(),
                asset,
                sha_url,
            })
        })
        .collect())
}

/// The newest release that is newer than `current`, has this build's archive, and is a
/// stable release (or any release, if `current` is a pre-release).
pub fn pick(releases: Vec<Release>, current: &str) -> Option<Release> {
    let allow_pre = is_prerelease(current);
    releases
        .into_iter()
        .filter(|r| (allow_pre || !r.prerelease) && r.asset.is_some() && r.sha_url.is_some())
        .filter(|r| is_newer(&r.version, current))
        .max_by(|a, b| version_key(&a.version).cmp(&version_key(&b.version)))
}

/// Where updates are unpacked before they replace the app.
fn work_dir() -> PathBuf {
    std::env::temp_dir().join("dictate-update")
}

/// Downloads this build's archive and checks its SHA-256. `progress` gets 0..=100.
pub fn download(rel: &Release, progress: &dyn Fn(u32)) -> Result<PathBuf, String> {
    let (name, url, size) = rel.asset.clone().ok_or("no archive for this system in the release")?;
    let sha_url = rel.sha_url.clone().ok_or("the release has no checksum")?;
    let dir = work_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let file = dir.join(&name);

    let expected = {
        let out = curl().args(["-fsSL", "--max-time", "30"]).arg(&sha_url).output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err("can't download the checksum".into());
        }
        String::from_utf8_lossy(&out.stdout).split_whitespace().next().unwrap_or_default().to_ascii_lowercase()
    };
    let mut child = curl()
        .args(["-fsSL", "--retry", "3", "-o"])
        .arg(&file)
        .arg(&url)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("can't run curl: {e}"))?;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    let mut err = String::new();
                    if let Some(mut s) = child.stderr.take() {
                        let _ = std::io::Read::read_to_string(&mut s, &mut err);
                    }
                    return Err(format!("download failed: {}", err.trim()));
                }
                break;
            }
            Ok(None) => {
                let got = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
                progress(((got * 100) / size.max(1)).min(99) as u32);
                std::thread::sleep(std::time::Duration::from_millis(400));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    progress(100);
    let got = sha256_file(&file)?;
    if got != expected {
        let _ = std::fs::remove_file(&file);
        return Err(format!("the download is damaged (SHA-256 {got}, expected {expected})"));
    }
    Ok(file)
}

fn sha256_file(p: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = std::io::Read::read(&mut f, &mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Unpacks the archive into the work folder; returns the folder with the new files
/// (the archive's top folder, or Dictate.app's parent on macOS).
fn unpack(archive: &Path) -> Result<PathBuf, String> {
    let dest = work_dir().join("new");
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    let status = if cfg!(target_os = "macos") {
        Command::new("ditto").arg("-x").arg("-k").arg(archive).arg(&dest).status()
    } else {
        // bsdtar on Windows 10+ reads zip files; GNU tar on Linux the .tar.gz
        let mut c = Command::new(if cfg!(windows) { "tar.exe" } else { "tar" });
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000);
        }
        c.arg("-xf").arg(archive).arg("-C").arg(&dest).status()
    };
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => return Err(format!("unpacking failed ({s})")),
        Err(e) => return Err(format!("unpacking failed: {e}")),
    }
    if cfg!(target_os = "macos") {
        return Ok(dest);
    }
    let mut dirs = std::fs::read_dir(&dest).map_err(|e| e.to_string())?.filter_map(Result::ok).filter(|e| e.path().is_dir());
    match (dirs.next(), dirs.next()) {
        (Some(d), None) => Ok(d.path()),
        _ => Err("unexpected archive layout".into()),
    }
}

/// Where this copy is installed: the exe's folder, or the .app bundle on macOS.
fn install_target() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // follow symlinks on Unix; on Windows canonicalize() gives \\?\ paths that cmd can't start
    #[cfg(not(windows))]
    let exe = exe.canonicalize().unwrap_or(exe);
    if cfg!(target_os = "macos") {
        let bundle = exe.ancestors().nth(3).filter(|b| b.extension().is_some_and(|e| e == "app"));
        return bundle.map(Path::to_path_buf).ok_or_else(|| "updates work for Dictate.app only".into());
    }
    exe.parent().map(Path::to_path_buf).ok_or_else(|| "no install folder".into())
}

/// Unpacks the verified archive and starts the script that swaps the files in once
/// this process has exited and then starts the new version. The caller quits right after.
pub fn install(archive: &Path) -> Result<(), String> {
    let new = unpack(archive)?;
    let target = install_target()?;
    let pid = std::process::id();
    let dir = work_dir();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // robocopy: /E with subfolders; dictate.ini is never in the zip, so it stays.
        let script = format!(
            "@echo off\r\n\
             :wait\r\n\
             tasklist /FI \"PID eq {pid}\" /NH >\"{pids}\" 2>nul <nul\r\n\
             findstr /C:\" {pid} \" \"{pids}\" >nul <nul && (ping -n 2 127.0.0.1 >nul <nul & goto wait)\r\n\
             robocopy \"{new}\" \"{target}\" /E /R:5 /W:1 /NP /NJH >\"{log}\"\r\n\
             echo robocopy exit %errorlevel% >>\"{log}\"\r\n\
             start \"\" \"{target}\\dictate.exe\"\r\n\
             rd /s /q \"{new}\"\r\n\
             del \"{archive}\" \"{pids}\"\r\n",
            new = new.display(),
            target = target.display(),
            archive = archive.display(),
            log = dir.join("update.log").display(),
            pids = dir.join("pids.txt").display(),
        );
        let path = dir.join("update.cmd");
        std::fs::write(&path, script).map_err(|e| e.to_string())?;
        // no inherited handles: the script outlives this process
        Command::new("cmd.exe")
            .arg("/c")
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // a hidden console of its own that tasklist/findstr/robocopy share; with
            // DETACHED_PROCESS they had no console and `tasklist | find` never finished
            .creation_flags(0x0800_0000 | 0x0000_0200) // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP
            .spawn()
            .map_err(|e| format!("can't start the updater: {e}"))?;
    }
    #[cfg(not(windows))]
    {
        let q = |p: &Path| format!("'{}'", p.display().to_string().replace('\'', "'\\''"));
        let swap = if cfg!(target_os = "macos") {
            format!(
                "rm -rf {old}; mv {t} {old} && ditto {n}/Dictate.app {t} && rm -rf {old} || mv {old} {t}\nopen {t}\n",
                old = q(&target.with_extension("app.old")),
                t = q(&target),
                n = q(&new),
            )
        } else {
            format!(
                "cp -Rf {n}/. {t}/\nchmod +x {t}/dictate {t}/download-models.sh\ncd {t} && nohup ./dictate >/dev/null 2>&1 &\n",
                n = q(&new),
                t = q(&target),
            )
        };
        let script = format!(
            "#!/bin/sh\nwhile kill -0 {pid} 2>/dev/null; do sleep 0.5; done\n{swap}rm -rf {n} {a}\n",
            n = q(&new),
            a = q(archive),
        );
        let path = dir.join("update.sh");
        std::fs::write(&path, script).map_err(|e| e.to_string())?;
        Command::new("/bin/sh")
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("can't start the updater: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_newer("0.3.0", "0.2.0"));
        assert!(is_newer("v0.3.0", "0.3.0-beta.1"));
        assert!(is_newer("0.3.0-beta.2", "0.3.0-beta.1"));
        assert!(is_newer("0.3.0-beta.10", "0.3.0-beta.9"));
        assert!(!is_newer("0.3.0-beta.1", "0.3.0"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    fn rel(tag: &str, pre: bool, with_asset: bool) -> String {
        let assets = if with_asset {
            let n = format!("dictate-{tag}{}", asset_suffix());
            format!(
                r#"[{{"name":"{n}","browser_download_url":"https://x/{n}","size":5}},
                   {{"name":"{n}.sha256","browser_download_url":"https://x/{n}.sha256","size":1}}]"#
            )
        } else {
            "[]".into()
        };
        format!(r#"{{"tag_name":"{tag}","draft":false,"prerelease":{pre},"html_url":"https://x/{tag}","assets":{assets}}}"#)
    }

    #[test]
    fn picks_the_right_release() {
        let json = format!(
            "[{},{},{},{}]",
            rel("v0.4.0-beta.1", true, true),
            rel("v0.3.1", false, false), // no archive for this system
            rel("v0.3.0", false, true),
            rel("v0.2.0", false, true)
        );
        let all = parse_releases(&json).unwrap();
        assert_eq!(all.len(), 4);
        assert_eq!(pick(all.clone(), "0.2.0").map(|r| r.version), Some("0.3.0".into()));
        assert_eq!(pick(all.clone(), "0.3.0-beta.1").map(|r| r.version), Some("0.4.0-beta.1".into()));
        assert_eq!(pick(all.clone(), "0.3.0"), None);
        let r = pick(all, "0.1.0").unwrap();
        assert!(r.asset.unwrap().1.ends_with(asset_suffix()));
        assert!(r.sha_url.unwrap().ends_with(".sha256"));
    }
}
