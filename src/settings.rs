//! Settings window in a macOS System Settings look: a sidebar with pages, grouped
//! cards with rows, toggle switches, segmented controls, popup buttons, text fields,
//! and the dictation history.
//! Everything is custom-painted into a DIB (shapes with `dictate::paint`, text
//! with GDI); only the prompt and search fields are real (borderless) EDIT controls.
//! Follows the Windows light/dark theme. Saving rewrites dictate.ini and restarts.

use std::cell::RefCell;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use dictate::config::{self, Config, Mode, Overlay};
use dictate::history;
use dictate::i18n::{self, LANGS, Lang, T, t};
use dictate::paint::{Canvas, mix};
use dictate::surface::{Dib, make_dib};
use dictate::{autostart, clipboard, wide};

use windows_sys::Win32::Foundation::{COLORREF, FILETIME, HWND, LPARAM, LRESULT, POINT, RECT, SYSTEMTIME, WPARAM};
use windows_sys::Win32::Globalization::{GetDateFormatEx, GetTimeFormatEx};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CLEARTYPE_QUALITY, ClientToScreen, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET,
    DT_CALCRECT, DT_CENTER, DT_EDITCONTROL, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER,
    DT_WORDBREAK, DeleteObject, DrawTextW, EndPaint, FW_NORMAL, FW_SEMIBOLD, GdiFlush, GetMonitorInfoW,
    GetTextFaceW, HBRUSH, HDC, HFONT, IntersectClipRect, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint, PAINTSTRUCT, RestoreDC, SRCCOPY, SaveDC, SelectObject, SetBkColor, SetBkMode, SetTextColor,
    TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows_sys::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForMonitor, GetSystemMetricsForDpi, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow, ES_AUTOHSCROLL,
    ES_AUTOVSCROLL, ES_MULTILINE, GetCursorPos, GetWindowTextLengthW, GetWindowTextW, HICON, ICON_BIG, ICON_SMALL,
    IDC_ARROW, IDCANCEL, IDNO, IDOK, IMAGE_ICON, IsDialogMessageW, KillTimer, LR_DEFAULTCOLOR, LoadCursorW,
    LoadIconW, LoadImageW, MB_ICONERROR, MB_ICONWARNING, MB_OK, MB_YESNO, MF_CHECKED, MF_SEPARATOR, MF_STRING, MSG,
    MessageBoxW, RegisterClassW, SM_CXICON, SM_CXSMICON, SW_HIDE, SW_SHOW, SendMessageW, SetForegroundWindow,
    SetTimer, SetWindowTextW, ShowWindow, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_TOPALIGN, TrackPopupMenuEx, WM_CHAR,
    WM_CLOSE, WM_COMMAND, WM_CTLCOLOREDIT, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_SETFONT, WM_SETICON, WM_SETTINGCHANGE, WM_SYSCHAR,
    WM_SYSKEYDOWN, WM_SYSKEYUP, WM_TIMER, WNDCLASSW, WS_CAPTION, WS_CHILD, WS_MINIMIZEBOX, WS_OVERLAPPED,
    WS_SYSMENU, WS_TABSTOP,
};

/// The dictation hotkey is ignored while a new key is being captured.
pub static HOTKEY_PAUSED: AtomicBool = AtomicBool::new(false);

const WM_MOUSELEAVE: u32 = 0x02A3;
const EN_SETFOCUS: u32 = 0x0100;
const EN_KILLFOCUS: u32 = 0x0200;
const EN_CHANGE: u32 = 0x0300;
const EM_SETCUEBANNER: u32 = 0x1501;
const ID_PROMPT: usize = 104;
const ID_SEARCH: usize = 105;
const TIMER_CAPTURE: usize = 2;
const TIMER_ANIM: usize = 3;
const TIMER_COPIED: usize = 4;
/// The app icon in the exe's resources (assets/dictate.rc).
const IDI_APP: u16 = 1;

// ----------------------------------------------------------------- layout --

const SIDEBAR_W: f32 = 200.0;
const WIN_W: f32 = SIDEBAR_W + 560.0;
const WIN_H: f32 = 540.0;
const _: () = assert!(WIN_H < 760.0, "window must fit a 768 px tall screen");
const CARD_X: f32 = SIDEBAR_W + 20.0;
const CARD_W: f32 = 520.0;
const ROW_PAD: f32 = 16.0;
const MODE_W: f32 = 214.0;
const BUBBLE_W: f32 = 252.0;
const POPUP_W: f32 = 180.0;
const SEARCH_W: f32 = 210.0;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Rect {
    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
    fn right(&self) -> f32 {
        self.x + self.w
    }
    fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    General,
    Recognition,
    App,
    History,
}

const PAGES: [Page; 4] = [Page::General, Page::Recognition, Page::App, Page::History];

impl Page {
    pub fn parse(s: &str) -> Option<Page> {
        PAGES.into_iter().find(|p| format!("{p:?}").eq_ignore_ascii_case(s))
    }
    pub fn index(self) -> usize {
        PAGES.iter().position(|&p| p == self).unwrap_or(0)
    }
    pub fn from_index(i: usize) -> Page {
        PAGES.get(i).copied().unwrap_or(Page::General)
    }
    fn title(self) -> T {
        match self {
            Page::General => T::PageGeneral,
            Page::Recognition => T::SecRecognition,
            Page::App => T::PageApp,
            Page::History => T::PageHistory,
        }
    }
    fn sections(self) -> &'static [(T, &'static [RowKind])] {
        match self {
            Page::General => &[
                (T::SecControls, &[RowKind::Key, RowKind::Mode]),
                (T::Bubble, &[RowKind::Bubble, RowKind::Live]),
            ],
            Page::Recognition => &[
                (T::SecSpeech, &[RowKind::SpeechLang, RowKind::Prompt]),
                (T::SecOutput, &[RowKind::Space, RowKind::Clipboard]),
            ],
            Page::App => &[
                (T::SecInterface, &[RowKind::UiLang, RowKind::Autostart]),
                (T::PageHistory, &[RowKind::HistoryMode]),
            ],
            Page::History => &[],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hit {
    Page(u8),
    Key,
    Mode(u8),
    SpeechLang,
    Space,
    Clipboard,
    Overlay(u8),
    Live,
    UiLang,
    Autostart,
    HistoryMode,
    Copy(usize),
    Clear,
    File,
    Cancel,
    Save,
}

#[derive(Clone, Copy)]
enum RowKind {
    Key,
    Mode,
    SpeechLang,
    Prompt,
    Space,
    Clipboard,
    Bubble,
    Live,
    UiLang,
    Autostart,
    HistoryMode,
}

fn row_h(k: RowKind) -> f32 {
    match k {
        RowKind::Key | RowKind::Mode | RowKind::Live | RowKind::Clipboard | RowKind::HistoryMode => 60.0,
        RowKind::Prompt => 118.0,
        _ => 48.0,
    }
}

const TOP: f32 = 12.0;
const TITLE_H: f32 = 40.0;
const SECTION_LABEL_H: f32 = 26.0;
const SECTION_GAP: f32 = 16.0;
const FOOTER_H: f32 = 32.0;

fn content_top() -> f32 {
    TOP + TITLE_H
}

#[cfg(test)]
fn page_height(p: Page) -> f32 {
    let mut y = content_top();
    for (_, rows) in p.sections() {
        y += SECTION_LABEL_H + rows.iter().map(|&r| row_h(r)).sum::<f32>() + SECTION_GAP;
    }
    y
}

fn footer_y() -> f32 {
    WIN_H - 20.0 - FOOTER_H
}

/// Text field rect of the prompt row (fixed geometry, also used to place the EDIT).
fn prompt_field() -> Rect {
    let mut y = content_top();
    for (_, rows) in Page::Recognition.sections() {
        y += SECTION_LABEL_H;
        for &r in *rows {
            if matches!(r, RowKind::Prompt) {
                return Rect { x: CARD_X + ROW_PAD, y: y + 58.0, w: CARD_W - 2.0 * ROW_PAD, h: 48.0 };
            }
            y += row_h(r);
        }
        y += SECTION_GAP;
    }
    Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 }
}

/// Search box of the history page, in the title row.
fn search_field(clear_w: f32) -> Rect {
    Rect { x: CARD_X + CARD_W - clear_w - 10.0 - SEARCH_W, y: TOP + 4.0, w: SEARCH_W, h: 30.0 }
}

/// The scrolling list of the history page.
fn list_rect() -> Rect {
    let y = content_top() + 6.0;
    Rect { x: CARD_X, y, w: CARD_W, h: WIN_H - 16.0 - y }
}

// ------------------------------------------------------------------ theme --

#[derive(Clone, Copy)]
struct Theme {
    dark: bool,
    bg: u32,
    sidebar: u32,
    card: u32,
    border: u32,
    sep: u32,
    text: u32,
    sub: u32,
    accent: u32,
    control: u32,
    seg_sel: u32,
    toggle_off: u32,
    field: u32,
    chip: u32,
    danger: u32,
    ok: u32,
}

fn theme(dark: bool) -> Theme {
    if dark {
        Theme {
            dark,
            bg: 0x1E1E20,
            sidebar: 0x262629,
            card: 0x2A2A2D,
            border: 0x3A3A3E,
            sep: 0x3A3A3D,
            text: 0xF5F5F7,
            sub: 0x98989D,
            accent: 0x0A84FF,
            control: 0x3A3A3E,
            seg_sel: 0x636366,
            toggle_off: 0x4A4A4E,
            field: 0x1F1F22,
            chip: 0x3A3A3E,
            danger: 0xFF6961,
            ok: 0x32D74B,
        }
    } else {
        Theme {
            dark,
            bg: 0xF0F0F3,
            sidebar: 0xE7E7EB,
            card: 0xFFFFFF,
            border: 0xE0E0E4,
            sep: 0xE8E8EB,
            text: 0x1D1D1F,
            sub: 0x86868B,
            accent: 0x007AFF,
            control: 0xE6E6EA,
            seg_sel: 0xFFFFFF,
            toggle_off: 0xD9D9DE,
            field: 0xF7F7F9,
            chip: 0xF2F2F5,
            danger: 0xD70015,
            ok: 0x248A3D,
        }
    }
}

fn system_dark() -> bool {
    // DICTATE_THEME=light|dark overrides the system setting (for checking both looks).
    if let Some(v) = std::env::var_os("DICTATE_THEME") {
        return v == "dark";
    }
    let key = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
    let val = wide("AppsUseLightTheme");
    let mut data: u32 = 1;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            val.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            &mut data as *mut u32 as *mut _,
            &mut size,
        )
    };
    r == 0 && data == 0
}

fn colorref(rgb: u32) -> COLORREF {
    ((rgb & 0xFF) << 16) | (rgb & 0xFF00) | ((rgb >> 16) & 0xFF)
}

// ------------------------------------------------------------------- time --

/// "Today 14:32", "Yesterday 09:10" or "13.11.2026 18:05" in the user's locale.
fn format_time(at: u64) -> String {
    fn local(unix: u64) -> SYSTEMTIME {
        let ticks = (unix + 11_644_473_600) * 10_000_000;
        let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
        unsafe {
            let mut utc: SYSTEMTIME = std::mem::zeroed();
            let mut loc: SYSTEMTIME = std::mem::zeroed();
            FileTimeToSystemTime(&ft, &mut utc);
            SystemTimeToTzSpecificLocalTime(null(), &utc, &mut loc);
            loc
        }
    }
    let same_day = |a: &SYSTEMTIME, b: &SYSTEMTIME| a.wYear == b.wYear && a.wMonth == b.wMonth && a.wDay == b.wDay;
    let when = local(at);
    let mut now: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut now) };
    let yesterday = local(history::now().saturating_sub(86_400));
    let mut buf = [0u16; 80];
    let n = unsafe { GetTimeFormatEx(null(), 2 /* TIME_NOSECONDS */, &when, null(), buf.as_mut_ptr(), 80) };
    let time = String::from_utf16_lossy(&buf[..(n.max(1) - 1) as usize]);
    let day = if same_day(&when, &now) {
        t(T::Today).to_string()
    } else if same_day(&when, &yesterday) {
        t(T::Yesterday).to_string()
    } else {
        let n = unsafe { GetDateFormatEx(null(), 1 /* DATE_SHORTDATE */, &when, null(), buf.as_mut_ptr(), 80, null()) };
        String::from_utf16_lossy(&buf[..(n.max(1) - 1) as usize])
    };
    format!("{day}  {time}")
}

// ------------------------------------------------------------------ state --

enum Capture {
    Off,
    Waiting(Instant),
    /// Captured; dictation resumes once the key is released.
    Releasing(i32),
}

struct Fonts {
    body: HFONT,
    bold: HFONT,
    sub: HFONT,
    section: HFONT,
    title: HFONT,
}

struct St {
    hwnd: HWND,
    edit: HWND,
    search: HWND,
    fonts: Fonts,
    field_brush: HBRUSH,
    dib: Option<Dib>,
    scale: f32,
    th: Theme,
    page: Page,
    // values
    vk: i32,
    mode: Mode,
    speech: String,
    overlay: Overlay,
    live: bool,
    space: bool,
    clipboard: bool,
    history_mode: history::Mode,
    ui_lang: String,
    ui_lang_before: Lang,
    /// Lives in the registry (HKCU Run), not in dictate.ini.
    autostart: bool,
    autostart_before: bool,
    // history page
    query: String,
    scroll: f32,
    max_scroll: f32,
    /// Texts of the entries on screen, newest first, indexed by `Hit::Copy`.
    shown: Vec<String>,
    copied: Option<(usize, Instant)>,
    // interaction
    hits: Vec<(Hit, Rect)>,
    hover: Option<Hit>,
    pressed: Option<Hit>,
    tracking: bool,
    edit_focus: bool,
    search_focus: bool,
    capture: Capture,
    capture_bad: bool,
    // animation: current values chasing targets
    a_mode: f32,
    a_overlay: f32,
    a_space: f32,
    a_live: f32,
    a_auto: f32,
    a_clip: f32,
    last_anim: Instant,
    cfg_path: PathBuf,
    on_saved: fn(),
}

thread_local! {
    static ST: RefCell<Option<St>> = const { RefCell::new(None) };
}

/// Runs `f` on the window state. Modal loops (popup menus, message boxes) are
/// never entered from inside `f`, but if a message arrives re-entrantly anyway
/// it is skipped rather than aborting on a double borrow.
fn with<R>(f: impl FnOnce(&mut St) -> R) -> Option<R> {
    ST.with(|s| s.try_borrow_mut().ok()?.as_mut().map(f))
}

pub fn hwnd() -> Option<HWND> {
    ST.with(|s| s.borrow().as_ref().map(|s| s.hwnd))
}

fn capturing() -> bool {
    ST.with(|s| s.borrow().as_ref().is_some_and(|s| !matches!(s.capture, Capture::Off)))
}

/// Call for every message in the UI loop; true = already handled.
pub fn pre_translate(msg: &MSG) -> bool {
    let Some(h) = hwnd() else { return false };
    if capturing() && matches!(msg.message, WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP | WM_CHAR | WM_SYSCHAR) {
        return true; // keys being captured must not type or trigger buttons
    }
    // Enter = Save, Esc = Cancel, Tab focus: the dialog manager does it for us.
    unsafe { IsDialogMessageW(h, msg) != 0 }
}

/// Opens the window on `page`, or brings the open one forward and switches to it.
pub fn open(cfg: &Config, cfg_path: PathBuf, on_saved: fn(), page: Page) {
    if let Some(h) = hwnd() {
        with(|s| s.set_page(page));
        unsafe { SetForegroundWindow(h) };
        return;
    }
    unsafe { create(cfg, cfg_path, on_saved, page) };
}

/// A dictation was added to the history: repaint if it is on screen.
pub fn history_changed() {
    with(|s| {
        if s.page == Page::History {
            s.invalidate();
        }
    });
}

fn make_font(dpi: u32, pt: f32, weight: u32) -> HFONT {
    let make = |face: &str| {
        let f = wide(face);
        unsafe {
            CreateFontW(
                -((pt * dpi as f32 / 72.0).round() as i32),
                0,
                0,
                0,
                weight as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                0,
                0,
                CLEARTYPE_QUALITY as u32,
                0,
                f.as_ptr(),
            )
        }
    };
    // Windows 11's variable UI font if present, classic Segoe UI otherwise.
    let font = make("Segoe UI Variable Text");
    let ok = unsafe {
        let dc = windows_sys::Win32::Graphics::Gdi::CreateCompatibleDC(null_mut());
        let old = SelectObject(dc, font);
        let mut name = [0u16; 64];
        let n = GetTextFaceW(dc, 64, name.as_mut_ptr());
        SelectObject(dc, old);
        windows_sys::Win32::Graphics::Gdi::DeleteDC(dc);
        String::from_utf16_lossy(&name[..n.max(1) as usize - 1]).starts_with("Segoe UI Variable")
    };
    if ok {
        font
    } else {
        unsafe { DeleteObject(font) };
        make("Segoe UI")
    }
}

/// Borderless EDIT inside a painted field.
unsafe fn make_edit(parent: HWND, id: usize, text: &str, style: u32, r: Rect, s: f32, font: HFONT) -> HWND {
    unsafe {
        let cls = wide("EDIT");
        let txt = wide(text);
        let edit = CreateWindowExW(
            0,
            cls.as_ptr(),
            txt.as_ptr(),
            WS_CHILD | WS_TABSTOP | style,
            (r.x * s) as i32,
            (r.y * s) as i32,
            (r.w * s) as i32,
            (r.h * s) as i32,
            parent,
            id as _,
            GetModuleHandleW(null()),
            null(),
        );
        SendMessageW(edit, WM_SETFONT, font as usize, 1);
        edit
    }
}

unsafe fn create(cfg: &Config, cfg_path: PathBuf, on_saved: fn(), page: Page) {
    unsafe {
        let hinst = GetModuleHandleW(null());
        let class = wide("dictate-settings");
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(proc_),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: LoadIconW(hinst, IDI_APP as usize as *const u16),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc); // fails harmlessly on reopen

        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let (mut dpi, mut dy) = (96u32, 96u32);
        GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dpi, &mut dy);
        let s = dpi as f32 / 96.0;
        let (cw, ch) = ((WIN_W * s).round() as i32, (WIN_H * s).round() as i32);
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let mut r = RECT { left: 0, top: 0, right: cw, bottom: ch };
        AdjustWindowRectExForDpi(&mut r, style, 0, 0, dpi);
        let (ww, wh) = (r.right - r.left, r.bottom - r.top);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(mon, &mut mi);
        let wa = mi.rcWork;
        let (x, y) = ((wa.left + wa.right - ww) / 2, (wa.top + wa.bottom - wh) / 2);

        let title = wide(t(T::SettingsTitle));
        let hwnd = CreateWindowExW(0, class.as_ptr(), title.as_ptr(), style, x, y, ww, wh, null_mut(), null_mut(), hinst, null());
        if hwnd.is_null() {
            return;
        }
        // Title bar and Alt+Tab icons at the right size for this DPI.
        for (which, metric) in [(ICON_SMALL, SM_CXSMICON), (ICON_BIG, SM_CXICON)] {
            let px = GetSystemMetricsForDpi(metric, dpi);
            let icon = LoadImageW(hinst, IDI_APP as usize as *const u16, IMAGE_ICON, px, px, LR_DEFAULTCOLOR) as HICON;
            if !icon.is_null() {
                SendMessageW(hwnd, WM_SETICON, which as usize, icon as isize);
            }
        }
        let th = theme(system_dark());
        apply_titlebar(hwnd, th);

        let fonts = Fonts {
            body: make_font(dpi, 10.0, FW_NORMAL),
            bold: make_font(dpi, 10.0, FW_SEMIBOLD),
            sub: make_font(dpi, 8.5, FW_NORMAL),
            section: make_font(dpi, 9.0, FW_SEMIBOLD),
            title: make_font(dpi, 14.0, FW_SEMIBOLD),
        };

        // The prompt and search are real EDITs (IME, selection, clipboard) inside painted fields.
        let f = prompt_field();
        let pr = Rect { x: f.x + 10.0, y: f.y + 7.0, w: f.w - 20.0, h: f.h - 14.0 };
        let edit = make_edit(hwnd, ID_PROMPT, &cfg.prompt, ES_MULTILINE as u32 | ES_AUTOVSCROLL as u32, pr, s, fonts.body);
        let sf = search_field(80.0);
        let sr = Rect { x: sf.x + 10.0, y: sf.y + 6.0, w: sf.w - 20.0, h: sf.h - 12.0 };
        let search = make_edit(hwnd, ID_SEARCH, "", ES_AUTOHSCROLL as u32, sr, s, fonts.body);
        let cue = wide(t(T::HistorySearch));
        SendMessageW(search, EM_SETCUEBANNER, 1, cue.as_ptr() as isize);

        let ui_before = i18n::get();
        let auto_on = std::env::current_exe().is_ok_and(|e| autostart::is_enabled(&e));
        let b = |v: bool| if v { 1.0 } else { 0.0 };
        let st = St {
            hwnd,
            edit,
            search,
            field_brush: CreateSolidBrush(colorref(th.field)),
            fonts,
            dib: None,
            scale: s,
            th,
            page,
            vk: cfg.hotkey,
            mode: cfg.mode,
            speech: cfg.language.clone(),
            overlay: cfg.overlay,
            live: cfg.live_preview,
            space: cfg.append_space,
            clipboard: cfg.clipboard,
            history_mode: cfg.history,
            ui_lang: cfg.ui_language.clone(),
            ui_lang_before: ui_before,
            autostart: auto_on,
            autostart_before: auto_on,
            query: String::new(),
            scroll: 0.0,
            max_scroll: 0.0,
            shown: Vec::new(),
            copied: None,
            hits: Vec::new(),
            hover: None,
            pressed: None,
            tracking: false,
            edit_focus: false,
            search_focus: false,
            capture: Capture::Off,
            capture_bad: false,
            a_mode: b(cfg.mode == Mode::Toggle),
            a_overlay: overlay_index(cfg.overlay) as f32,
            a_space: b(cfg.append_space),
            a_live: b(cfg.live_preview),
            a_auto: b(auto_on),
            a_clip: b(cfg.clipboard),
            last_anim: Instant::now(),
            cfg_path,
            on_saved,
        };
        ST.with(|s| *s.borrow_mut() = Some(st));
        with(|s| s.show_page_controls());
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        SetFocus(hwnd);
    }
}

fn apply_titlebar(hwnd: HWND, th: Theme) {
    unsafe {
        let dark: i32 = th.dark as i32;
        DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE as u32, &dark as *const i32 as *const _, 4);
        // Windows 11: title bar in the window's background color, for a seamless macOS-like look.
        let cap = colorref(th.sidebar);
        let txt = colorref(th.text);
        DwmSetWindowAttribute(hwnd, DWMWA_CAPTION_COLOR as u32, &cap as *const u32 as *const _, 4);
        DwmSetWindowAttribute(hwnd, DWMWA_TEXT_COLOR as u32, &txt as *const u32 as *const _, 4);
    }
}

fn overlay_index(o: Overlay) -> usize {
    match o {
        Overlay::Bottom => 0,
        Overlay::Center => 1,
        Overlay::Off => 2,
    }
}

fn speech_options() -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = LANGS.iter().map(|l| (l.code().to_string(), l.native_name().to_string())).collect();
    v.push(("auto".into(), t(T::SpeechAuto).into()));
    v
}

fn ui_options() -> Vec<(String, String)> {
    let mut v = vec![("auto".to_string(), t(T::MenuSystemLang).to_string())];
    v.extend(LANGS.iter().map(|l| (l.code().to_string(), l.native_name().to_string())));
    v
}

fn history_options() -> Vec<(String, String)> {
    [(history::Mode::Off, T::BubbleOff), (history::Mode::Session, T::HistorySession), (history::Mode::Disk, T::HistoryDisk)]
        .iter()
        .map(|(m, l)| (m.as_str().to_string(), t(*l).to_string()))
        .collect()
}

fn label_of(options: &[(String, String)], code: &str) -> String {
    options.iter().find(|o| o.0 == code).map(|o| o.1.clone()).unwrap_or_else(|| code.to_string())
}

// ------------------------------------------------------------------ paint --

struct TextOp {
    text: String,
    r: Rect,
    font: HFONT,
    color: u32,
    flags: u32,
    clip: Option<Rect>,
}

impl TextOp {
    fn new(text: impl Into<String>, r: Rect, font: HFONT, color: u32, flags: u32) -> Self {
        Self { text: text.into(), r, font, color, flags, clip: None }
    }
    fn clipped(mut self, c: Rect) -> Self {
        self.clip = Some(c);
        self
    }
}

const ONE_LINE: u32 = DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS;

impl St {
    fn invalidate(&self) {
        unsafe { InvalidateRect(self.hwnd, null(), 0) };
    }

    fn measure(&self, font: HFONT, text: &str) -> f32 {
        let Some(d) = &self.dib else { return text.chars().count() as f32 * 7.0 };
        let w = wide(text);
        let mut r = RECT { left: 0, top: 0, right: 10_000, bottom: 100 };
        unsafe {
            let old = SelectObject(d.dc, font);
            DrawTextW(d.dc, w.as_ptr(), -1, &mut r, DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX);
            SelectObject(d.dc, old);
        }
        (r.right - r.left) as f32 / self.scale
    }

    /// Height of `text` wrapped to `width` logical pixels.
    fn measure_wrapped(&self, font: HFONT, text: &str, width: f32) -> f32 {
        let Some(d) = &self.dib else { return 20.0 };
        let w = wide(text);
        let mut r = RECT { left: 0, top: 0, right: (width * self.scale) as i32, bottom: 100 };
        unsafe {
            let old = SelectObject(d.dc, font);
            DrawTextW(d.dc, w.as_ptr(), -1, &mut r, DT_CALCRECT | DT_WORDBREAK | DT_EDITCONTROL | DT_NOPREFIX);
            SelectObject(d.dc, old);
        }
        (r.bottom - r.top) as f32 / self.scale
    }

    fn set_page(&mut self, p: Page) {
        if self.page != p {
            self.page = p;
            self.scroll = 0.0;
            unsafe { SetFocus(self.hwnd) };
            self.show_page_controls();
            self.invalidate();
        }
    }

    fn show_page_controls(&self) {
        unsafe {
            ShowWindow(self.edit, if self.page == Page::Recognition { SW_SHOW } else { SW_HIDE });
            ShowWindow(self.search, if self.page == Page::History { SW_SHOW } else { SW_HIDE });
        }
    }

    fn paint(&mut self, hdc: HDC) {
        let s = self.scale;
        let (w, h) = ((WIN_W * s).round() as i32, (WIN_H * s).round() as i32);
        if self.dib.as_ref().is_none_or(|d| d.w != w || d.h != h) {
            self.dib = make_dib(w, h);
        }
        if self.dib.is_none() {
            return;
        }
        let mut texts: Vec<TextOp> = Vec::new();
        let mut hits: Vec<(Hit, Rect)> = Vec::new();

        // Everything that needs GDI measuring happens before the pixels are borrowed.
        let history_layout = (self.page == Page::History).then(|| self.history_layout());
        let footer_w = (
            (self.measure(self.fonts.bold, t(T::Save)) + 36.0).max(100.0),
            (self.measure(self.fonts.body, t(T::Cancel)) + 36.0).max(90.0),
            self.measure(self.fonts.body, t(T::SettingsFile)) + 20.0,
        );
        let key_text = match self.capture {
            Capture::Waiting(_) => t(T::HotkeyPress).to_string(),
            _ => config::key_label(self.vk),
        };
        let key_w = (self.measure(self.fonts.body, &key_text) + 32.0).max(128.0);
        let clear_w = self.measure(self.fonts.body, t(T::HistoryClear)) + 24.0;
        let copy_w = self.measure(self.fonts.sub, t(T::Copied)).max(self.measure(self.fonts.sub, t(T::Copy))) + 20.0;

        let th = self.th;
        // The DIB is taken out while its pixels are borrowed by the canvas.
        let mut dib = self.dib.take().unwrap();
        {
            let mut cv = Canvas { px: dib.pixels(), w, h, s };
            cv.clear(th.bg);
            self.paint_sidebar(&mut cv, &mut texts, &mut hits);

            let title_r = Rect { x: CARD_X + 2.0, y: TOP, w: CARD_W * 0.6, h: TITLE_H - 6.0 };
            texts.push(TextOp::new(t(self.page.title()), title_r, self.fonts.title, th.text, DT_LEFT | ONE_LINE));

            if let Some(layout) = history_layout {
                self.paint_history(&mut cv, &mut texts, &mut hits, layout, clear_w, copy_w);
            } else {
                self.paint_rows(&mut cv, &mut texts, &mut hits, &key_text, key_w);
                self.paint_footer(&mut cv, &mut texts, &mut hits, footer_w);
            }
        }
        self.dib = Some(dib);

        // text on top of the shapes, then present
        let dib = self.dib.as_ref().unwrap();
        unsafe {
            GdiFlush();
            SetBkMode(dib.dc, TRANSPARENT as i32);
            for op in &texts {
                let saved = op.clip.map(|c| {
                    let id = SaveDC(dib.dc);
                    IntersectClipRect(
                        dib.dc,
                        (c.x * s) as i32,
                        (c.y * s) as i32,
                        (c.right() * s).ceil() as i32,
                        (c.bottom() * s).ceil() as i32,
                    );
                    id
                });
                let old = SelectObject(dib.dc, op.font);
                SetTextColor(dib.dc, colorref(op.color));
                let w = wide(&op.text);
                let mut r = RECT {
                    left: (op.r.x * s).round() as i32,
                    top: (op.r.y * s).round() as i32,
                    right: (op.r.right() * s).round() as i32,
                    bottom: (op.r.bottom() * s).round() as i32,
                };
                DrawTextW(dib.dc, w.as_ptr(), -1, &mut r, op.flags | DT_NOPREFIX);
                SelectObject(dib.dc, old);
                if let Some(id) = saved {
                    RestoreDC(dib.dc, id);
                }
            }
            GdiFlush();
            BitBlt(hdc, 0, 0, w, h, dib.dc, 0, 0, SRCCOPY);
        }
        self.hits = hits;
    }

    fn tint(&self, base: u32, hit: Hit) -> u32 {
        if self.pressed == Some(hit) {
            mix(base, self.th.text, 0.12)
        } else if self.hover == Some(hit) {
            mix(base, self.th.text, 0.05)
        } else {
            base
        }
    }

    fn paint_sidebar(&self, cv: &mut Canvas, texts: &mut Vec<TextOp>, hits: &mut Vec<(Hit, Rect)>) {
        let th = self.th;
        let sb = |cv: &mut Canvas| {
            // the sidebar is a plain rectangle; round_rect with r = 0 fills it
            cv.round_rect(0.0, 0.0, SIDEBAR_W, WIN_H, 0.0, th.sidebar, 1.0);
            cv.line((SIDEBAR_W - 0.5, 0.0), (SIDEBAR_W - 0.5, WIN_H), 1.0, th.border, 1.0);
        };
        sb(cv);
        for (i, &p) in PAGES.iter().enumerate() {
            let r = Rect { x: 10.0, y: 12.0 + i as f32 * 36.0, w: SIDEBAR_W - 20.0, h: 32.0 };
            let hit = Hit::Page(i as u8);
            let selected = self.page == p;
            if selected {
                cv.round_rect(r.x, r.y, r.w, r.h, 7.0, th.accent, 1.0);
            } else if self.hover == Some(hit) {
                cv.round_rect(r.x, r.y, r.w, r.h, 7.0, mix(th.sidebar, th.text, 0.07), 1.0);
            }
            page_icon(cv, p, r.x + 8.0, r.y + 5.0, selected);
            texts.push(TextOp::new(
                t(p.title()),
                Rect { x: r.x + 38.0, y: r.y, w: r.w - 44.0, h: r.h },
                if selected { self.fonts.bold } else { self.fonts.body },
                if selected { 0xFFFFFF } else { th.text },
                DT_LEFT | ONE_LINE,
            ));
            hits.push((hit, r));
        }
    }

    fn paint_rows(&self, cv: &mut Canvas, texts: &mut Vec<TextOp>, hits: &mut Vec<(Hit, Rect)>, key_text: &str, key_w: f32) {
        let th = self.th;
        let hover = self.hover;
        let right = CARD_X + CARD_W - ROW_PAD;
        let mut y = content_top();
        let speech_opts = speech_options();
        let ui_opts = ui_options();
        let hist_opts = history_options();
        for (title, rows) in self.page.sections() {
            texts.push(TextOp::new(
                t(*title),
                Rect { x: CARD_X + 6.0, y, w: CARD_W, h: SECTION_LABEL_H - 6.0 },
                self.fonts.section,
                th.sub,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE,
            ));
            y += SECTION_LABEL_H;
            let card_h: f32 = rows.iter().map(|&r| row_h(r)).sum();
            cv.shadow(CARD_X, y, CARD_W, card_h, 10.0, 6.0, 1.0, 0x000000, if th.dark { 0.25 } else { 0.06 });
            cv.round_rect(CARD_X, y, CARD_W, card_h, 10.0, th.card, 1.0);
            cv.round_rect_stroke(CARD_X, y, CARD_W, card_h, 10.0, 1.0, th.border, 1.0);

            for (ri, &row) in rows.iter().enumerate() {
                let rh = row_h(row);
                if ri > 0 {
                    cv.line((CARD_X + ROW_PAD, y), (CARD_X + CARD_W - ROW_PAD, y), 1.0, th.sep, 1.0);
                }
                let (title, sub): (&str, Option<String>) = match row {
                    RowKind::Key => (
                        t(T::Hotkey),
                        Some(match self.capture {
                            Capture::Waiting(_) if self.capture_bad => t(T::HotkeyBad).to_string(),
                            Capture::Waiting(_) => t(T::HotkeyEsc).to_string(),
                            _ => t(T::HotkeySub).to_string(),
                        }),
                    ),
                    RowKind::Mode => (
                        t(T::Mode),
                        Some(t(if self.mode == Mode::Toggle { T::ModeToggleSub } else { T::ModeHoldSub }).to_string()),
                    ),
                    RowKind::SpeechLang => (t(T::SpeechLang), None),
                    RowKind::Prompt => (t(T::Prompt), Some(t(T::PromptSub).to_string())),
                    RowKind::Space => (t(T::Space), None),
                    RowKind::Clipboard => (t(T::Clipboard), Some(t(T::ClipboardSub).to_string())),
                    RowKind::Bubble => (t(T::BubblePos), None),
                    RowKind::Live => (t(T::Live), Some(t(T::LiveSub).to_string())),
                    RowKind::UiLang => (t(T::UiLang), None),
                    RowKind::Autostart => (t(T::Autostart), None),
                    RowKind::HistoryMode => (
                        t(T::HistoryMode),
                        Some(
                            t(match self.history_mode {
                                history::Mode::Off => T::HistoryOffSub,
                                history::Mode::Session => T::HistorySessionSub,
                                history::Mode::Disk => T::HistoryDiskSub,
                            })
                            .to_string(),
                        ),
                    ),
                };
                let tx = CARD_X + ROW_PAD;
                // width of the control on the right, so the texts never run under it
                let ctrl_w = match row {
                    RowKind::Key => key_w,
                    RowKind::Mode => MODE_W,
                    RowKind::Bubble => BUBBLE_W,
                    RowKind::SpeechLang | RowKind::UiLang | RowKind::HistoryMode => POPUP_W,
                    RowKind::Space | RowKind::Live | RowKind::Autostart | RowKind::Clipboard => 42.0,
                    RowKind::Prompt => 0.0,
                };
                let text_w = CARD_W - 2.0 * ROW_PAD - if ctrl_w > 0.0 { ctrl_w + 14.0 } else { 0.0 };
                let (ty, th_) = match (&sub, row) {
                    (_, RowKind::Prompt) => (y + 10.0, 20.0),
                    (Some(_), _) => (y + 11.0, 20.0),
                    (None, _) => (y, rh),
                };
                texts.push(TextOp::new(title, Rect { x: tx, y: ty, w: text_w, h: th_ }, self.fonts.body, th.text, DT_LEFT | ONE_LINE));
                if let Some(sub) = sub {
                    let bad = matches!(row, RowKind::Key) && self.capture_bad;
                    texts.push(TextOp::new(
                        sub,
                        Rect { x: tx, y: ty + 20.0, w: text_w, h: 18.0 },
                        self.fonts.sub,
                        if bad { th.danger } else { th.sub },
                        DT_LEFT | ONE_LINE,
                    ));
                }

                let cy = y + if matches!(row, RowKind::Prompt) { 20.0 } else { rh / 2.0 };
                match row {
                    RowKind::Key => {
                        let r = Rect { x: right - key_w, y: cy - 15.0, w: key_w, h: 30.0 };
                        let capturing = matches!(self.capture, Capture::Waiting(_));
                        cv.round_rect(r.x, r.y, r.w, r.h, 8.0, self.tint(th.chip, Hit::Key), 1.0);
                        if capturing {
                            cv.round_rect_stroke(r.x - 1.0, r.y - 1.0, r.w + 2.0, r.h + 2.0, 9.0, 2.0, th.accent, 1.0);
                        } else {
                            cv.round_rect_stroke(r.x, r.y, r.w, r.h, 8.0, 1.0, th.border, 1.0);
                        }
                        texts.push(TextOp::new(
                            key_text,
                            r,
                            self.fonts.body,
                            if capturing { th.accent } else { th.text },
                            DT_CENTER | ONE_LINE,
                        ));
                        hits.push((Hit::Key, r));
                    }
                    RowKind::Mode => {
                        let labels = [t(T::ModeHold), t(T::ModeToggle)];
                        segmented(cv, texts, hits, th, &self.fonts, right, cy, MODE_W, &labels, self.a_mode, |i| Hit::Mode(i as u8), hover);
                    }
                    RowKind::Bubble => {
                        let labels = [t(T::BubbleBottom), t(T::BubbleCenter), t(T::BubbleOff)];
                        segmented(cv, texts, hits, th, &self.fonts, right, cy, BUBBLE_W, &labels, self.a_overlay, |i| {
                            Hit::Overlay(i as u8)
                        }, hover);
                    }
                    RowKind::SpeechLang | RowKind::UiLang | RowKind::HistoryMode => {
                        let (hit, label) = match row {
                            RowKind::SpeechLang => (Hit::SpeechLang, label_of(&speech_opts, &self.speech)),
                            RowKind::UiLang => (Hit::UiLang, label_of(&ui_opts, &self.ui_lang)),
                            _ => (Hit::HistoryMode, label_of(&hist_opts, self.history_mode.as_str())),
                        };
                        let r = Rect { x: right - POPUP_W, y: cy - 14.0, w: POPUP_W, h: 28.0 };
                        popup_button(cv, texts, th, &self.fonts, r, &label, self.tint(th.chip, hit));
                        hits.push((hit, r));
                    }
                    RowKind::Space | RowKind::Live | RowKind::Autostart | RowKind::Clipboard => {
                        let (hit, v) = match row {
                            RowKind::Space => (Hit::Space, self.a_space),
                            RowKind::Live => (Hit::Live, self.a_live),
                            RowKind::Clipboard => (Hit::Clipboard, self.a_clip),
                            _ => (Hit::Autostart, self.a_auto),
                        };
                        let r = Rect { x: right - 42.0, y: cy - 12.5, w: 42.0, h: 25.0 };
                        toggle(cv, th, r, v, hover == Some(hit));
                        hits.push((hit, Rect { x: r.x - 8.0, y: r.y - 6.0, w: r.w + 16.0, h: r.h + 12.0 }));
                    }
                    RowKind::Prompt => {
                        let f = prompt_field();
                        cv.round_rect(f.x, f.y, f.w, f.h, 8.0, th.field, 1.0);
                        if self.edit_focus {
                            cv.round_rect_stroke(f.x - 1.0, f.y - 1.0, f.w + 2.0, f.h + 2.0, 9.0, 2.5, th.accent, 0.85);
                        } else {
                            cv.round_rect_stroke(f.x, f.y, f.w, f.h, 8.0, 1.0, th.border, 1.0);
                        }
                    }
                }
                y += rh;
            }
            y += SECTION_GAP;
        }
    }

    fn paint_footer(&self, cv: &mut Canvas, texts: &mut Vec<TextOp>, hits: &mut Vec<(Hit, Rect)>, (save_w, cancel_w, file_w): (f32, f32, f32)) {
        let th = self.th;
        let fy = footer_y();
        let save = Rect { x: CARD_X + CARD_W - save_w, y: fy, w: save_w, h: FOOTER_H };
        let cancel = Rect { x: save.x - 10.0 - cancel_w, y: fy, w: cancel_w, h: FOOTER_H };
        let file = Rect { x: CARD_X, y: fy, w: file_w, h: FOOTER_H };
        let save_bg = if self.pressed == Some(Hit::Save) {
            mix(th.accent, 0x000000, 0.15)
        } else if self.hover == Some(Hit::Save) {
            mix(th.accent, 0xFFFFFF, 0.08)
        } else {
            th.accent
        };
        cv.shadow(save.x, save.y, save.w, save.h, 8.0, 4.0, 1.0, th.accent, 0.25);
        cv.round_rect(save.x, save.y, save.w, save.h, 8.0, save_bg, 1.0);
        cv.round_rect(cancel.x, cancel.y, cancel.w, cancel.h, 8.0, self.tint(th.card, Hit::Cancel), 1.0);
        cv.round_rect_stroke(cancel.x, cancel.y, cancel.w, cancel.h, 8.0, 1.0, th.border, 1.0);
        if self.hover == Some(Hit::File) {
            cv.round_rect(file.x, file.y, file.w, file.h, 8.0, mix(th.bg, th.text, 0.05), 1.0);
        }
        for (txt, r, font, color) in [
            (t(T::Save), save, self.fonts.bold, 0xFFFFFF),
            (t(T::Cancel), cancel, self.fonts.body, th.text),
            (t(T::SettingsFile), file, self.fonts.body, th.accent),
        ] {
            texts.push(TextOp::new(txt, r, font, color, DT_CENTER | DT_VCENTER | DT_SINGLELINE));
        }
        hits.push((Hit::Save, save));
        hits.push((Hit::Cancel, cancel));
        hits.push((Hit::File, file));
    }

    /// History entries to show (newest first, filtered by the search) with the
    /// height of each wrapped text.
    fn history_layout(&mut self) -> Vec<(u64, String, f32)> {
        let q = self.query.to_lowercase();
        let text_w = CARD_W - 2.0 * ROW_PAD;
        let entries = crate::history().entries();
        let shown: Vec<(u64, String, f32)> = entries
            .into_iter()
            .rev()
            .filter(|e| q.is_empty() || e.text.to_lowercase().contains(&q))
            .map(|e| {
                let h = self.measure_wrapped(self.fonts.body, &e.text, text_w);
                (e.at, e.text, h)
            })
            .collect();
        self.shown = shown.iter().map(|e| e.1.clone()).collect();
        shown
    }

    fn paint_history(
        &mut self,
        cv: &mut Canvas,
        texts: &mut Vec<TextOp>,
        hits: &mut Vec<(Hit, Rect)>,
        rows: Vec<(u64, String, f32)>,
        clear_w: f32,
        copy_w: f32,
    ) {
        let th = self.th;
        let list = list_rect();
        let row_total = |text_h: f32| 12.0 + 18.0 + 4.0 + text_h + 14.0;
        let total: f32 = rows.iter().map(|r| row_total(r.2)).sum();
        self.max_scroll = (total - list.h).max(0.0);
        self.scroll = self.scroll.clamp(0.0, self.max_scroll);
        let any = !crate::history().is_empty();

        if rows.is_empty() {
            let (head, sub) = if crate::history().mode() == history::Mode::Off {
                (T::HistoryIsOff, T::HistoryIsOffSub)
            } else if any {
                (T::HistoryNoMatch, T::HistorySearch)
            } else {
                (T::HistoryEmpty, T::HistoryEmptySub)
            };
            let cy = list.y + list.h * 0.38;
            page_icon(cv, Page::History, list.x + list.w / 2.0 - 11.0, cy - 50.0, false);
            texts.push(TextOp::new(t(head), Rect { x: list.x, y: cy - 16.0, w: list.w, h: 24.0 }, self.fonts.bold, th.text, DT_CENTER | ONE_LINE));
            if head != T::HistoryNoMatch {
                texts.push(TextOp::new(t(sub), Rect { x: list.x, y: cy + 10.0, w: list.w, h: 20.0 }, self.fonts.sub, th.sub, DT_CENTER | ONE_LINE));
            }
        } else {
            let top = list.y - self.scroll;
            cv.round_rect(list.x, top, list.w, total, 10.0, th.card, 1.0);
            cv.round_rect_stroke(list.x, top, list.w, total, 10.0, 1.0, th.border, 1.0);
            let mut y = top;
            let copied = self.copied.filter(|c| c.1.elapsed().as_millis() < 1500).map(|c| c.0);
            for (i, (at, text, text_h)) in rows.iter().enumerate() {
                let rh = row_total(*text_h);
                let visible = y + rh > list.y && y < list.bottom();
                if visible {
                    if i > 0 {
                        cv.line((list.x + ROW_PAD, y), (list.right() - ROW_PAD, y), 1.0, th.sep, 1.0);
                    }
                    let tx = list.x + ROW_PAD;
                    texts.push(
                        TextOp::new(format_time(*at), Rect { x: tx, y: y + 12.0, w: list.w - 2.0 * ROW_PAD - copy_w, h: 18.0 }, self.fonts.sub, th.sub, DT_LEFT | ONE_LINE)
                            .clipped(list),
                    );
                    texts.push(
                        TextOp::new(
                            text.clone(),
                            Rect { x: tx, y: y + 34.0, w: list.w - 2.0 * ROW_PAD, h: *text_h + 2.0 },
                            self.fonts.body,
                            th.text,
                            DT_LEFT | DT_WORDBREAK | DT_EDITCONTROL,
                        )
                        .clipped(list),
                    );
                    let b = Rect { x: list.right() - ROW_PAD - copy_w + 10.0, y: y + 8.0, w: copy_w, h: 24.0 };
                    let hit = Hit::Copy(i);
                    let done = copied == Some(i);
                    if self.hover == Some(hit) && !done {
                        cv.round_rect(b.x, b.y, b.w, b.h, 6.0, mix(th.card, th.accent, 0.10), 1.0);
                    }
                    texts.push(
                        TextOp::new(
                            t(if done { T::Copied } else { T::Copy }),
                            b,
                            self.fonts.sub,
                            if done { th.ok } else { th.accent },
                            DT_CENTER | ONE_LINE,
                        )
                        .clipped(list),
                    );
                    let vis = Rect { x: b.x, y: b.y.max(list.y), w: b.w, h: (b.bottom().min(list.bottom()) - b.y.max(list.y)).max(0.0) };
                    if vis.h > 0.0 {
                        hits.push((hit, vis));
                    }
                }
                y += rh;
            }
            // cover what scrolled past the list edges
            cv.round_rect(SIDEBAR_W + 1.0, 0.0, WIN_W - SIDEBAR_W, list.y, 0.0, th.bg, 1.0);
            cv.round_rect(SIDEBAR_W + 1.0, list.bottom(), WIN_W - SIDEBAR_W, WIN_H - list.bottom(), 0.0, th.bg, 1.0);
            if self.max_scroll > 0.0 {
                let bar_h = (list.h * list.h / total).max(30.0);
                let bar_y = list.y + (list.h - bar_h) * (self.scroll / self.max_scroll);
                cv.round_rect(list.right() + 6.0, bar_y, 5.0, bar_h, 2.5, th.sub, 0.5);
            }
        }

        // title row: search field and Clear
        let f = search_field(clear_w);
        cv.round_rect(f.x, f.y, f.w, f.h, 8.0, th.field, 1.0);
        if self.search_focus {
            cv.round_rect_stroke(f.x - 1.0, f.y - 1.0, f.w + 2.0, f.h + 2.0, 9.0, 2.5, th.accent, 0.85);
        } else {
            cv.round_rect_stroke(f.x, f.y, f.w, f.h, 8.0, 1.0, th.border, 1.0);
        }
        let c = Rect { x: f.right() + 10.0, y: f.y, w: clear_w, h: f.h };
        if any {
            cv.round_rect(c.x, c.y, c.w, c.h, 8.0, self.tint(th.card, Hit::Clear), 1.0);
            cv.round_rect_stroke(c.x, c.y, c.w, c.h, 8.0, 1.0, th.border, 1.0);
            hits.push((Hit::Clear, c));
        }
        texts.push(TextOp::new(t(T::HistoryClear), c, self.fonts.body, if any { th.danger } else { th.sub }, DT_CENTER | ONE_LINE));
        // keep the EDIT inside its painted field (the field moves with the Clear width)
        let s = self.scale;
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::MoveWindow(
                self.search,
                ((f.x + 10.0) * s) as i32,
                ((f.y + 6.0) * s) as i32,
                ((f.w - 20.0) * s) as i32,
                ((f.h - 12.0) * s) as i32,
                0,
            );
        }
    }
}

/// Colored tile with a white glyph, like the icons of macOS System Settings.
fn page_icon(cv: &mut Canvas, p: Page, x: f32, y: f32, selected: bool) {
    let tile = match p {
        Page::General => 0x8E8E93,
        Page::Recognition => 0x0A84FF,
        Page::App => 0xAF52DE,
        Page::History => 0xFF9F0A,
    };
    let (sz, w) = (22.0, 0xFFFFFF);
    cv.round_rect(x, y, sz, sz, 6.0, tile, 1.0);
    if selected {
        cv.round_rect_stroke(x, y, sz, sz, 6.0, 1.0, 0xFFFFFF, 0.6);
    }
    let (cx, cy) = (x + sz / 2.0, y + sz / 2.0);
    match p {
        Page::General => {
            // two sliders
            for (dy, knob) in [(-3.5, -2.5), (3.5, 2.5)] {
                cv.line((cx - 6.0, cy + dy), (cx + 6.0, cy + dy), 1.6, w, 1.0);
                cv.circle(cx + knob, cy + dy, 2.4, w, 1.0);
                cv.circle(cx + knob, cy + dy, 1.1, tile, 1.0);
            }
        }
        Page::Recognition => {
            // waveform
            for (i, hh) in [3.0, 6.5, 4.5, 7.5, 3.5].iter().enumerate() {
                let bx = cx - 6.0 + i as f32 * 3.0;
                cv.line((bx, cy - hh), (bx, cy + hh), 1.7, w, 1.0);
            }
        }
        Page::App => {
            for (dx, dy) in [(-3.2, -3.2), (3.2, -3.2), (-3.2, 3.2), (3.2, 3.2)] {
                cv.round_rect(cx + dx - 2.4, cy + dy - 2.4, 4.8, 4.8, 1.2, w, 1.0);
            }
        }
        Page::History => {
            cv.circle(cx, cy, 6.8, w, 1.0);
            cv.circle(cx, cy, 5.2, tile, 1.0);
            cv.line((cx, cy), (cx, cy - 3.8), 1.5, w, 1.0);
            cv.line((cx, cy), (cx + 2.8, cy + 1.2), 1.5, w, 1.0);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn segmented(
    cv: &mut Canvas,
    texts: &mut Vec<TextOp>,
    hits: &mut Vec<(Hit, Rect)>,
    th: Theme,
    fonts: &Fonts,
    right: f32,
    cy: f32,
    width: f32,
    labels: &[&str],
    pos: f32,
    hit: impl Fn(usize) -> Hit,
    hover: Option<Hit>,
) {
    let n = labels.len() as f32;
    let r = Rect { x: right - width, y: cy - 14.0, w: width, h: 28.0 };
    cv.round_rect(r.x, r.y, r.w, r.h, 8.0, th.control, 1.0);
    let seg_w = (r.w - 4.0) / n;
    // sliding thumb at the (animated) selected position
    let sx = r.x + 2.0 + pos * seg_w;
    cv.shadow(sx, r.y + 2.0, seg_w, r.h - 4.0, 6.5, 3.0, 1.0, 0x000000, if th.dark { 0.3 } else { 0.10 });
    cv.round_rect(sx, r.y + 2.0, seg_w, r.h - 4.0, 6.5, th.seg_sel, 1.0);
    for (i, l) in labels.iter().enumerate() {
        let sr = Rect { x: r.x + 2.0 + i as f32 * seg_w, y: r.y, w: seg_w, h: r.h };
        let selected = (pos - i as f32).abs() < 0.5;
        if !selected && hover == Some(hit(i)) {
            cv.round_rect(sr.x + 1.0, sr.y + 3.0, sr.w - 2.0, sr.h - 6.0, 6.0, mix(th.control, th.text, 0.06), 1.0);
        }
        if i > 0 {
            // hairline separators between unselected neighbours, like macOS
            let near = (pos - i as f32).abs() < 1.0 || (pos - (i as f32 - 1.0)).abs() < 1.0;
            let a = if near { 0.0 } else { 0.8 };
            cv.line((sr.x, r.y + 8.0), (sr.x, r.y + r.h - 8.0), 1.0, mix(th.control, th.sub, 0.5), a);
        }
        texts.push(TextOp::new(*l, sr, if selected { fonts.bold } else { fonts.body }, th.text, DT_CENTER | ONE_LINE));
        hits.push((hit(i), sr));
    }
}

fn popup_button(cv: &mut Canvas, texts: &mut Vec<TextOp>, th: Theme, fonts: &Fonts, r: Rect, label: &str, bg: u32) {
    cv.round_rect(r.x, r.y, r.w, r.h, 7.0, bg, 1.0);
    cv.round_rect_stroke(r.x, r.y, r.w, r.h, 7.0, 1.0, th.border, 1.0);
    // up/down chevrons in an accent badge, as on macOS pop-up buttons
    let bx = r.right() - 22.0;
    cv.round_rect(bx, r.y + 4.0, 18.0, r.h - 8.0, 5.0, th.accent, 1.0);
    let (cx, cy) = (bx + 9.0, r.y + r.h / 2.0);
    for (dy, dir) in [(-3.2, -1.0), (3.2, 1.0)] {
        let y0 = cy + dy - dir * 1.6;
        let y1 = cy + dy + dir * 1.6;
        cv.line((cx - 3.2, y0), (cx, y1), 1.6, 0xFFFFFF, 1.0);
        cv.line((cx, y1), (cx + 3.2, y0), 1.6, 0xFFFFFF, 1.0);
    }
    texts.push(TextOp::new(label, Rect { x: r.x + 12.0, y: r.y, w: r.w - 40.0, h: r.h }, fonts.body, th.text, DT_LEFT | ONE_LINE));
}

fn toggle(cv: &mut Canvas, th: Theme, r: Rect, v: f32, hover: bool) {
    let track = mix(th.toggle_off, th.accent, v);
    let track = if hover { mix(track, th.text, 0.05) } else { track };
    cv.round_rect(r.x, r.y, r.w, r.h, r.h / 2.0, track, 1.0);
    let kr = r.h / 2.0 - 2.0;
    let kx = r.x + 2.0 + kr + v * (r.w - 4.0 - 2.0 * kr);
    let ky = r.y + r.h / 2.0;
    cv.shadow(kx - kr, ky - kr, 2.0 * kr, 2.0 * kr, kr, 3.0, 1.0, 0x000000, 0.25);
    cv.circle(kx, ky, kr, 0xFFFFFF, 1.0);
}

// -------------------------------------------------------------- behaviour --

fn down(vk: i32) -> bool {
    (unsafe { GetAsyncKeyState(vk) } as u16) & 0x8000 != 0
}

fn edit_text(h: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h);
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32);
        String::from_utf16_lossy(&buf[..got.max(0) as usize])
    }
}

impl St {
    fn hit_at(&self, x: f32, y: f32) -> Option<Hit> {
        self.hits.iter().find(|(_, r)| r.contains(x, y)).map(|(h, _)| *h)
    }

    fn targets(&self) -> [(f32, f32); 6] {
        let b = |v: bool| if v { 1.0 } else { 0.0 };
        [
            (self.a_mode, b(self.mode == Mode::Toggle)),
            (self.a_overlay, overlay_index(self.overlay) as f32),
            (self.a_space, b(self.space)),
            (self.a_live, b(self.live)),
            (self.a_auto, b(self.autostart)),
            (self.a_clip, b(self.clipboard)),
        ]
    }

    fn start_anim(&mut self) {
        self.last_anim = Instant::now();
        unsafe { SetTimer(self.hwnd, TIMER_ANIM, 15, None) };
    }

    fn step_anim(&mut self) {
        let dt = self.last_anim.elapsed().as_secs_f32().min(0.05);
        self.last_anim = Instant::now();
        let k = (dt * 16.0).min(1.0);
        let tg = self.targets();
        let mut cur = [self.a_mode, self.a_overlay, self.a_space, self.a_live, self.a_auto, self.a_clip];
        let mut done = true;
        for (c, (_, target)) in cur.iter_mut().zip(tg) {
            *c += (target - *c) * k;
            if (target - *c).abs() < 0.002 {
                *c = target;
            } else {
                done = false;
            }
        }
        [self.a_mode, self.a_overlay, self.a_space, self.a_live, self.a_auto, self.a_clip] = cur;
        if done {
            unsafe { KillTimer(self.hwnd, TIMER_ANIM) };
        }
        self.invalidate();
    }

    fn activate(&mut self, hit: Hit) -> Action {
        match hit {
            Hit::Page(i) => {
                self.set_page(PAGES[i as usize % PAGES.len()]);
                return Action::None;
            }
            Hit::Key => {
                if matches!(self.capture, Capture::Off) {
                    self.capture = Capture::Waiting(Instant::now());
                    self.capture_bad = false;
                    HOTKEY_PAUSED.store(true, Ordering::Release);
                    unsafe { SetTimer(self.hwnd, TIMER_CAPTURE, 25, None) };
                }
            }
            Hit::Mode(i) => self.mode = if i == 1 { Mode::Toggle } else { Mode::Hold },
            Hit::Overlay(i) => self.overlay = [Overlay::Bottom, Overlay::Center, Overlay::Off][i as usize % 3],
            Hit::Space => self.space = !self.space,
            Hit::Clipboard => self.clipboard = !self.clipboard,
            Hit::Live => self.live = !self.live,
            Hit::Autostart => self.autostart = !self.autostart,
            Hit::Copy(i) => {
                if let Some(text) = self.shown.get(i) {
                    match clipboard::set_text(self.hwnd, text) {
                        Ok(()) => {
                            self.copied = Some((i, Instant::now()));
                            unsafe { SetTimer(self.hwnd, TIMER_COPIED, 1600, None) };
                        }
                        Err(e) => return Action::Error(e),
                    }
                }
                self.invalidate();
                return Action::None;
            }
            Hit::Clear => return Action::ClearHistory,
            Hit::SpeechLang | Hit::UiLang | Hit::HistoryMode => return Action::Popup(hit),
            Hit::File => return Action::OpenFile,
            Hit::Cancel => return Action::Close,
            Hit::Save => return Action::Save,
        }
        self.start_anim();
        Action::None
    }

    fn set_choice(&mut self, hit: Hit, code: String) {
        match hit {
            Hit::SpeechLang => self.speech = code,
            Hit::HistoryMode => self.history_mode = history::Mode::parse(&code).unwrap_or(self.history_mode),
            _ => {
                self.ui_lang = code;
                i18n::set(i18n::resolve(&self.ui_lang)); // preview right away
                let title = wide(t(T::SettingsTitle));
                let cue = wide(t(T::HistorySearch));
                unsafe {
                    SetWindowTextW(self.hwnd, title.as_ptr());
                    SendMessageW(self.search, EM_SETCUEBANNER, 1, cue.as_ptr() as isize);
                }
            }
        }
        self.invalidate();
    }

    fn current(&self, hit: Hit) -> (String, Vec<(String, String)>) {
        match hit {
            Hit::SpeechLang => (self.speech.clone(), speech_options()),
            Hit::HistoryMode => (self.history_mode.as_str().to_string(), history_options()),
            _ => (self.ui_lang.clone(), ui_options()),
        }
    }
}

/// Native popup menu under a popup button. Runs a modal loop, so it must be
/// called without the window state borrowed.
fn popup_menu(hwnd: HWND, scale: f32, r: Rect, options: &[(String, String)], current: &str) -> Option<String> {
    unsafe {
        let menu = CreatePopupMenu();
        for (i, (code, label)) in options.iter().enumerate() {
            if code == "auto" && i > 0 {
                AppendMenuW(menu, MF_SEPARATOR, 0, null());
            }
            let w = wide(label);
            let flags = MF_STRING | if code == current { MF_CHECKED } else { 0 };
            AppendMenuW(menu, flags, i + 1, w.as_ptr());
            if code == "auto" && i == 0 {
                AppendMenuW(menu, MF_SEPARATOR, 0, null());
            }
        }
        let mut pt = POINT { x: (r.x * scale) as i32, y: ((r.y + r.h + 4.0) * scale) as i32 };
        ClientToScreen(hwnd, &mut pt);
        let cmd = TrackPopupMenuEx(menu, TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN, pt.x, pt.y, hwnd, null());
        DestroyMenu(menu);
        (cmd > 0).then(|| options[cmd as usize - 1].0.clone())
    }
}

impl St {
    fn poll_capture(&mut self) {
        match self.capture {
            Capture::Off => {}
            Capture::Waiting(t0) => {
                if t0.elapsed().as_millis() < 250 {
                    return; // let the click that opened capture be released
                }
                if down(0x1B) {
                    self.capture = Capture::Releasing(0x1B);
                    self.invalidate();
                    return;
                }
                let keys = config::capturable_keys();
                if let Some(&vk) = keys.iter().find(|&&k| down(k)) {
                    self.vk = vk;
                    self.capture = Capture::Releasing(vk);
                    self.capture_bad = false;
                    self.invalidate();
                    return;
                }
                let other = (0x08..=0xFE).any(|k| !matches!(k, 0x10..=0x12) && !keys.contains(&k) && down(k));
                if other && !self.capture_bad {
                    self.capture_bad = true;
                    self.invalidate();
                }
            }
            Capture::Releasing(vk) => {
                if !down(vk) {
                    self.capture = Capture::Off;
                    HOTKEY_PAUSED.store(false, Ordering::Release);
                    unsafe { KillTimer(self.hwnd, TIMER_CAPTURE) };
                }
            }
        }
    }

    fn save(&self) -> Result<(), String> {
        let prompt = edit_text(self.edit).split_whitespace().collect::<Vec<_>>().join(" ");
        // Only if switched here, so a change made from the tray meanwhile isn't undone.
        if self.autostart != self.autostart_before {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            autostart::set(self.autostart, &exe)?;
        }
        config::save(
            &self.cfg_path,
            &[
                ("hotkey", config::key_name(self.vk)),
                ("mode", self.mode.as_str().to_string()),
                ("language", self.speech.clone()),
                ("overlay", self.overlay.as_str().to_string()),
                ("live_preview", self.live.to_string()),
                ("append_space", self.space.to_string()),
                ("clipboard", self.clipboard.to_string()),
                ("history", self.history_mode.as_str().to_string()),
                ("ui_language", self.ui_lang.clone()),
                ("prompt", prompt),
            ],
        )
    }
}

enum Action {
    None,
    Popup(Hit),
    OpenFile,
    ClearHistory,
    Error(String),
    Close,
    Save,
}

fn close(saved: bool) {
    let st = ST.with(|s| s.borrow().as_ref().map(|s| (s.hwnd, s.ui_lang_before)));
    if let Some((h, before)) = st {
        if !saved {
            i18n::set(before); // the language preview is undone on cancel
        }
        unsafe { DestroyWindow(h) };
    }
}

fn error_box(hwnd: HWND, e: &str) {
    let (m, c) = (wide(&format!("{}:\n{e}", t(T::SaveFailed))), wide(t(T::AppName)));
    unsafe { MessageBoxW(hwnd, m.as_ptr(), c.as_ptr(), MB_OK | MB_ICONERROR) };
}

fn run(action: Action) {
    match action {
        Action::None => {}
        Action::Popup(hit) => {
            let info = with(|s| {
                let r = s.hits.iter().find(|(h, _)| *h == hit).map(|x| x.1);
                (s.hwnd, s.scale, r, s.current(hit))
            });
            if let Some((hwnd, scale, Some(r), (current, options))) = info
                && let Some(code) = popup_menu(hwnd, scale, r, &options, &current)
            {
                with(|s| s.set_choice(hit, code));
            }
        }
        Action::OpenFile => {
            if let Some(p) = ST.with(|s| s.borrow().as_ref().map(|s| s.cfg_path.clone())) {
                crate::open_path(&p);
            }
        }
        Action::ClearHistory => {
            let Some(hwnd) = hwnd() else { return };
            let (m, c) = (wide(t(T::HistoryClearAsk)), wide(t(T::AppName)));
            let answer = unsafe { MessageBoxW(hwnd, m.as_ptr(), c.as_ptr(), MB_YESNO | MB_ICONWARNING) };
            if answer != IDNO && answer != 0 {
                crate::history().clear();
                with(|s| {
                    s.scroll = 0.0;
                    s.invalidate();
                });
            }
        }
        Action::Error(e) => {
            if let Some(h) = hwnd() {
                error_box(h, &e);
            }
        }
        Action::Close => close(false),
        Action::Save => {
            let res = ST.with(|s| s.borrow().as_ref().map(|s| (s.save(), s.on_saved, s.hwnd)));
            match res {
                Some((Ok(()), on_saved, _)) => {
                    close(true);
                    on_saved();
                }
                Some((Err(e), _, hwnd)) => error_box(hwnd, &e),
                None => {}
            }
        }
    }
}

fn mouse_xy(st: &St, lp: LPARAM) -> (f32, f32) {
    let x = (lp & 0xFFFF) as i16 as f32 / st.scale;
    let y = ((lp >> 16) & 0xFFFF) as i16 as f32 / st.scale;
    (x, y)
}

unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            unsafe {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let hdc = BeginPaint(hwnd, &mut ps);
                with(|s| s.paint(hdc));
                EndPaint(hwnd, &ps);
            }
            0
        }
        WM_ERASEBKGND => 1,
        WM_MOUSEMOVE => {
            with(|s| {
                if !s.tracking {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    unsafe { TrackMouseEvent(&mut tme) };
                    s.tracking = true;
                }
                let (x, y) = mouse_xy(s, lp);
                let h = s.hit_at(x, y);
                if h != s.hover {
                    s.hover = h;
                    s.invalidate();
                }
            });
            0
        }
        WM_MOUSELEAVE => {
            with(|s| {
                s.tracking = false;
                if s.hover.take().is_some() {
                    s.invalidate();
                }
            });
            0
        }
        WM_MOUSEWHEEL => {
            with(|s| {
                if s.page == Page::History {
                    let delta = ((wp >> 16) & 0xFFFF) as i16 as f32 / 120.0;
                    let before = s.scroll;
                    s.scroll = (s.scroll - delta * 60.0).clamp(0.0, s.max_scroll);
                    if s.scroll != before {
                        s.hover = None;
                        s.invalidate();
                    }
                }
            });
            0
        }
        WM_LBUTTONDOWN => {
            with(|s| {
                let (x, y) = mouse_xy(s, lp);
                s.pressed = s.hit_at(x, y);
                unsafe {
                    SetCapture(hwnd);
                    SetFocus(hwnd); // clicking outside a text field leaves it, like on macOS
                }
                s.invalidate();
            });
            0
        }
        WM_LBUTTONUP => {
            unsafe { ReleaseCapture() };
            let action = with(|s| {
                let (x, y) = mouse_xy(s, lp);
                let released_on = s.hit_at(x, y);
                let pressed = s.pressed.take();
                s.invalidate();
                match (pressed, released_on) {
                    (Some(p), Some(r)) if p == r => s.activate(p),
                    _ => Action::None,
                }
            });
            if let Some(a) = action {
                run(a);
            }
            0
        }
        WM_TIMER => {
            match wp {
                TIMER_CAPTURE => {
                    with(|s| s.poll_capture());
                }
                TIMER_ANIM => {
                    with(|s| s.step_anim());
                }
                TIMER_COPIED => {
                    unsafe { KillTimer(hwnd, TIMER_COPIED) };
                    with(|s| s.invalidate());
                }
                _ => {}
            }
            0
        }
        WM_COMMAND => {
            let id = wp & 0xFFFF;
            let code = ((wp >> 16) & 0xFFFF) as u32;
            if id == ID_PROMPT && (code == EN_SETFOCUS || code == EN_KILLFOCUS) {
                with(|s| {
                    s.edit_focus = code == EN_SETFOCUS;
                    s.invalidate();
                });
            } else if id == ID_SEARCH && (code == EN_SETFOCUS || code == EN_KILLFOCUS) {
                with(|s| {
                    s.search_focus = code == EN_SETFOCUS;
                    s.invalidate();
                });
            } else if id == ID_SEARCH && code == EN_CHANGE {
                with(|s| {
                    s.query = edit_text(s.search).trim().to_string();
                    s.scroll = 0.0;
                    s.invalidate();
                });
            } else if id == IDOK as usize {
                // Enter saves, except on the history page (it has nothing to save)
                if with(|s| s.page != Page::History).unwrap_or(false) {
                    run(Action::Save);
                }
            } else if id == IDCANCEL as usize {
                run(Action::Close);
            }
            0
        }
        WM_CTLCOLOREDIT => {
            let hdc = wp as HDC;
            with(|s| unsafe {
                SetTextColor(hdc, colorref(s.th.text));
                SetBkColor(hdc, colorref(s.th.field));
                s.field_brush as LRESULT
            })
            .unwrap_or(0)
        }
        WM_SETTINGCHANGE => {
            // light/dark theme switched while the window is open
            with(|s| {
                let dark = system_dark();
                if dark != s.th.dark {
                    s.th = theme(dark);
                    unsafe {
                        DeleteObject(s.field_brush);
                        s.field_brush = CreateSolidBrush(colorref(s.th.field));
                        InvalidateRect(s.edit, null(), 1);
                        InvalidateRect(s.search, null(), 1);
                    }
                    apply_titlebar(s.hwnd, s.th);
                    s.invalidate();
                }
            });
            0
        }
        WM_CLOSE => {
            close(false);
            0
        }
        WM_DESTROY => {
            HOTKEY_PAUSED.store(false, Ordering::Release);
            if let Some(st) = ST.with(|s| s.borrow_mut().take()) {
                unsafe {
                    for f in [st.fonts.body, st.fonts.bold, st.fonts.sub, st.fonts.section, st.fonts.title] {
                        DeleteObject(f);
                    }
                    DeleteObject(st.field_brush);
                }
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_fits() {
        let f = prompt_field();
        assert!(f.w > 400.0 && f.y > content_top());
        for p in PAGES {
            assert!(page_height(p) + 4.0 <= footer_y(), "{p:?} runs into the footer");
        }
        let l = list_rect();
        assert!(l.h > 300.0 && l.bottom() <= WIN_H);
        assert!(search_field(90.0).x > CARD_X + 120.0, "search must leave room for the page title");
    }

    #[test]
    fn pages_roundtrip() {
        for p in PAGES {
            assert!(!t(p.title()).is_empty());
            assert_eq!(Page::from_index(p.index()), p);
            assert_eq!(Page::parse(&format!("{p:?}").to_lowercase()), Some(p));
        }
        assert_eq!(Page::parse("nope"), None);
    }
}
