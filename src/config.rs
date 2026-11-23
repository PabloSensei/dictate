//! `dictate.ini`: plain `key = value` lines, `#` comments. Next to the executable on
//! Windows (portable), in the app's data folder on macOS and Linux.

use std::path::{Path, PathBuf};

use crate::replace::Rule;

pub const TEMPLATE: &str = r#"# dictate: offline push-to-talk dictation. Restart the app after editing
# (the settings window does that for you). Relative paths are relative to this file.

# Whisper model in whisper.cpp GGML format. Its SHA-256 is checked on every start;
# leave model_sha256 empty to skip the check (e.g. for your own fine-tune).
# download-models.cmd fetches it from Hugging Face (ggerganov/whisper.cpp).
model = models\ggml-large-v3-turbo.bin
model_sha256 = 1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69

# Voice activity detector (Silero, ggml-org/whisper-vad): skips noise-only recordings
# and trims silence, which stops Whisper from inventing text. Empty = off.
vad_model = models\ggml-silero-v6.2.0.bin
vad_model_sha256 = 2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987

# Spoken language: a code such as en, ru, de, pl, uk... or auto. Setting it is more
# reliable than auto, which can misdetect short phrases.
language = auto

# Style hint for the model: it copies the punctuation and spelling of this text.
# List names and terms you say often. Empty = no hint. Examples:
#   prompt = Hi! Meeting notes: Kubernetes, PostgreSQL, pull request, Grafana.
#   prompt = Привет! Текст по-русски, термины по-английски: Docker, GitHub, pull request.
prompt =

# Autocorrect, one rule per line: what the model writes => what you want.
# Ignores case (and ё/е), matches whole words only, a space also matches a hyphen.
# replace = cube control => kubectl
# replace = графана => Grafana

# Push-to-talk key: RCtrl, LCtrl, RAlt, RShift, LShift, Apps, F1..F24, Pause, ScrollLock,
# Insert, XButton1 / XButton2 (mouse side buttons), MButton, or a virtual-key code like 0xA3.
hotkey = RCtrl
# hold: hold the key while speaking. toggle: tap to start, tap again to finish (Esc cancels).
# Presses combined with other keys (RCtrl+C...) never start or finish a dictation.
mode = hold
# In hold mode, shorter presses are ignored.
min_hold_ms = 300
max_record_s = 120

# Microphone: empty = the system default, or one microphone by its name as the settings
# window and the tray menu list it. When that one isn't connected, the default is used.
microphone =

# 1 = greedy (fastest), 5 = beam search (a bit more accurate).
beam_size = 5
use_gpu = true
threads = 4

# Add a space after each inserted phrase.
append_space = true
# Also copy each dictation to the clipboard (it is typed either way). Not synced to
# the cloud clipboard.
clipboard = false
# Dictation history (tray menu > History): session = in memory until the app quits,
# disk = also saved to %LOCALAPPDATA%\dictate\history.txt, off = not kept.
history = session

# Animated bubble while dictating, on the monitor with the mouse: bottom | center | off.
overlay = bottom
# Show the text being recognized next to the bubble while you speak (uses the GPU while recording).
live_preview = true

# Interface language: auto (as in Windows) | en | ru | pl | uk.
ui_language = auto
"#;

/// Model and threads that suit a CPU-only build: large-v3-turbo takes ~12 s per phrase
/// on an 8-core CPU, small takes 2-5 s.
const CPU_DEFAULTS: [(&str, &str); 3] = [
    ("model = models\\ggml-large-v3-turbo.bin", "model = models\\ggml-small.bin"),
    (
        "model_sha256 = 1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        "model_sha256 = 1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
    ),
    ("threads = 4", "threads = 8"),
];

/// The template speaks Windows; these lines change on macOS and Linux.
#[cfg(not(windows))]
const UNIX_EDITS: &[(&str, &str)] = &[
    ("models\\", "models/"),
    ("(the settings window does that for you)", "(tray menu > Restart)"),
    ("download-models.cmd fetches it", "download-models.sh fetches it"),
    ("disk = also saved to %LOCALAPPDATA%\\dictate\\history.txt", "disk = also saved to history.txt next to this file"),
    ("auto (as in Windows)", "auto (as in the system)"),
    (
        "# Push-to-talk key: RCtrl, LCtrl, RAlt, RShift, LShift, Apps, F1..F24, Pause, ScrollLock,\n\
         # Insert, XButton1 / XButton2 (mouse side buttons), MButton, or a virtual-key code like 0xA3.\n\
         hotkey = RCtrl",
        HOTKEY_LINES,
    ),
];

#[cfg(target_os = "macos")]
const HOTKEY_LINES: &str = "# Push-to-talk key: ROption, LOption, RCmd, LCmd, RCtrl, LCtrl, RShift, LShift, F1..F20.\n\
     # MacBook keyboards have no right Control key, so Right Option is the default.\n\
     hotkey = ROption";
#[cfg(all(unix, not(target_os = "macos")))]
const HOTKEY_LINES: &str = "# Push-to-talk key: RCtrl, LCtrl, RAlt, RShift, LShift, RSuper, Apps, F1..F24, Pause,\n\
     # ScrollLock, Insert.\n\
     hotkey = RCtrl";

/// Line ending for files people open in a text editor.
pub const NL: &str = if cfg!(windows) { "\r\n" } else { "\n" };

/// The default dictate.ini for this build (the CPU-only build gets a smaller model).
pub fn template() -> String {
    let mut t = TEMPLATE.to_string();
    if !cfg!(feature = "cuda") {
        for (from, to) in CPU_DEFAULTS {
            t = t.replace(from, to);
        }
    }
    #[cfg(not(windows))]
    for (from, to) in UNIX_EDITS {
        t = t.replace(from, to);
    }
    t
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Hold,
    Toggle,
}

impl Mode {
    pub fn parse(v: &str) -> Option<Self> {
        match v.to_ascii_lowercase().as_str() {
            "hold" | "push" | "ptt" => Some(Self::Hold),
            "toggle" | "tap" => Some(Self::Toggle),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hold => "hold",
            Self::Toggle => "toggle",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub model: PathBuf,
    pub model_sha256: Option<String>,
    pub vad_model: Option<PathBuf>,
    pub vad_model_sha256: Option<String>,
    pub language: String,
    pub prompt: String,
    pub replace: Vec<Rule>,
    pub hotkey: i32,
    pub hotkey_name: String,
    /// Microphone name; empty = the system default.
    pub microphone: String,
    pub overlay: Overlay,
    pub mode: Mode,
    pub live_preview: bool,
    /// "auto" or a language code, see `i18n`.
    pub ui_language: String,
    pub min_hold_ms: u64,
    pub max_record_s: u32,
    pub beam_size: i32,
    pub use_gpu: bool,
    pub threads: i32,
    pub append_space: bool,
    /// Copy each dictation to the clipboard too.
    pub clipboard: bool,
    pub history: crate::history::Mode,
}

impl Default for Config {
    fn default() -> Self {
        parse(&template()).expect("template parses")
    }
}

impl Config {
    /// Makes relative model paths relative to `base` (the folder of dictate.ini).
    pub fn resolve_paths(&mut self, base: &Path) {
        let abs = |p: &mut PathBuf| {
            if p.is_relative() && !p.as_os_str().is_empty() {
                *p = base.join(&*p);
            }
        };
        abs(&mut self.model);
        if let Some(v) = self.vad_model.as_mut() {
            abs(v);
        }
    }
}

/// Where the app keeps dictate.ini: next to the exe on Windows, in the data folder elsewhere.
pub fn default_path() -> PathBuf {
    if cfg!(windows) {
        return path_next_to_exe();
    }
    crate::log::data_dir().map(|d| d.join("dictate.ini")).unwrap_or_else(path_next_to_exe)
}

pub fn path_next_to_exe() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("dictate.exe"));
    exe.with_file_name("dictate.ini")
}

/// Loads the config, creating it from the template on first run.
pub fn load_or_create(path: &Path) -> Result<Config, String> {
    if !path.exists() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(path, template().replace('\n', NL))
            .map_err(|e| format!("can't create {}: {e}", path.display()))?;
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut c = parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    c.resolve_paths(path.parent().unwrap_or(Path::new(".")));
    Ok(c)
}

pub fn parse(text: &str) -> Result<Config, String> {
    let mut c = Config {
        model: PathBuf::new(),
        model_sha256: None,
        vad_model: None,
        vad_model_sha256: None,
        language: "auto".into(),
        prompt: String::new(),
        replace: Vec::new(),
        hotkey: 0xA3,
        hotkey_name: "RCtrl".into(),
        microphone: String::new(),
        overlay: Overlay::Bottom,
        mode: Mode::Hold,
        live_preview: true,
        ui_language: "auto".into(),
        min_hold_ms: 300,
        max_record_s: 120,
        beam_size: 5,
        use_gpu: true,
        threads: 4,
        append_space: true,
        clipboard: false,
        history: crate::history::Mode::Session,
    };
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (k, v) = line
            .split_once('=')
            .ok_or_else(|| format!("line {}: expected key = value", n + 1))?;
        let (k, v) = (k.trim(), v.trim());
        let bad = |what: &str| format!("line {}: invalid {what}: {v}", n + 1);
        match k {
            "model" => c.model = PathBuf::from(v),
            "model_sha256" => c.model_sha256 = (!v.is_empty()).then(|| v.to_ascii_lowercase()),
            "vad_model" => c.vad_model = (!v.is_empty()).then(|| PathBuf::from(v)),
            "vad_model_sha256" => c.vad_model_sha256 = (!v.is_empty()).then(|| v.to_ascii_lowercase()),
            "language" => c.language = v.to_string(),
            "microphone" => c.microphone = v.to_string(),
            "prompt" => c.prompt = v.to_string(),
            "replace" => c.replace.push(Rule::parse(v).ok_or_else(|| {
                format!("line {}: expected replace = what the model writes => what you want", n + 1)
            })?),
            "hotkey" => {
                c.hotkey = parse_key(v).ok_or_else(|| bad("hotkey"))?;
                c.hotkey_name = v.to_string();
            }
            "min_hold_ms" => c.min_hold_ms = v.parse().map_err(|_| bad(k))?,
            "max_record_s" => c.max_record_s = v.parse().map_err(|_| bad(k))?,
            "beam_size" => c.beam_size = v.parse().map_err(|_| bad(k))?,
            "use_gpu" => c.use_gpu = parse_bool(v).ok_or_else(|| bad(k))?,
            "threads" => c.threads = v.parse().map_err(|_| bad(k))?,
            "append_space" => c.append_space = parse_bool(v).ok_or_else(|| bad(k))?,
            "clipboard" => c.clipboard = parse_bool(v).ok_or_else(|| bad(k))?,
            "history" => c.history = crate::history::Mode::parse(v).ok_or_else(|| bad(k))?,
            "overlay" => c.overlay = Overlay::parse(v).ok_or_else(|| bad(k))?,
            "mode" => c.mode = Mode::parse(v).ok_or_else(|| bad(k))?,
            "live_preview" => c.live_preview = parse_bool(v).ok_or_else(|| bad(k))?,
            "ui_language" => {
                if v != "auto" && crate::i18n::Lang::from_code(v).is_none() {
                    return Err(bad(k));
                }
                c.ui_language = v.to_ascii_lowercase();
            }
            _ => return Err(format!("line {}: unknown key {k}", n + 1)),
        }
    }
    Ok(c)
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "true" | "yes" | "1" | "on" => Some(true),
        "false" | "no" | "0" | "off" => Some(false),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    Bottom,
    Center,
    Off,
}

impl Overlay {
    pub fn parse(v: &str) -> Option<Self> {
        match v.to_ascii_lowercase().as_str() {
            "bottom" | "on" | "true" => Some(Self::Bottom),
            "center" => Some(Self::Center),
            "off" | "false" => Some(Self::Off),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bottom => "bottom",
            Self::Center => "center",
            Self::Off => "off",
        }
    }
}

/// Keys that make good push-to-talk buttons: (virtual key, ini name).
/// Keys that type characters, open menus on release (Alt, Win) or toggle state
/// (Caps Lock) are left out on purpose. Display names live in `i18n::key_label`.
pub const HOTKEYS: &[(i32, &str)] = &[
    (0xA5, "RAlt"), // first: AltGr also sends LCtrl
    (0xA3, "RCtrl"),
    (0xA2, "LCtrl"),
    (0xA1, "RShift"),
    (0xA0, "LShift"),
    (0x5D, "Apps"),
    (0x13, "Pause"),
    (0x91, "ScrollLock"),
    (0x2D, "Insert"),
    (0x05, "XButton1"),
    (0x06, "XButton2"),
    (0x04, "MButton"),
];

/// Every key the settings window can capture, F1..F24 included.
pub fn capturable_keys() -> Vec<i32> {
    HOTKEYS.iter().map(|k| k.0).chain(0x70..=0x87).collect()
}

pub fn key_name(vk: i32) -> String {
    if (0x70..=0x87).contains(&vk) {
        return format!("F{}", vk - 0x70 + 1);
    }
    HOTKEYS.iter().find(|k| k.0 == vk).map(|k| k.1.to_string()).unwrap_or_else(|| format!("0x{vk:02X}"))
}

/// Localized key name (current interface language).
pub fn key_label(vk: i32) -> String {
    crate::i18n::key_label(vk)
}

pub fn parse_key(v: &str) -> Option<i32> {
    let l = v.to_ascii_lowercase();
    if let Some(hex) = l.strip_prefix("0x") {
        return i32::from_str_radix(hex, 16).ok().filter(|k| (1..=0xFE).contains(k));
    }
    if let Some(n) = l.strip_prefix('f').and_then(|n| n.parse::<i32>().ok()) {
        return (1..=24).contains(&n).then_some(0x70 + n - 1);
    }
    if let Some(k) = HOTKEYS.iter().find(|k| k.1.eq_ignore_ascii_case(v)) {
        return Some(k.0);
    }
    Some(match l.as_str() {
        "rcontrol" => 0xA3,
        "lcontrol" => 0xA2,
        "rmenu" => 0xA5,
        "lalt" | "lmenu" => 0xA4,
        "rwin" | "rsuper" | "rcmd" | "rcommand" => 0x5C,
        "lcmd" | "lcommand" | "lsuper" => 0x5B,
        "roption" | "ropt" => 0xA5,
        "loption" | "lopt" => 0xA4,
        "capslock" => 0x14,
        "menu" => 0x5D,
        _ => return None,
    })
}

/// Rewrites `key = value` lines in place (first uncommented occurrence), keeping
/// comments, order and other keys; missing keys are appended. Values are single-line.
pub fn save(path: &Path, updates: &[(&str, String)]) -> Result<(), String> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|_| template());
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    for (key, value) in updates {
        let value = value.replace(['\r', '\n'], " ");
        let new_line = format!("{key} = {}", value.trim());
        let pos = lines.iter().position(|l| {
            let t = l.trim();
            !t.starts_with('#') && t.split_once('=').is_some_and(|(k, _)| k.trim() == *key)
        });
        match pos {
            Some(i) => lines[i] = new_line,
            None => lines.push(new_line),
        }
    }
    let mut out = lines.join(NL);
    out.push_str(NL);
    parse(&out)?; // never write a config we can't read back
    let tmp = path.with_extension("ini.tmp");
    std::fs::write(&tmp, out).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses() {
        let c = Config::default();
        assert_eq!(c.hotkey, if cfg!(target_os = "macos") { 0xA5 } else { 0xA3 });
        assert_eq!(c.language, "auto");
        assert!(c.model_sha256.is_some());
        assert!(c.replace.is_empty() && c.prompt.is_empty());
        let mut r = c.clone();
        let base = Path::new(if cfg!(windows) { r"C:\apps\dictate" } else { "/home/u/.local/share/dictate" });
        r.resolve_paths(base);
        assert!(r.model.starts_with(base.join("models")) && r.model.extension().is_some_and(|e| e == "bin"));
        assert!(r.vad_model.is_some_and(|v| v.is_absolute()));
    }

    #[test]
    fn platform_template() {
        let t = template();
        let c = parse(&t).unwrap();
        #[cfg(not(windows))]
        {
            for (from, _) in UNIX_EDITS {
                assert_eq!(TEMPLATE.matches(from).count().min(1), 1, "{from}");
            }
            assert!(!t.contains('\\') &&!t.contains("LOCALAPPDATA"), "{t}");
            assert!(crate::keys::supported(c.hotkey));
        }
        assert!(c.model.to_string_lossy().starts_with("models"));
    }

    #[test]
    fn cpu_defaults_apply_to_the_template() {
        for (from, to) in CPU_DEFAULTS {
            assert_eq!(TEMPLATE.matches(from).count(), 1, "{from}");
            let t = TEMPLATE.replace(from, to);
            assert!(parse(&t).is_ok(), "{to}");
        }
    }

    #[test]
    fn save_keeps_comments_and_replaces() {
        let dir = std::env::temp_dir().join(format!("dictate-cfg-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("dictate.ini");
        std::fs::write(&p, TEMPLATE).unwrap();
        save(&p, &[("hotkey", "F9".into()), ("overlay", "center".into()), ("prompt", "a\nb".into())]).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        let c = parse(&text).unwrap();
        assert_eq!(c.hotkey, 0x78);
        assert_eq!(c.overlay, Overlay::Center);
        assert_eq!(c.prompt, "a b");
        assert!(text.contains("# Push-to-talk key"));
        save(&p, &[("replace", "x => y".into())]).unwrap(); // a missing key is appended
        assert_eq!(parse(&std::fs::read_to_string(&p).unwrap()).unwrap().replace.len(), 1);
        assert!(save(&p, &[("hotkey", "nonsense".into())]).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn key_names_roundtrip() {
        for vk in capturable_keys() {
            assert_eq!(parse_key(&key_name(vk)), Some(vk), "vk {vk:#x}");
        }
    }

    #[test]
    fn keys() {
        assert_eq!(parse_key("F9"), Some(0x78));
        assert_eq!(parse_key("0xA5"), Some(0xA5));
        assert_eq!(parse_key("XButton2"), Some(0x06));
        assert_eq!(parse_key("nope"), None);
    }
}
