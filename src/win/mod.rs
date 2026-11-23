//! Windows UI: tray icon and menu, the bubble overlay, the settings window, and the
//! message loop. Core events arrive as window messages.

pub mod overlay;
pub mod settings;

use std::ffi::c_void;
use std::path::Path;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::OnceLock;

use crate::app::{self, UiEvent, shared};
use dictate::config;
use dictate::i18n::{self, T, t};
use dictate::{autostart, clipboard, log, wide};

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_ERROR, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
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
const WM_UPDATE_FOUND: u32 = WM_APP + 8;
const WM_NOTE: u32 = WM_APP + 9;
const WM_QUIT_UPDATE: u32 = WM_APP + 10;
/// NOTIFYICON_VERSION_4: the user clicked the balloon.
const NIN_BALLOONUSERCLICK: u32 = 0x0400 + 5;

const ID_SETTINGS: usize = 10;
const ID_LOG: usize = 11;
const ID_RESTART: usize = 12;
const ID_QUIT: usize = 13;
const ID_AUTOSTART: usize = 14;
const ID_HISTORY: usize = 15;
const ID_UPDATE: usize = 16;
const ID_LANG_BASE: usize = 30; // 30 = system default, 31.. = i18n::LANGS

static HWND_MAIN: AtomicPtr<c_void> = AtomicPtr::new(null_mut());
static TASKBAR_CREATED: OnceLock<u32> = OnceLock::new();
static ICONS: OnceLock<[usize; 5]> = OnceLock::new();
static MUTEX: AtomicPtr<c_void> = AtomicPtr::new(null_mut());

fn post(msg: u32) {
    let hwnd = HWND_MAIN.load(Ordering::Acquire);
    if !hwnd.is_null() {
        unsafe { PostMessageW(hwnd, msg, 0, 0) };
    }
}

/// Core events become window messages, handled on the UI thread.
fn post_event(ev: UiEvent) {
    post(match ev {
        UiEvent::Refresh => WM_REFRESH,
        UiEvent::ShowError => WM_SHOW_ERROR,
        UiEvent::Preview => WM_PREVIEW,
        UiEvent::History => WM_HISTORY,
        UiEvent::CopyText => WM_COPY_TEXT,
        UiEvent::UpdateFound => WM_UPDATE_FOUND,
        UiEvent::UpdateState => return, // the menu is built when it opens
        UiEvent::Note => WM_NOTE,
        UiEvent::QuitForUpdate => WM_QUIT_UPDATE,
    });
}

fn message_box(text: &str) {
    let (t, c) = (wide(text), wide(t(T::AppName)));
    unsafe { MessageBoxW(null_mut(), t.as_ptr(), c.as_ptr(), MB_OK | MB_ICONERROR) };
}

fn set_ui_language(code: &str) {
    app::set_ui_language(code);
    tray(NIM_MODIFY);
}

pub fn main() {
    // Restarted after saving settings: take over the previous process's session history.
    let carried = app::take_handoff();
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
    app::init(cfg, cfg_path, carried);
    app::set_poster(post_event);

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

    app::start_threads();
    app::start_update_checks();

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

// ---------------------------------------------------------------- tray ----

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
    let (look, status) = app::current_look();
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
    notify(text, NIIF_ERROR);
}

/// A balloon with the info icon (an update was found, "you have the latest version"...).
fn info_balloon(text: &str) {
    notify(text, NIIF_INFO);
}

fn notify(text: &str, icon: u32) {
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = HWND_MAIN.load(Ordering::Acquire);
    nid.uID = 1;
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = icon;
    copy_w(&mut nid.szInfoTitle, t(T::AppName));
    copy_w(&mut nid.szInfo, text);
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
}

pub fn open_path(p: &Path) {
    let (op, file) = (wide("open"), wide(&p.to_string_lossy()));
    unsafe { ShellExecuteW(null_mut(), op.as_ptr(), file.as_ptr(), null(), null(), SW_SHOWNORMAL) };
}

fn show_menu(hwnd: HWND) {
    let sh = shared();
    let (_, status) = app::current_look();
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
        let (label, enabled) = app::update_menu_label();
        add(MF_STRING | if enabled { 0 } else { MF_GRAYED }, ID_UPDATE, &label);
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
    app::spawn_successor();
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
            } else if ev == NIN_BALLOONUSERCLICK && app::update_available().is_some() {
                app::update_clicked(); // the "version X is available" balloon
            }
            0
        }
        WM_UPDATE_FOUND => {
            if let Some(v) = app::update_available() {
                info_balloon(&i18n::fill(t(T::UpdAvailable), &[("v", &v)]));
            }
            0
        }
        WM_NOTE => {
            if let Some(text) = app::take_note() {
                info_balloon(&text);
            }
            0
        }
        WM_QUIT_UPDATE => {
            // the installer waits for this process to exit, then starts the new version
            unsafe { PostQuitMessage(0) };
            0
        }
        WM_REFRESH => {
            tray(NIM_MODIFY);
            overlay::set_target(match app::bubble_phase() {
                Some(p) => overlay::Target::Show(p),
                None => overlay::Target::Hidden,
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
            if let Some(text) = app::take_clipboard_text()
                && let Err(e) = clipboard::set_text(hwnd, &text)
            {
                app::set_error(e);
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
                ID_UPDATE => app::update_clicked(),
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

/// Draws the tray icons (see `dictate::icon`), sized for the current DPI.
fn make_icons() -> [usize; 5] {
    let s = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 64);
    dictate::icon::LOOK_COLORS.map(|c| make_icon(s, c) as usize)
}

fn make_icon(s: i32, color: (u8, u8, u8)) -> HICON {
    let px = dictate::icon::tray_pixels(s, color);
    let n = s as usize;
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
