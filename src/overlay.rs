//! The dictation bubble window: layered, topmost, click-through, never takes focus.
//! Lives on the UI thread; animates with a 60 Hz timer only while visible.
//! The live-preview caption is drawn with GDI into a grayscale mask, then
//! composited by the software renderer in `dictate::bubble`.

use std::cell::RefCell;
use std::ptr::{null, null_mut};
use std::time::Instant;

use dictate::bubble::{self, CAP_TEXT_W, Caption, Frame, N_BARS, PILL_H, Phase};
use dictate::config::Overlay;
use dictate::surface::{Dib, TextSurface, draw_caption, make_dib, make_text_surface};
use dictate::{audio, wide};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetCursorPos, HWND_TOPMOST, KillTimer, RegisterClassW, SW_HIDE,
    SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetTimer, SetWindowPos, ShowWindow, ULW_ALPHA,
    UpdateLayeredWindow, WM_TIMER, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    Hidden,
    Show(Phase),
}

struct Ov {
    hwnd: HWND,
    mode: Overlay,
    target: Target,
    phase: Phase,
    appear: f32,
    visible: bool,
    shown_at: Instant,
    last_tick: Instant,
    hist: [f32; N_BARS / 2 + 1],
    smooth: f32,
    last_push: Instant,
    dib: Option<Dib>,
    text: Option<TextSurface>,
    scale: f32,
    pos: POINT,
    caption_text: String,
    mask: Vec<u8>,
    mask_w: i32,
    mask_h: i32,
    cap_appear: f32,
}

thread_local! {
    static OV: RefCell<Option<Ov>> = const { RefCell::new(None) };
}

const TIMER_ID: usize = 1;

pub fn init(mode: Overlay) {
    if mode == Overlay::Off {
        return;
    }
    unsafe {
        let hinst = GetModuleHandleW(null());
        let class = wide("dictate-bubble");
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(proc_),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: null_mut(),
            hCursor: null_mut(),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc);
        let ex = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        let hwnd = CreateWindowExW(ex, class.as_ptr(), null(), WS_POPUP, 0, 0, 1, 1, null_mut(), null_mut(), hinst, null());
        if hwnd.is_null() {
            return;
        }
        let now = Instant::now();
        OV.with(|o| {
            *o.borrow_mut() = Some(Ov {
                hwnd,
                mode,
                target: Target::Hidden,
                phase: Phase::Recording,
                appear: 0.0,
                visible: false,
                shown_at: now,
                last_tick: now,
                hist: [0.0; N_BARS / 2 + 1],
                smooth: 0.0,
                last_push: now,
                dib: None,
                text: None,
                scale: 1.0,
                pos: POINT { x: 0, y: 0 },
                caption_text: String::new(),
                mask: Vec::new(),
                mask_w: 0,
                mask_h: 0,
                cap_appear: 0.0,
            })
        });
    }
}

pub fn set_target(target: Target) {
    with(|ov| ov.set_target(target));
}

/// Live preview text shown next to the bubble ("" hides the caption).
pub fn set_caption(text: &str) {
    with(|ov| ov.set_caption(text));
}

fn with(f: impl FnOnce(&mut Ov)) {
    OV.with(|o| {
        if let Some(ov) = o.borrow_mut().as_mut() {
            f(ov);
        }
    });
}

unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_TIMER && wp == TIMER_ID {
        with(|ov| ov.tick());
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn ease_out_cubic(x: f32) -> f32 {
    1.0 - (1.0 - x.clamp(0.0, 1.0)).powi(3)
}

impl Ov {
    fn caption_above(&self) -> bool {
        self.mode == Overlay::Bottom
    }

    fn set_target(&mut self, target: Target) {
        self.target = target;
        if let Target::Show(p) = target {
            self.phase = p;
            if !self.visible {
                self.place();
                self.visible = true;
                self.appear = 0.0;
                self.hist = [0.0; N_BARS / 2 + 1];
                self.smooth = 0.0;
                self.caption_text.clear();
                self.mask_w = 0;
                self.mask_h = 0;
                self.cap_appear = 0.0;
                self.shown_at = Instant::now();
                self.last_tick = Instant::now();
                self.tick();
                unsafe {
                    ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                    SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                    SetTimer(self.hwnd, TIMER_ID, 15, None);
                }
            }
        }
    }

    fn set_caption(&mut self, text: &str) {
        if !self.visible || text == self.caption_text {
            return;
        }
        self.caption_text = text.to_string();
        if text.is_empty() {
            return; // keep the old mask while it fades out
        }
        if let Some(ts) = self.text.as_mut() {
            let (mask, w, h) = draw_caption(ts, text, (CAP_TEXT_W * self.scale) as i32);
            self.mask = mask;
            self.mask_w = w;
            self.mask_h = h;
        }
    }

    /// Positions the window on the monitor under the mouse: pill at the bottom
    /// (caption above it) or pill at the screen center (caption below it).
    fn place(&mut self) {
        unsafe {
            let mut pt = POINT { x: 0, y: 0 };
            GetCursorPos(&mut pt);
            let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
            let mut mi: MONITORINFO = std::mem::zeroed();
            mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            GetMonitorInfoW(mon, &mut mi);
            let (mut dx, mut dy) = (96u32, 96u32);
            GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
            let scale = dx as f32 / 96.0;
            let (w, h) = bubble::size(scale);
            if self.dib.as_ref().is_none_or(|d| d.w != w || d.h != h) {
                self.dib = make_dib(w, h);
            }
            if self.text.is_none() || (self.scale - scale).abs() > 0.01 {
                self.text = make_text_surface(scale);
            }
            self.scale = scale;
            let wa = mi.rcWork;
            let x = (wa.left + wa.right) / 2 - w / 2;
            let (_, pcy) = bubble::pill_center(self.caption_above());
            let y = match self.mode {
                Overlay::Center => (wa.top + wa.bottom) / 2 - (pcy * scale) as i32,
                // pill bottom 28 px above the taskbar, like before the caption existed
                _ => wa.bottom - ((28.0 + pcy + PILL_H / 2.0) * scale) as i32,
            };
            self.pos = POINT { x, y };
        }
    }

    fn tick(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f32().min(0.1);
        self.last_tick = now;

        match self.target {
            Target::Show(_) => self.appear = (self.appear + dt / 0.16).min(1.0),
            Target::Hidden => self.appear = (self.appear - dt / 0.18).max(0.0),
        }
        let want_caption = !self.caption_text.is_empty() && self.mask_w > 0;
        self.cap_appear = if want_caption {
            (self.cap_appear + dt / 0.18).min(1.0)
        } else {
            (self.cap_appear - dt / 0.18).max(0.0)
        };

        let lvl = audio::level();
        self.smooth = if lvl > self.smooth { lvl } else { self.smooth * 0.88 + lvl * 0.12 };
        if now.duration_since(self.last_push).as_millis() >= 45 {
            self.last_push = now;
            self.hist.rotate_right(1);
            self.hist[0] = self.smooth;
        }
        let mid = N_BARS / 2;
        let bars: Vec<f32> = (0..N_BARS).map(|i| self.hist[i.abs_diff(mid)]).collect();

        let caption_above = self.caption_above();
        let caption = (self.cap_appear > 0.0 && self.mask_w > 0).then(|| Caption {
            mask: &self.mask,
            w: self.mask_w,
            h: self.mask_h,
            appear: ease_out_cubic(self.cap_appear),
        });
        if let Some(d) = self.dib.as_mut() {
            let (w, h) = (d.w, d.h);
            let frame = Frame {
                phase: self.phase,
                t: now.duration_since(self.shown_at).as_secs_f32(),
                appear: ease_out_cubic(self.appear),
                bars: &bars,
                caption,
                caption_above,
            };
            bubble::render(d.pixels(), w, h, self.scale, &frame);
            let size = SIZE { cx: w, cy: h };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            unsafe { UpdateLayeredWindow(self.hwnd, null_mut(), &self.pos, &size, d.dc, &src, 0, &blend, ULW_ALPHA) };
        }

        if self.target == Target::Hidden && self.appear <= 0.0 && self.visible {
            self.visible = false;
            unsafe {
                KillTimer(self.hwnd, TIMER_ID);
                ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }
}

