//! Dictation history: what was typed and when.
//!
//! `history = session` (the default) keeps it in memory only, and it is gone when the
//! app quits. `history = disk` also appends every entry to a file in the app's data
//! folder, so it survives restarts. `history = off` keeps nothing.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Entries kept in memory (and loaded from disk); older ones drop off.
pub const MAX_ENTRIES: usize = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Off,
    Session,
    Disk,
}

impl Mode {
    pub fn parse(v: &str) -> Option<Self> {
        match v.to_ascii_lowercase().as_str() {
            "off" | "false" | "none" => Some(Self::Off),
            "session" | "memory" | "ram" => Some(Self::Session),
            "disk" | "file" | "on" | "true" => Some(Self::Disk),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Session => "session",
            Self::Disk => "disk",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Unix time, seconds.
    pub at: u64,
    pub text: String,
}

pub struct History {
    mode: Mode,
    file: Option<PathBuf>,
    entries: Mutex<Vec<Entry>>,
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl History {
    /// `file` is used only in `Mode::Disk`; its entries are loaded right away.
    pub fn new(mode: Mode, file: Option<PathBuf>) -> Self {
        let file = if mode == Mode::Disk { file } else { None };
        let mut entries = file.as_deref().map(load).unwrap_or_default();
        trim(&mut entries);
        Self { mode, file, entries: Mutex::new(entries) }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn add(&self, text: &str) {
        let text = text.trim();
        if self.mode == Mode::Off || text.is_empty() {
            return;
        }
        let e = Entry { at: now(), text: text.to_string() };
        if let Some(f) = &self.file {
            append(f, &e);
        }
        let mut v = self.entries.lock().unwrap();
        v.push(e);
        trim(&mut v);
    }

    /// Adds entries carried over from the previous process (restart after saving settings).
    pub fn extend(&self, carried: Vec<Entry>) {
        if self.mode == Mode::Off {
            return;
        }
        let mut v = self.entries.lock().unwrap();
        if self.mode == Mode::Session {
            v.extend(carried);
            v.sort_by_key(|e| e.at);
            trim(&mut v);
        }
        // in Disk mode the file already holds them
    }

    /// Oldest first.
    pub fn entries(&self) -> Vec<Entry> {
        self.entries.lock().unwrap().clone()
    }

    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Forgets everything, including the file.
    pub fn clear(&self) {
        self.entries.lock().unwrap().clear();
        if let Some(f) = &self.file {
            let _ = std::fs::remove_file(f);
        }
    }
}

fn trim(v: &mut Vec<Entry>) {
    if v.len() > MAX_ENTRIES {
        v.drain(..v.len() - MAX_ENTRIES);
    }
}

/// One entry per line: `<unix seconds>\t<text>` with `\\`, `\n`, `\r`, `\t` escaped.
pub fn encode(e: &Entry) -> String {
    let mut s = format!("{}\t", e.at);
    for c in e.text.chars() {
        match c {
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\t' => s.push_str("\\t"),
            c => s.push(c),
        }
    }
    s
}

pub fn decode(line: &str) -> Option<Entry> {
    let (at, raw) = line.split_once('\t')?;
    let at = at.trim().parse().ok()?;
    let mut text = String::with_capacity(raw.len());
    let mut it = raw.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            text.push(c);
            continue;
        }
        match it.next() {
            Some('n') => text.push('\n'),
            Some('r') => text.push('\r'),
            Some('t') => text.push('\t'),
            Some(o) => text.push(o),
            None => {}
        }
    }
    (!text.is_empty()).then_some(Entry { at, text })
}

pub fn encode_all(v: &[Entry]) -> String {
    v.iter().map(|e| encode(e) + "\n").collect()
}

pub fn decode_all(s: &str) -> Vec<Entry> {
    s.lines().filter_map(decode).collect()
}

fn load(path: &Path) -> Vec<Entry> {
    let v = std::fs::read_to_string(path).map(|s| decode_all(&s)).unwrap_or_default();
    // keep the file from growing forever: rewrite it once it is well past the cap
    if v.len() > MAX_ENTRIES * 2 {
        let keep = &v[v.len() - MAX_ENTRIES..];
        let _ = std::fs::write(path, encode_all(keep));
        return keep.to_vec();
    }
    v
}

fn append(path: &Path, e: &Entry) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{}", encode(e));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_roundtrip() {
        for text in ["plain", "tab\there", "two\nlines\r\n", "back\\slash \\n literal", "Привет, мир"] {
            let e = Entry { at: 1_700_000_000, text: text.to_string() };
            assert_eq!(decode(&encode(&e)), Some(e.clone()), "{text:?}");
            assert!(!encode(&e).contains('\n'));
        }
        assert_eq!(decode("garbage"), None);
        assert_eq!(decode_all("1\ta\n\nx\ty\n2\tb\n").len(), 2);
    }

    #[test]
    fn modes_and_disk() {
        let dir = std::env::temp_dir().join(format!("dictate-hist-test-{}", std::process::id()));
        let file = dir.join("history.txt");
        let h = History::new(Mode::Disk, Some(file.clone()));
        h.add("first");
        h.add("  ");
        h.add("second\nline");
        assert_eq!(h.len(), 2);
        let again = History::new(Mode::Disk, Some(file.clone()));
        assert_eq!(again.entries().iter().map(|e| e.text.as_str()).collect::<Vec<_>>(), ["first", "second\nline"]);
        again.clear();
        assert!(!file.exists() && again.is_empty());

        let off = History::new(Mode::Off, Some(file.clone()));
        off.add("nothing");
        assert!(off.is_empty() && !file.exists());

        let s = History::new(Mode::Session, Some(file.clone()));
        s.add("in memory");
        s.extend(vec![Entry { at: 1, text: "carried".into() }]);
        assert_eq!(s.entries()[0].text, "carried");
        assert!(!file.exists());
        let _ = std::fs::remove_dir_all(&dir);
        for m in [Mode::Off, Mode::Session, Mode::Disk] {
            assert_eq!(Mode::parse(m.as_str()), Some(m));
        }
    }
}
