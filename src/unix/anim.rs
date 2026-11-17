//! The bubble's animation state without a window (the same behavior as the
//! Windows overlay): fade in and out, the level meter history, and the live caption.
//! The platform shell positions a window, calls `tick` at ~60 Hz while `visible`,
//! and shows `buf` (premultiplied BGRA, `w`×`h`).

use std::time::Instant;

use dictate::bubble::{self, CAP_TEXT_W, Caption, Frame, N_BARS, PILL_H, Phase};
use dictate::caption::{TextSurface, draw_caption, make_text_surface};
use dictate::audio;
use dictate::config::Overlay;

pub struct Anim {
    pub mode: Overlay,
    target: Option<Phase>,
    phase: Phase,
    appear: f32,
    visible: bool,
    shown_at: Instant,
    last_tick: Instant,
    hist: [f32; N_BARS / 2 + 1],
    smooth: f32,
    last_push: Instant,
    text: Option<TextSurface>,
    scale: f32,
    caption_text: String,
    mask: Vec<u8>,
    mask_w: i32,
    mask_h: i32,
    cap_appear: f32,
    pub buf: Vec<u32>,
    pub w: i32,
    pub h: i32,
}

fn ease_out_cubic(x: f32) -> f32 {
    1.0 - (1.0 - x.clamp(0.0, 1.0)).powi(3)
}

impl Anim {
    pub fn new(mode: Overlay) -> Self {
        let now = Instant::now();
        Self {
            mode,
            target: None,
            phase: Phase::Recording,
            appear: 0.0,
            visible: false,
            shown_at: now,
            last_tick: now,
            hist: [0.0; N_BARS / 2 + 1],
            smooth: 0.0,
            last_push: now,
            text: None,
            scale: 0.0,
            caption_text: String::new(),
            mask: Vec::new(),
            mask_w: 0,
            mask_h: 0,
            cap_appear: 0.0,
            buf: Vec::new(),
            w: 0,
            h: 0,
        }
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    fn caption_above(&self) -> bool {
        self.mode == Overlay::Bottom
    }

    /// Returns true when the bubble was hidden and must now be placed and shown
    /// (call `set_scale`, position the window, then `tick`).
    pub fn set_target(&mut self, target: Option<Phase>) -> bool {
        self.target = target;
        let Some(p) = target else { return false };
        self.phase = p;
        if self.visible {
            return false;
        }
        self.visible = true;
        self.appear = 0.0;
        self.hist = [0.0; N_BARS / 2 + 1];
        self.smooth = 0.0;
        self.caption_text.clear();
        self.mask_w = 0;
        self.mask_h = 0;
        self.cap_appear = 0.0;
        self.shown_at = Instant::now();
        self.last_tick = self.shown_at;
        true
    }

    /// Pixel size of the window for this display scale.
    pub fn set_scale(&mut self, scale: f32) {
        if (self.scale - scale).abs() > 0.01 || self.text.is_none() {
            self.text = make_text_surface(scale);
            if self.text.is_none() {
                dictate::log::write("caption: no system font with Cyrillic found; live text is off");
            }
        }
        self.scale = scale;
        let (w, h) = bubble::size(scale);
        if (w, h) != (self.w, self.h) {
            self.w = w;
            self.h = h;
            self.buf = vec![0; (w * h) as usize];
        }
    }

    /// Top-left corner of the window inside the work area (same units as `w`, `h`):
    /// the pill at the bottom (caption above it) or at the center (caption below it).
    pub fn position(&self, left: i32, top: i32, right: i32, bottom: i32) -> (i32, i32) {
        let x = (left + right) / 2 - self.w / 2;
        let (_, pcy) = bubble::pill_center(self.caption_above());
        let y = match self.mode {
            Overlay::Center => (top + bottom) / 2 - (pcy * self.scale) as i32,
            // pill bottom 28 px above the bottom edge of the work area
            _ => bottom - ((28.0 + pcy + PILL_H / 2.0) * self.scale) as i32,
        };
        (x, y)
    }

    pub fn set_caption(&mut self, text: &str) {
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

    /// Renders the next frame into `buf`. Returns false once the bubble has faded
    /// out: hide the window then.
    pub fn tick(&mut self) -> bool {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f32().min(0.1);
        self.last_tick = now;
        match self.target {
            Some(_) => self.appear = (self.appear + dt / 0.16).min(1.0),
            None => self.appear = (self.appear - dt / 0.18).max(0.0),
        }
        let want_caption = !self.caption_text.is_empty() && self.mask_w > 0;
        self.cap_appear =
            if want_caption { (self.cap_appear + dt / 0.18).min(1.0) } else { (self.cap_appear - dt / 0.18).max(0.0) };

        let lvl = audio::level();
        self.smooth = if lvl > self.smooth { lvl } else { self.smooth * 0.88 + lvl * 0.12 };
        if now.duration_since(self.last_push).as_millis() >= 45 {
            self.last_push = now;
            self.hist.rotate_right(1);
            self.hist[0] = self.smooth;
        }
        let mid = N_BARS / 2;
        let bars: Vec<f32> = (0..N_BARS).map(|i| self.hist[i.abs_diff(mid)]).collect();
        let caption = (self.cap_appear > 0.0 && self.mask_w > 0).then(|| Caption {
            mask: &self.mask,
            w: self.mask_w,
            h: self.mask_h,
            appear: ease_out_cubic(self.cap_appear),
        });
        if self.w > 0 {
            let frame = Frame {
                phase: self.phase,
                t: now.duration_since(self.shown_at).as_secs_f32(),
                appear: ease_out_cubic(self.appear),
                bars: &bars,
                caption,
                caption_above: self.caption_above(),
            };
            bubble::render(&mut self.buf, self.w, self.h, self.scale, &frame);
        }
        if self.target.is_none() && self.appear <= 0.0 {
            self.visible = false;
        }
        self.visible
    }
}
