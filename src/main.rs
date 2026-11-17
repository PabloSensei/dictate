//! Tray app: hold the hotkey, speak, release -> text is typed into the focused window.
//!
//! Threads: main (tray + message loop), hotkey (polls one key, records),
//! engine (loads the model, transcribes, types). No hooks, no network; the clipboard
//! only when `clipboard = true` or you press Copy in the history.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod overlay;
mod settings;

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use dictate::audio::{self, Recorder};
use dictate::bubble::Phase;
use dictate::trigger::{Event, Trigger};
use dictate::config::{self, Config};
use dictate::engine::Engine;
use dictate::history::{self, History};
use dictate::i18n::{self, T, t};
use dictate::{autostart, clipboard, inject, log, wide};

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_ERROR, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NIM_SETVERSION, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, ShellExecuteW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    AllowSetForegroundWindow, ASFW_ANY, DispatchMessageW, FindWindowW, GetCursorPos, GetMessageW, GetSystemMetrics, HICON, ICONINFO, MB_ICONERROR, MB_OK, MF_GRAYED,
    MF_CHECKED, MF_POPUP, MF_SEPARATOR, MF_STRING, MSG, MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassW,
    RegisterWindowMessageW, SM_CXSMICON, SW_SHOWNORMAL, SetForegroundWindow, TPM_BOTTOMALIGN, TPM_RIGHTBUTTON,
    TrackPopupMenu, TranslateMessage, WM_APP, WM_COMMAND, WM_CONTEXTMENU, WM_DESTROY, WM_LBUTTONUP, WM_NULL,
    WM_RBUTTONUP, WNDCLASSW,
};

const WM_TRAY: u32 = WM_APP + 1;
const WM_REFRESH: u32 = WM_APP + 2;
const WM_SHOW_ERROR: u32 = WM_APP + 3;
const WM_OPEN_SETTINGS: u32 = WM_APP + 4;
const WM_PREVIEW: u32 = WM_APP + 5;
const WM_HISTORY: u32 = WM_APP + 6;
const WM_COPY_TEXT: u32 = WM_APP + 7;

/// Set on the new process when restarting: the session history comes on stdin.
const HANDOFF_ENV: &str = "DICTATE_HISTORY_HANDOFF";

const ID_SETTINGS: usize = 10;
const ID_LOG: usize = 11;
const ID_RESTART: usize = 12;
const ID_QUIT: usize = 13;
const ID_AUTOSTART: usize = 14;
const ID_HISTORY: usize = 15;
const ID_LANG_BASE: usize = 30; // 30 = system default, 31.. = i18n::LANGS

struct Shared {
    cfg: Config,
    cfg_path: PathBuf,
    ready: AtomicBool,
    /// Hotkey held past min_hold: a real dictation, not a Ctrl+C.
    recording: AtomicBool,
    pending: AtomicUsize,
    error: Mutex<Option<String>>,
    last: Mutex<String>,
    /// Live preview text for the bubble caption.
    preview: Mutex<String>,
    /// Interface language setting ("auto" or a code), for the tray menu checkmark.
    ui_lang: Mutex<String>,
    history: History,
    /// Text waiting to be put on the clipboard by the main thread.
    to_clipboard: Mutex<Option<String>>,
}

static SHARED: OnceLock<Shared> = OnceLock::new();
static HWND_MAIN: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
static TASKBAR_CREATED: OnceLock<u32> = OnceLock::new();
static ICONS: OnceLock<[usize; 5]> = OnceLock::new();
static MUTEX: AtomicPtr<c_void> = AtomicPtr::new(null_mut());

#[derive(Clone, Copy)]
enum Look {
    Loading = 0,
    Idle = 1,
    Recording = 2,
    Busy = 3,
    Error = 4,
}

fn shared() -> &'static Shared {
    SHARED.get().expect("shared state")
}

pub fn history() -> &'static History {
    &shared().history
}

pub fn main_hwnd() -> HWND {
    HWND_MAIN.load(Ordering::Acquire)
}

fn post(msg: u32) {
    let hwnd = HWND_MAIN.load(Ordering::Acquire);
    if !hwnd.is_null() {
        unsafe { PostMessageW(hwnd, msg, 0, 0) };
    }
}

fn set_error(msg: String) {
    log::write(&format!("error: {}", msg.replace('\n', " | ")));
    *shared().error.lock().unwrap() = Some(msg);
    post(WM_SHOW_ERROR);
    post(WM_REFRESH);
}

fn message_box(text: &str) {
    let (t, c) = (wide(text), wide(t(T::AppName)));
    unsafe { MessageBoxW(null_mut(), t.as_ptr(), c.as_ptr(), MB_OK | MB_ICONERROR) };
}

/// Interface language picked from the tray menu: "auto" or a code. Saved to dictate.ini.
fn set_ui_language(code: &str) {
    let sh = shared();
    i18n::set(i18n::resolve(code));
    *sh.ui_lang.lock().unwrap() = code.to_string();
    if let Err(e) = config::save(&sh.cfg_path, &[("ui_language", code.to_string())]) {
        set_error(e);
    }
    tray(NIM_MODIFY);
}

fn main() {
    // Restarted after saving settings: take over the previous process's session history.
    let carried = if std::env::var_os(HANDOFF_ENV).is_some() {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut s);
        history::decode_all(&s)
    } else {
        Vec::new()
    };
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    i18n::set(i18n::resolve("auto")); // until the config says otherwise

    let name = wide("Local\\dictate-single-instance");
    let m = unsafe { CreateMutexW(null(), 0, name.as_ptr()) };
    if m.is_null() || unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        // Already running: launching it again opens the settings of the running copy
        // (`--page general|recognition|app|history` picks the page).
        let page = std::env::args()
            .skip_while(|a| a != "--page")
            .nth(1)
            .and_then(|p| settings::Page::parse(&p))
            .unwrap_or(settings::Page::General);
        let class = wide("dictate-tray");
        let other = unsafe { FindWindowW(class.as_ptr(), null()) };
        if other.is_null() {
            message_box(t(T::AlreadyRunning));
        } else {
            unsafe {
                AllowSetForegroundWindow(ASFW_ANY);
                PostMessageW(other, WM_OPEN_SETTINGS, page.index(), 0);
            }
        }
        return;
    }
    MUTEX.store(m, Ordering::Release);

    let cfg_path = config::path_next_to_exe();
    let cfg = match config::load_or_create(&cfg_path) {
        Ok(c) => c,
        Err(e) => {
            message_box(&format!("{}:\n{e}", t(T::ConfigError)));
            return;
        }
    };
    i18n::set(i18n::resolve(&cfg.ui_language));
    let ui_lang = cfg.ui_language.clone();
    let history = History::new(cfg.history, log::data_dir().map(|d| d.join("history.txt")));
    history.extend(carried);
    log::write(&format!("start: model={} hotkey={} beam={}", cfg.model.display(), cfg.hotkey_name, cfg.beam_size));
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

    let hwnd = match create_window() {
        Ok(h) => h,
        Err(e) => {
            message_box(&e);
            return;
        }
    };
    HWND_MAIN.store(hwnd, Ordering::Release);
    let _ = ICONS.set(make_icons());
    tray(NIM_ADD);

    let (tx, rx) = mpsc::channel::<Vec<f32>>();
    let _ = thread::Builder::new().name("engine".into()).spawn(move || engine_thread(rx));
    let _ = thread::Builder::new().name("hotkey".into()).spawn(move || hotkey_thread(tx));

    overlay::init(shared().cfg.overlay);

    let mut msg: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) } > 0 {
        if settings::pre_translate(&msg) {
            continue;
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    tray(NIM_DELETE);
    log::write("exit");
    // Worker threads may be inside CUDA calls; exiting the process is the clean way out.
    std::process::exit(0);
}

fn engine_thread(rx: Receiver<Vec<f32>>) {
    let sh = shared();
    let t0 = Instant::now();
    let mut engine = match Engine::load(&sh.cfg) {
        Ok(e) => e,
        Err(e) => {
            set_error(format!("{}:\n{e}", t(T::ModelFailed)));
            return;
        }
    };
    log::write(&format!("model loaded in {:.1}s", t0.elapsed().as_secs_f32()));
    sh.ready.store(true, Ordering::Release);
    post(WM_REFRESH);

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
        post(WM_REFRESH);
        let res = engine.transcribe(&pcm);
        let status = match res {
            Ok(Some(t)) => {
                let mut text = t.text;
                set_preview(text.clone()); // the bubble fades out showing exactly what was typed
                sh.history.add(&text);
                post(WM_HISTORY);
                if sh.cfg.clipboard {
                    *sh.to_clipboard.lock().unwrap() = Some(text.clone());
                    post(WM_COPY_TEXT);
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
        post(WM_REFRESH);
    }
}

fn key_down(vk: i32) -> bool {
    (unsafe { GetAsyncKeyState(vk) } as u16) & 0x8000 != 0
}

/// Is any key other than the hotkey down? Not counted: generic Ctrl/Shift/Alt (they
/// mirror the left/right codes), VK_PACKET and the menu mask key (our own SendInput),
/// and for Right Alt the left Ctrl that AltGr layouts (Polish...) press along with it.
fn other_key_down(hotkey: i32) -> bool {
    let altgr_ctrl = |k: i32| hotkey == 0xA5 && k == 0xA2;
    (0x01..=0xFE).any(|k| {
        k != hotkey
            && !matches!(k, 0x07 | 0x0A | 0x0B | 0x10..=0x12 | 0xE7 | 0xFF)
            && k != inject::MASK_VK as i32
            && !altgr_ctrl(k)
            && key_down(k)
    })
}

fn set_preview(text: String) {
    *shared().preview.lock().unwrap() = text;
    post(WM_PREVIEW);
}

fn hotkey_thread(tx: Sender<Vec<f32>>) {
    let sh = shared();
    let vk = sh.cfg.hotkey;
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
        let paused = settings::HOTKEY_PAUSED.load(Ordering::Acquire);
        let down = !paused && key_down(vk);
        if is_alt && down && !was_down {
            inject::mask_alt_menu(); // releasing Alt must not open the app's menu bar
        }
        was_down = down;
        // Scanning every key costs ~250 cheap calls; only needed while it matters.
        let other = down && other_key_down(vk);
        let esc = trig.recording() && key_down(0x1B);
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
                        post(WM_REFRESH);
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
                    post(WM_REFRESH);
                }
            }
        }
    }
}

// ---------------------------------------------------------------- tray ----

/// Icon state and a short status line (without the app name).
fn current_look() -> (Look, String) {
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

fn copy_w(dst: &mut [u16], s: &str) {
    let w: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..w.len()].copy_from_slice(&w);
    dst[w.len()] = 0;
}

fn tray(op: u32) {
    let hwnd = HWND_MAIN.load(Ordering::Acquire);
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    if op == NIM_DELETE {
        unsafe { Shell_NotifyIconW(NIM_DELETE, &nid) };
        return;
    }
    let (look, status) = current_look();
    let tip = format!("{} — {status}", t(T::AppName));
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = ICONS.get().map(|i| i[look as usize] as HICON).unwrap_or(null_mut());
    copy_w(&mut nid.szTip, &tip);
    unsafe { Shell_NotifyIconW(op, &nid) };
    if op == NIM_ADD {
        nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        unsafe { Shell_NotifyIconW(NIM_SETVERSION, &nid) };
    }
}

fn balloon(text: &str) {
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = HWND_MAIN.load(Ordering::Acquire);
    nid.uID = 1;
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = NIIF_ERROR;
    copy_w(&mut nid.szInfoTitle, t(T::AppName));
    copy_w(&mut nid.szInfo, text);
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
}

fn open_path(p: &Path) {
    let (op, file) = (wide("open"), wide(&p.to_string_lossy()));
    unsafe { ShellExecuteW(null_mut(), op.as_ptr(), file.as_ptr(), null(), null(), SW_SHOWNORMAL) };
}

fn show_menu(hwnd: HWND) {
    let sh = shared();
    let (_, status) = current_look();
    let model = sh.cfg.model.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let last = sh.last.lock().unwrap().clone();
    let err = sh.error.lock().unwrap().clone();
    let ui_lang = sh.ui_lang.lock().unwrap().clone();
    unsafe {
        let menu = CreatePopupMenu();
        let add_to = |m, flags: u32, id: usize, text: &str| {
            let w = wide(text);
            AppendMenuW(m, flags, id, w.as_ptr());
        };
        let add = |flags: u32, id: usize, text: &str| add_to(menu, flags, id, text);
        add(MF_STRING | MF_GRAYED, 0, &status);
        add(MF_STRING | MF_GRAYED, 0, &format!("{}: {model}", t(T::MenuModel)));
        if !last.is_empty() {
            add(MF_STRING | MF_GRAYED, 0, &format!("{}: {last}", t(T::MenuLast)));
        }
        if let Some(e) = err {
            add(MF_STRING | MF_GRAYED, 0, &format!("{}: {}", t(T::StError), e.lines().last().unwrap_or("")));
        }
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        add(MF_STRING, ID_SETTINGS, t(T::MenuSettings));
        add(MF_STRING, ID_HISTORY, t(T::MenuHistory));

        // Interface language submenu: system default + each language in its own name.
        let langs = CreatePopupMenu();
        let check = |code: &str| if ui_lang == code { MF_CHECKED } else { 0 };
        add_to(langs, MF_STRING | check("auto"), ID_LANG_BASE, t(T::MenuSystemLang));
        AppendMenuW(langs, MF_SEPARATOR, 0, null());
        for (i, l) in i18n::LANGS.iter().enumerate() {
            add_to(langs, MF_STRING | check(l.code()), ID_LANG_BASE + 1 + i, l.native_name());
        }
        let w = wide(t(T::MenuUiLang));
        AppendMenuW(menu, MF_POPUP, langs as usize, w.as_ptr());

        let auto_on = std::env::current_exe().is_ok_and(|e| autostart::is_enabled(&e));
        add(MF_STRING | if auto_on { MF_CHECKED } else { 0 }, ID_AUTOSTART, t(T::MenuAutostart));
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        add(MF_STRING, ID_LOG, t(T::MenuLog));
        add(MF_STRING, ID_RESTART, t(T::MenuRestart));
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        add(MF_STRING, ID_QUIT, t(T::MenuQuit));

        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        SetForegroundWindow(hwnd);
        TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, 0, hwnd, null());
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);
    }
}

fn open_settings(page: settings::Page) {
    let sh = shared();
    // Re-read the file so the window shows what is on disk, not what this process started with.
    let cfg = config::load_or_create(&sh.cfg_path).unwrap_or_else(|_| sh.cfg.clone());
    settings::open(&cfg, sh.cfg_path.clone(), restart, page);
}

fn restart() {
    let m = MUTEX.swap(null_mut(), Ordering::AcqRel);
    if !m.is_null() {
        unsafe { CloseHandle(m as HANDLE) };
    }
    if let Ok(exe) = std::env::current_exe() {
        let sh = shared();
        let carry = if sh.history.mode() == history::Mode::Session { sh.history.entries() } else { Vec::new() };
        if carry.is_empty() {
            let _ = std::process::Command::new(exe).spawn();
        } else {
            // In-memory history goes to the new process through a pipe, never to disk.
            let child = std::process::Command::new(exe)
                .env(HANDOFF_ENV, "1")
                .stdin(std::process::Stdio::piped())
                .spawn();
            if let Ok(mut c) = child
                && let Some(mut stdin) = c.stdin.take()
            {
                let _ = std::io::Write::write_all(&mut stdin, history::encode_all(&carry).as_bytes());
            }
        }
    }
    unsafe { PostQuitMessage(0) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY => {
            let ev = (lp as u32) & 0xFFFF;
            if ev == WM_CONTEXTMENU || ev == WM_RBUTTONUP {
                show_menu(hwnd);
            } else if ev == WM_LBUTTONUP {
                open_settings(settings::Page::General);
            }
            0
        }
        WM_REFRESH => {
            tray(NIM_MODIFY);
            let sh = shared();
            overlay::set_target(if sh.recording.load(Ordering::Acquire) {
                overlay::Target::Show(Phase::Recording)
            } else if sh.pending.load(Ordering::Acquire) > 0 {
                overlay::Target::Show(Phase::Busy)
            } else {
                overlay::Target::Hidden
            });
            0
        }
        WM_PREVIEW => {
            let text = shared().preview.lock().unwrap().clone();
            overlay::set_caption(&text);
            0
        }
        WM_OPEN_SETTINGS => {
            open_settings(settings::Page::from_index(wp));
            0
        }
        WM_HISTORY => {
            settings::history_changed();
            0
        }
        WM_COPY_TEXT => {
            if let Some(text) = shared().to_clipboard.lock().unwrap().take()
                && let Err(e) = clipboard::set_text(hwnd, &text)
            {
                set_error(e);
            }
            0
        }
        WM_SHOW_ERROR => {
            if let Some(e) = shared().error.lock().unwrap().clone() {
                balloon(&e);
            }
            0
        }
        WM_COMMAND => {
            match wp & 0xFFFF {
                ID_SETTINGS => open_settings(settings::Page::General),
                ID_HISTORY => open_settings(settings::Page::History),
                ID_LOG => {
                    if let Some(p) = log::path() {
                        open_path(&p);
                    }
                }
                ID_RESTART => restart(),
                ID_AUTOSTART => {
                    if let Ok(exe) = std::env::current_exe() {
                        let on = !autostart::is_enabled(&exe);
                        match autostart::set(on, &exe) {
                            Ok(()) => log::write(&format!("autostart {}", if on { "on" } else { "off" })),
                            Err(e) => balloon(&e),
                        }
                    }
                }
                ID_QUIT => unsafe { PostQuitMessage(0) },
                id if (ID_LANG_BASE..=ID_LANG_BASE + i18n::LANGS.len()).contains(&id) => {
                    let code = if id == ID_LANG_BASE { "auto" } else { i18n::LANGS[id - ID_LANG_BASE - 1].code() };
                    set_ui_language(code);
                }
                _ => {}
            }
            0
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            0
        }
        m if Some(&m) == TASKBAR_CREATED.get() => {
            tray(NIM_ADD); // Explorer restarted: put the icon back
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

fn create_window() -> Result<HWND, String> {
    unsafe {
        let hinst = GetModuleHandleW(null());
        let class = wide("dictate-tray");
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: null_mut(),
            hCursor: null_mut(),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class.as_ptr(),
        };
        if RegisterClassW(&wc) == 0 {
            return Err("RegisterClassW failed".into());
        }
        let _ = TASKBAR_CREATED.set(RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()));
        let title = wide("dictate");
        let hwnd = CreateWindowExW(0, class.as_ptr(), title.as_ptr(), 0, 0, 0, 0, 0, null_mut(), null_mut(), hinst, null());
        if hwnd.is_null() {
            let _ = DestroyWindow(hwnd);
            return Err("CreateWindowExW failed".into());
        }
        Ok(hwnd)
    }
}

/// Draws the tray icons: a microphone glyph in a colored disc, sized for the current DPI.
fn make_icons() -> [usize; 5] {
    let s = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 64);
    let colors: [(u8, u8, u8); 5] = [
        (0xF5, 0xA6, 0x23), // loading: amber
        (0x6E, 0x76, 0x80), // idle: slate
        (0xE5, 0x39, 0x35), // recording: red
        (0x1E, 0x88, 0xE5), // busy: blue
        (0x8E, 0x24, 0xAA), // error: purple
    ];
    colors.map(|c| make_icon(s, c) as usize)
}

fn make_icon(s: i32, (r, g, b): (u8, u8, u8)) -> HICON {
    let n = s as usize;
    let f = s as f32;
    let mut px = vec![0u32; n * n];
    // signed distance helpers in pixel units
    let disc = |x: f32, y: f32| ((x - f / 2.0).hypot(y - f / 2.0)) - f * 0.48;
    let capsule = |x: f32, y: f32| {
        let (cx, w) = (f / 2.0, f * 0.11);
        let (y0, y1) = (f * 0.22, f * 0.50);
        let dy = if y < y0 { y0 - y } else if y > y1 { y - y1 } else { 0.0 };
        (x - cx).hypot(dy) - w
    };
    let cup = |x: f32, y: f32| {
        // U-shaped holder: ring segment below the capsule + stem + base
        let (cx, cy, rad, th) = (f / 2.0, f * 0.47, f * 0.21, f * 0.055);
        let ring = if y >= cy { ((x - cx).hypot(y - cy) - rad).abs() - th } else { f };
        let stem = if (y - f * 0.74).abs() < f * 0.07 { (x - cx).abs() - th } else { f };
        let base = if (x - cx).abs() < f * 0.16 { (y - f * 0.80).abs() - th } else { f };
        ring.min(stem).min(base)
    };
    for yi in 0..n {
        for xi in 0..n {
            let (x, y) = (xi as f32 + 0.5, yi as f32 + 0.5);
            let a_disc = (0.5 - disc(x, y)).clamp(0.0, 1.0);
            let a_mic = (0.5 - capsule(x, y).min(cup(x, y))).clamp(0.0, 1.0);
            let mix = |c: u8| (c as f32 * (1.0 - a_mic) + 255.0 * a_mic) as u32;
            let a = (a_disc * 255.0) as u32;
            px[yi * n + xi] = (a << 24) | (mix(r) << 16) | (mix(g) << 8) | mix(b);
        }
    }
    unsafe {
        let color = CreateBitmap(s, s, 1, 32, px.as_ptr() as *const c_void);
        let mask_row = n.div_ceil(16) * 2;
        let mask = vec![0u8; mask_row * n];
        let mask_bm = CreateBitmap(s, s, 1, 1, mask.as_ptr() as *const c_void);
        let info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask_bm, hbmColor: color };
        let icon = CreateIconIndirect(&info);
        DeleteObject(color);
        DeleteObject(mask_bm);
        icon
    }
}
