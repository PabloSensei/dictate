//! The platform-agnostic core of the tray app: shared state, the hotkey thread
//! (polls one key, records) and the engine thread (loads the model, transcribes,
//! types). The UI of each platform registers a poster with `set_poster` and
//! reacts to `UiEvent`s on its own main thread.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use dictate::audio::{self, Recorder};
use dictate::config::{self, Config};
use dictate::engine::Engine;
use dictate::history::{self, Entry, History};
use dictate::i18n::{self, T, t};
use dictate::trigger::{Event, Trigger};
use dictate::{inject, keys, log};

/// Something changed; the UI thread reads the details from `shared()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiEvent {
    /// Recording / busy / ready / error state: update the tray and the bubble.
    Refresh,
    /// A new error to show (balloon, notification).
    ShowError,
    /// New live-preview caption text.
    Preview,
    /// The history has a new entry.
    History,
    /// `take_clipboard_text()` has text to put on the clipboard.
    CopyText,
}

/// Set on the new process when restarting: the session history comes on stdin.
const HANDOFF_ENV: &str = "DICTATE_HISTORY_HANDOFF";

/// True while the settings window records a new hotkey: the hotkey thread ignores keys.
pub static HOTKEY_PAUSED: AtomicBool = AtomicBool::new(false);

pub struct Shared {
    pub cfg: Config,
    pub cfg_path: PathBuf,
    pub ready: AtomicBool,
    /// Hotkey held past min_hold: a real dictation, not a Ctrl+C.
    pub recording: AtomicBool,
    pub pending: AtomicUsize,
    pub error: Mutex<Option<String>>,
    pub last: Mutex<String>,
    /// Live preview text for the bubble caption.
    pub preview: Mutex<String>,
    /// Interface language setting ("auto" or a code), for the tray menu checkmark.
    pub ui_lang: Mutex<String>,
    pub history: History,
    /// Text waiting to be put on the clipboard by the main thread.
    to_clipboard: Mutex<Option<String>>,
}

static SHARED: OnceLock<Shared> = OnceLock::new();
static POSTER: OnceLock<fn(UiEvent)> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    Loading = 0,
    Idle = 1,
    Recording = 2,
    Busy = 3,
    Error = 4,
}

pub fn shared() -> &'static Shared {
    SHARED.get().expect("shared state")
}

pub fn history() -> &'static History {
    &shared().history
}

/// The UI registers how events reach its main thread. Must be thread-safe.
pub fn set_poster(f: fn(UiEvent)) {
    let _ = POSTER.set(f);
}

pub fn post(ev: UiEvent) {
    if let Some(f) = POSTER.get() {
        f(ev);
    }
}

pub fn set_error(msg: String) {
    log::write(&format!("error: {}", msg.replace('\n', " | ")));
    *shared().error.lock().unwrap() = Some(msg);
    post(UiEvent::ShowError);
    post(UiEvent::Refresh);
}

pub fn set_preview(text: String) {
    *shared().preview.lock().unwrap() = text;
    post(UiEvent::Preview);
}

pub fn take_clipboard_text() -> Option<String> {
    shared().to_clipboard.lock().unwrap().take()
}

/// After a restart: the previous process's session history, piped on stdin.
/// Call first thing in `main`.
pub fn take_handoff() -> Vec<Entry> {
    if std::env::var_os(HANDOFF_ENV).is_none() {
        return Vec::new();
    }
    let mut s = String::new();
    let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut s);
    history::decode_all(&s)
}

/// Sets up the shared state. The caller has loaded the config and set the UI language.
pub fn init(cfg: Config, cfg_path: PathBuf, carried: Vec<Entry>) {
    let ui_lang = cfg.ui_language.clone();
    let history = History::new(cfg.history, log::data_dir().map(|d| d.join("history.txt")));
    history.extend(carried);
    log::write(&format!(
        "start: v{} {} model={} hotkey={} beam={}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        cfg.model.display(),
        cfg.hotkey_name,
        cfg.beam_size
    ));
    let _ = SHARED.set(Shared {
        cfg,
        cfg_path,
        ready: AtomicBool::new(false),
        recording: AtomicBool::new(false),
        pending: AtomicUsize::new(0),
        error: Mutex::new(None),
        last: Mutex::new(String::new()),
        preview: Mutex::new(String::new()),
        ui_lang: Mutex::new(ui_lang),
        history,
        to_clipboard: Mutex::new(None),
    });
}

/// Starts the engine and hotkey threads. Call after `init` and `set_poster`.
pub fn start_threads() {
    let (tx, rx) = mpsc::channel::<Vec<f32>>();
    let _ = thread::Builder::new().name("engine".into()).spawn(move || engine_thread(rx));
    let _ = thread::Builder::new().name("hotkey".into()).spawn(move || hotkey_thread(tx));
}

/// Interface language picked from the tray menu: "auto" or a code. Saved to dictate.ini.
/// The caller refreshes its menu.
pub fn set_ui_language(code: &str) {
    let sh = shared();
    i18n::set(i18n::resolve(code));
    *sh.ui_lang.lock().unwrap() = code.to_string();
    if let Err(e) = config::save(&sh.cfg_path, &[("ui_language", code.to_string())]) {
        set_error(e);
    }
}

/// Starts a new copy of the app and hands it the session history through a pipe
/// (never through the disk). The caller releases its single-instance lock first
/// and quits right after.
pub fn spawn_successor() {
    let Ok(exe) = std::env::current_exe() else { return };
    let sh = shared();
    let carry = if sh.history.mode() == history::Mode::Session { sh.history.entries() } else { Vec::new() };
    if carry.is_empty() {
        let _ = std::process::Command::new(exe).spawn();
        return;
    }
    let child = std::process::Command::new(exe).env(HANDOFF_ENV, "1").stdin(std::process::Stdio::piped()).spawn();
    if let Ok(mut c) = child
        && let Some(mut stdin) = c.stdin.take()
    {
        let _ = std::io::Write::write_all(&mut stdin, history::encode_all(&carry).as_bytes());
    }
}

/// Icon state and a short status line (without the app name).
pub fn current_look() -> (Look, String) {
    let sh = shared();
    let key = config::key_label(sh.cfg.hotkey);
    if sh.recording.load(Ordering::Acquire) {
        return (Look::Recording, t(T::StListening).into());
    }
    if sh.pending.load(Ordering::Acquire) > 0 {
        return (Look::Busy, t(T::StBusy).into());
    }
    if let Some(e) = sh.error.lock().unwrap().as_ref()
        && !sh.ready.load(Ordering::Acquire)
    {
        return (Look::Error, format!("{}: {}", t(T::StError), e.lines().last().unwrap_or("")));
    }
    if !sh.ready.load(Ordering::Acquire) {
        return (Look::Loading, t(T::StLoading).into());
    }
    let idle = if sh.cfg.mode == config::Mode::Toggle { T::StIdleToggle } else { T::StIdleHold };
    (Look::Idle, i18n::fill(t(idle), &[("key", &key)]))
}

/// What the bubble should show right now.
pub fn bubble_phase() -> Option<dictate::bubble::Phase> {
    let sh = shared();
    if sh.recording.load(Ordering::Acquire) {
        Some(dictate::bubble::Phase::Recording)
    } else if sh.pending.load(Ordering::Acquire) > 0 {
        Some(dictate::bubble::Phase::Busy)
    } else {
        None
    }
}

fn engine_thread(rx: Receiver<Vec<f32>>) {
    let sh = shared();
    let t0 = Instant::now();
    #[cfg(unix)]
    if let Some(hint) = crate::unix::missing_model_hint(&sh.cfg.model) {
        set_error(hint);
        return;
    }
    let mut engine = match Engine::load(&sh.cfg) {
        Ok(e) => e,
        Err(e) => {
            set_error(format!("{}:\n{e}", t(T::ModelFailed)));
            return;
        }
    };
    log::write(&format!(
        "model loaded in {:.1}s, {} threads",
        t0.elapsed().as_secs_f32(),
        dictate::engine::usable_threads(sh.cfg.threads)
    ));
    sh.ready.store(true, Ordering::Release);
    post(UiEvent::Refresh);

    let preview_on = sh.cfg.live_preview && sh.cfg.overlay != config::Overlay::Off;
    let mut last_total = 0usize;
    let mut last_at = Instant::now();
    loop {
        let pcm = match rx.recv_timeout(Duration::from_millis(40)) {
            Ok(pcm) => pcm,
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                // Live preview: re-recognize the tail of the recording every ~0.4 s of new audio.
                if !preview_on || !sh.recording.load(Ordering::Acquire) {
                    last_total = 0;
                    continue;
                }
                let total = audio::live_total();
                let sr = audio::SAMPLE_RATE as usize;
                if total < sr / 2 || total < last_total + sr * 2 / 5 || last_at.elapsed() < Duration::from_millis(300) {
                    continue;
                }
                let (snap, total) = audio::live_snapshot();
                // The caption shows ~3 lines, about 12 s of speech; a shorter tail keeps updates ~0.3 s.
                let tail = &snap[snap.len().saturating_sub(sr * 12)..];
                last_total = total;
                last_at = Instant::now();
                if let Ok(Some(text)) = engine.preview(tail)
                    && sh.recording.load(Ordering::Acquire)
                {
                    set_preview(if total > tail.len() { format!("…{text}") } else { text });
                }
                continue;
            }
        };
        post(UiEvent::Refresh);
        let res = engine.transcribe(&pcm);
        let status = match res {
            Ok(Some(t)) => {
                let mut text = t.text;
                set_preview(text.clone()); // the bubble fades out showing exactly what was typed
                sh.history.add(&text);
                post(UiEvent::History);
                if sh.cfg.clipboard {
                    *sh.to_clipboard.lock().unwrap() = Some(text.clone());
                    post(UiEvent::CopyText);
                }
                if sh.cfg.append_space {
                    text.push(' ');
                }
                let typed = inject::type_text(&text);
                let s = i18n::fill(
                    i18n::t(T::LastStat),
                    &[
                        ("s", &format!("{:.1}", t.audio_s)),
                        ("ms", &t.infer_ms.to_string()),
                        ("n", &text.chars().count().to_string()),
                    ],
                );
                log::write(&format!("dictation audio={:.1}s infer={}ms chars={}", t.audio_s, t.infer_ms, text.chars().count()));
                match typed {
                    Ok(()) => *sh.error.lock().unwrap() = None,
                    Err(e) => set_error(e),
                }
                s
            }
            Ok(None) => {
                log::write(&format!("no speech ({:.1}s)", pcm.len() as f32 / 16000.0));
                i18n::t(T::NoSpeech).to_string()
            }
            Err(e) => {
                set_error(e.clone());
                e
            }
        };
        *sh.last.lock().unwrap() = status;
        sh.pending.fetch_sub(1, Ordering::AcqRel);
        post(UiEvent::Refresh);
    }
}

fn hotkey_thread(tx: Sender<Vec<f32>>) {
    let sh = shared();
    let vk = sh.cfg.hotkey;
    if !keys::supported(vk) {
        set_error(format!("hotkey {} is not supported on this system; pick another one in dictate.ini", sh.cfg.hotkey_name));
    }
    let mut trig = Trigger::new(
        sh.cfg.mode,
        Duration::from_millis(sh.cfg.min_hold_ms),
        Duration::from_secs(sh.cfg.max_record_s.max(1) as u64),
    );
    let mut rec: Option<Recorder> = None;
    let is_alt = matches!(vk, 0xA4 | 0xA5);
    let mut was_down = false;
    loop {
        thread::sleep(Duration::from_millis(15));
        let paused = HOTKEY_PAUSED.load(Ordering::Acquire);
        let down = !paused && keys::down(vk);
        if is_alt && down && !was_down {
            inject::mask_alt_menu(); // releasing Alt must not open the app's menu bar
        }
        was_down = down;
        // Scanning every key costs ~250 cheap calls; only needed while it matters.
        let other = down && keys::other_down(vk);
        let esc = trig.recording() && keys::down(keys::VK_ESCAPE);
        for ev in trig.tick(Instant::now(), down, other, esc) {
            match ev {
                Event::Start => match Recorder::start(sh.cfg.max_record_s) {
                    Ok(r) => rec = Some(r),
                    Err(e) => {
                        trig.abort();
                        set_error(e);
                    }
                },
                Event::Visible => {
                    if rec.is_some() {
                        set_preview(String::new());
                        sh.recording.store(true, Ordering::Release);
                        post(UiEvent::Refresh);
                    }
                }
                Event::Finish | Event::Cancel => {
                    let Some(r) = rec.take() else { continue };
                    sh.recording.store(false, Ordering::Release);
                    match r.stop() {
                        Ok(pcm) if ev == Event::Finish => {
                            sh.pending.fetch_add(1, Ordering::AcqRel);
                            if tx.send(pcm).is_err() {
                                sh.pending.fetch_sub(1, Ordering::AcqRel);
                            }
                        }
                        Ok(_) => {} // cancelled: short tap, shortcut, or Esc
                        Err(e) => set_error(e),
                    }
                    post(UiEvent::Refresh);
                }
            }
        }
    }
}
