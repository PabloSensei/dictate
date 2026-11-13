//! Software renderer for the dictation bubble: a dark pill with a soft shadow,
//! a status dot and a row of rounded level bars, plus an optional caption panel
//! with the live preview text. Pure math, no GDI (the caption text arrives as a
//! grayscale coverage mask), so it can be unit-tested and rendered to a file.
//! Output is premultiplied BGRA (0xAARRGGBB).

/// Logical (96-DPI) geometry.
pub const PILL_W: f32 = 112.0;
pub const PILL_H: f32 = 36.0;
/// Room around shapes for the shadow and the pop-in animation.
pub const MARGIN: f32 = 18.0;
pub const N_BARS: usize = 11;
/// Caption panel: text area limits and padding.
pub const CAP_TEXT_W: f32 = 520.0;
pub const CAP_LINE_H: f32 = 21.0;
pub const CAP_LINES: usize = 3;
pub const CAP_PAD_X: f32 = 16.0;
pub const CAP_PAD_Y: f32 = 10.0;
const GAP: f32 = 10.0;
const CAP_R: f32 = 14.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Recording,
    Busy,
}

/// Rendered caption text: coverage 0..255, `w`×`h` physical pixels.
pub struct Caption<'a> {
    pub mask: &'a [u8],
    pub w: i32,
    pub h: i32,
    /// 0..1 fade of the caption panel itself.
    pub appear: f32,
}

pub struct Frame<'a> {
    pub phase: Phase,
    /// Seconds since the bubble appeared (drives the idle motion).
    pub t: f32,
    /// 0 = hidden, 1 = fully shown (already eased).
    pub appear: f32,
    /// Bar heights 0..1, `N_BARS` values, used while recording.
    pub bars: &'a [f32],
    pub caption: Option<Caption<'a>>,
    /// Caption above the pill (when the bubble sits at the bottom of the screen).
    pub caption_above: bool,
}

fn caption_box_h() -> f32 {
    CAP_LINE_H * CAP_LINES as f32 + 2.0 * CAP_PAD_Y
}

/// Window size in physical pixels (fixed, so the window never moves while text grows).
pub fn size(scale: f32) -> (i32, i32) {
    let w = CAP_TEXT_W + 2.0 * CAP_PAD_X + 2.0 * MARGIN;
    let h = PILL_H + GAP + caption_box_h() + 2.0 * MARGIN;
    ((w * scale).ceil() as i32, (h * scale).ceil() as i32)
}

/// Pill center in logical units within the window.
pub fn pill_center(caption_above: bool) -> (f32, f32) {
    let w = CAP_TEXT_W + 2.0 * CAP_PAD_X + 2.0 * MARGIN;
    let h = PILL_H + GAP + caption_box_h() + 2.0 * MARGIN;
    let y = if caption_above { h - MARGIN - PILL_H / 2.0 } else { MARGIN + PILL_H / 2.0 };
    (w / 2.0, y)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn sd_round_rect(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = (px - cx).abs() - (hw - r);
    let qy = (py - cy).abs() - (hh - r);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// Premultiplied RGBA accumulator.
#[derive(Clone, Copy)]
struct Px {
    r: f32,
    g: f32,
    b: f32,
    a: f32,
}

impl Px {
    fn unpack(p: u32) -> Px {
        let c = |s: u32| ((p >> s) & 0xFF) as f32 / 255.0;
        Px { r: c(16), g: c(8), b: c(0), a: c(24) }
    }
    fn over(&mut self, (r, g, b): (f32, f32, f32), a: f32) {
        if a <= 0.0 {
            return;
        }
        let k = 1.0 - a;
        self.r = r * a + self.r * k;
        self.g = g * a + self.g * k;
        self.b = b * a + self.b * k;
        self.a = a + self.a * k;
    }
    fn pack(self) -> u32 {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
        (c(self.a) << 24) | (c(self.r) << 16) | (c(self.g) << 8) | c(self.b)
    }
}

const BG: (f32, f32, f32) = (0.086, 0.086, 0.098);
const WHITE: (f32, f32, f32) = (1.0, 1.0, 1.0);
const RED: (f32, f32, f32) = (1.0, 0.271, 0.227);
const BLUE: (f32, f32, f32) = (0.40, 0.67, 1.0);
const TEXT: (f32, f32, f32) = (0.93, 0.93, 0.95);

/// Bar heights for the "recognizing" state: a calm travelling wave.
fn busy_bar(i: usize, t: f32) -> f32 {
    0.10 + 0.45 * (0.5 + 0.5 * (t * 6.5 - i as f32 * 0.6).sin()).powi(2)
}

pub fn render(buf: &mut [u32], w: i32, h: i32, scale: f32, f: &Frame) {
    buf.fill(0);
    let e = f.appear.clamp(0.0, 1.0);
    if e <= 0.0 {
        return;
    }
    if let Some(c) = &f.caption {
        render_caption(buf, w, h, scale, f, c, e);
    }
    render_pill(buf, w, h, scale, f, e);
}

/// Iterates the physical pixels of a logical box (plus shadow room), clipped to the window.
fn for_box(w: i32, h: i32, scale: f32, (x0, y0, x1, y1): (f32, f32, f32, f32), mut f: impl FnMut(i32, i32)) {
    let px0 = ((x0 - 14.0) * scale).floor().max(0.0) as i32;
    let py0 = ((y0 - 14.0) * scale).floor().max(0.0) as i32;
    let px1 = (((x1 + 14.0) * scale).ceil() as i32).min(w);
    let py1 = (((y1 + 18.0) * scale).ceil() as i32).min(h);
    for y in py0..py1 {
        for x in px0..px1 {
            f(x, y);
        }
    }
}

fn render_pill(buf: &mut [u32], w: i32, h: i32, scale: f32, f: &Frame, e: f32) {
    let (cx, cy) = pill_center(f.caption_above);
    let k = 0.82 + 0.18 * e; // pop-in scale
    let rise = (1.0 - e) * 8.0;
    let ps = scale * k;

    let bars_x0 = cx - PILL_W / 2.0 + 34.0;
    let bars_x1 = cx + PILL_W / 2.0 - 16.0;
    let step = (bars_x1 - bars_x0) / (N_BARS - 1) as f32;
    let bar_r = 1.4;
    let heights: Vec<f32> = (0..N_BARS)
        .map(|i| {
            let v = match f.phase {
                Phase::Recording => {
                    let v = f.bars.get(i).copied().unwrap_or(0.0);
                    (v * (0.88 + 0.12 * (f.t * 9.0 + i as f32 * 1.7).sin())).max(0.07)
                }
                Phase::Busy => busy_bar(i, f.t),
            };
            1.6 + 9.4 * v
        })
        .collect();
    let (dot_c, dot_a) = match f.phase {
        Phase::Recording => (RED, 0.75 + 0.25 * (f.t * 4.0).sin()),
        Phase::Busy => (BLUE, 0.55 + 0.45 * (f.t * 7.0).sin().abs()),
    };
    let bar_c = match f.phase {
        Phase::Recording => WHITE,
        Phase::Busy => (0.80, 0.88, 1.0),
    };

    let (x0, x1) = (cx - PILL_W / 2.0, cx + PILL_W / 2.0);
    let (y0, y1) = (cy - PILL_H / 2.0, cy + PILL_H / 2.0 + 8.0);
    for_box(w, h, scale, (x0, y0, x1, y1), |x, y| {
        let lx = cx + ((x as f32 + 0.5) / scale - cx) / k;
        let ly = cy + ((y as f32 + 0.5) / scale - cy - rise) / k;
        let mut p = Px { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };

        let ds = sd_round_rect(lx, ly - 3.0, cx, cy, PILL_W / 2.0, PILL_H / 2.0, PILL_H / 2.0);
        p.over((0.0, 0.0, 0.0), 0.30 * (1.0 - smoothstep(-6.0, 12.0, ds)));

        let d = sd_round_rect(lx, ly, cx, cy, PILL_W / 2.0, PILL_H / 2.0, PILL_H / 2.0);
        let cov = (0.5 - d * ps).clamp(0.0, 1.0);
        if cov > 0.0 {
            p.over(BG, 0.94 * cov);
            let edge = (1.0 - (d * ps + 1.0).abs()).clamp(0.0, 1.0);
            p.over(WHITE, 0.13 * edge * cov);

            let dd = (lx - (cx - PILL_W / 2.0 + 19.0)).hypot(ly - cy) - 4.2;
            p.over(dot_c, dot_a * (0.5 - dd * ps).clamp(0.0, 1.0));
            p.over(dot_c, 0.18 * dot_a * (1.0 - smoothstep(0.0, 6.0, dd)));

            let i = ((lx - bars_x0) / step).round();
            if (0.0..N_BARS as f32).contains(&i) {
                let i = i as usize;
                let bx = bars_x0 + i as f32 * step;
                let hh = heights[i];
                let dy = ((ly - cy).abs() - (hh - bar_r)).max(0.0);
                let db = (lx - bx).hypot(dy) - bar_r;
                p.over(bar_c, 0.95 * (0.5 - db * ps).clamp(0.0, 1.0));
            }
        }
        if p.a > 0.0 {
            blend_into(buf, w, x, y, p, e);
        }
    });
}

fn render_caption(buf: &mut [u32], w: i32, h: i32, scale: f32, f: &Frame, c: &Caption, e: f32) {
    let ca = (c.appear * e).clamp(0.0, 1.0);
    if ca <= 0.0 || c.w <= 0 || c.h <= 0 {
        return;
    }
    let (cx, pcy) = pill_center(f.caption_above);
    // panel size follows the text, in logical units
    let tw = c.w as f32 / scale;
    let th = c.h as f32 / scale;
    let bw = (tw + 2.0 * CAP_PAD_X).max(PILL_W);
    let bh = th + 2.0 * CAP_PAD_Y;
    let rise = (1.0 - ca) * if f.caption_above { 6.0 } else { -6.0 };
    let cy = if f.caption_above {
        pcy - PILL_H / 2.0 - GAP - bh / 2.0
    } else {
        pcy + PILL_H / 2.0 + GAP + bh / 2.0
    } + rise;
    let r = CAP_R.min(bh / 2.0);
    // text origin in physical pixels
    let tx0 = ((cx - tw / 2.0) * scale).round() as i32;
    let ty0 = ((cy - th / 2.0) * scale).round() as i32;

    for_box(w, h, scale, (cx - bw / 2.0, cy - bh / 2.0, cx + bw / 2.0, cy + bh / 2.0), |x, y| {
        let lx = (x as f32 + 0.5) / scale;
        let ly = (y as f32 + 0.5) / scale;
        let mut p = Px { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };
        let ds = sd_round_rect(lx, ly - 3.0, cx, cy, bw / 2.0, bh / 2.0, r);
        p.over((0.0, 0.0, 0.0), 0.28 * (1.0 - smoothstep(-6.0, 12.0, ds)));
        let d = sd_round_rect(lx, ly, cx, cy, bw / 2.0, bh / 2.0, r);
        let cov = (0.5 - d * scale).clamp(0.0, 1.0);
        if cov > 0.0 {
            p.over(BG, 0.90 * cov);
            let edge = (1.0 - (d * scale + 1.0).abs()).clamp(0.0, 1.0);
            p.over(WHITE, 0.10 * edge * cov);
            let (mx, my) = (x - tx0, y - ty0);
            if mx >= 0 && my >= 0 && mx < c.w && my < c.h {
                let m = c.mask[(my * c.w + mx) as usize] as f32 / 255.0;
                p.over(TEXT, m * cov);
            }
        }
        if p.a > 0.0 {
            blend_into(buf, w, x, y, p, ca);
        }
    });
}

/// Source-over of a premultiplied pixel (scaled by `opacity`) onto the buffer.
fn blend_into(buf: &mut [u32], w: i32, x: i32, y: i32, src: Px, opacity: f32) {
    let i = (y * w + x) as usize;
    let mut dst = Px::unpack(buf[i]);
    let a = src.a * opacity;
    let k = 1.0 - a;
    dst.r = src.r * opacity + dst.r * k;
    dst.g = src.g * opacity + dst.g * k;
    dst.b = src.b * opacity + dst.b * k;
    dst.a = a + dst.a * k;
    buf[i] = dst.pack();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame<'a>(bars: &'a [f32], caption: Option<Caption<'a>>, above: bool) -> Frame<'a> {
        Frame { phase: Phase::Recording, t: 0.3, appear: 1.0, bars, caption, caption_above: above }
    }

    fn premultiplied(buf: &[u32]) -> bool {
        buf.iter().all(|&p| {
            let a = p >> 24;
            (p >> 16 & 0xFF) <= a && (p >> 8 & 0xFF) <= a && (p & 0xFF) <= a
        })
    }

    #[test]
    fn pill_opaque_corners_clear() {
        let s = 1.5;
        let (w, h) = size(s);
        let mut buf = vec![0u32; (w * h) as usize];
        let bars = [0.5f32; N_BARS];
        render(&mut buf, w, h, s, &frame(&bars, None, false));
        let (cx, cy) = pill_center(false);
        let at = |x: f32, y: f32| buf[((y * s) as i32 * w + (x * s) as i32) as usize];
        assert_eq!(buf[0] >> 24, 0, "corner must be transparent");
        assert!(at(cx, cy) >> 24 > 200, "pill center must be opaque");
        assert!(premultiplied(&buf));
    }

    #[test]
    fn caption_below_and_above() {
        let s = 1.0;
        let (w, h) = size(s);
        let mask = vec![255u8; 200 * 20];
        for above in [false, true] {
            let mut buf = vec![0u32; (w * h) as usize];
            let cap = Caption { mask: &mask, w: 200, h: 20, appear: 1.0 };
            render(&mut buf, w, h, s, &frame(&[0.2; N_BARS], Some(cap), above));
            let (cx, pcy) = pill_center(above);
            let probe_y = if above { pcy - PILL_H / 2.0 - GAP - 20.0 } else { pcy + PILL_H / 2.0 + GAP + 20.0 };
            let p = buf[(probe_y as i32 * w + cx as i32) as usize];
            assert!(p >> 24 > 200, "caption text must be drawn (above={above})");
            assert!(p & 0xFF > 200, "text is light");
            assert!(premultiplied(&buf));
        }
    }

    #[test]
    fn hidden_is_fully_transparent() {
        let (w, h) = size(1.0);
        let mut buf = vec![1u32; (w * h) as usize];
        let f = Frame { phase: Phase::Busy, t: 0.0, appear: 0.0, bars: &[], caption: None, caption_above: false };
        render(&mut buf, w, h, 1.0, &f);
        assert!(buf.iter().all(|&p| p == 0));
    }
}
