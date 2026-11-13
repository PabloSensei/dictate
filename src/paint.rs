//! Tiny antialiased painter for opaque windows (settings UI): rounded rects,
//! strokes, circles, lines and soft shadows on 0x00RRGGBB pixels.
//! Coordinates are logical (96-DPI) pixels; `s` is the DPI scale.

pub struct Canvas<'a> {
    pub px: &'a mut [u32],
    pub w: i32,
    pub h: i32,
    pub s: f32,
}

/// 0xRRGGBB colors.
pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let ch = |sh: u32| {
        let x = ((a >> sh) & 0xFF) as f32;
        let y = ((b >> sh) & 0xFF) as f32;
        ((x + (y - x) * t).round() as u32) & 0xFF
    };
    (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

fn sd_round_rect(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let r = r.min(hw).min(hh);
    let qx = (px - cx).abs() - (hw - r);
    let qy = (py - cy).abs() - (hh - r);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

fn sd_segment(px: f32, py: f32, (ax, ay): (f32, f32), (bx, by): (f32, f32)) -> f32 {
    let (pax, pay, bax, bay) = (px - ax, py - ay, bx - ax, by - ay);
    let h = ((pax * bax + pay * bay) / (bax * bax + bay * bay)).clamp(0.0, 1.0);
    (pax - bax * h).hypot(pay - bay * h)
}

// Drawing calls take geometry as plain numbers on purpose: it reads like a canvas API.
#[allow(clippy::too_many_arguments)]
impl Canvas<'_> {
    pub fn clear(&mut self, c: u32) {
        self.px.fill(c & 0xFF_FFFF);
    }

    /// Runs `cover(lx, ly) -> 0..1` over the physical pixels of a logical bbox.
    fn shape(&mut self, (x0, y0, x1, y1): (f32, f32, f32, f32), c: u32, a: f32, cover: impl Fn(f32, f32) -> f32) {
        if a <= 0.0 {
            return;
        }
        let px0 = ((x0 * self.s).floor() as i32).max(0);
        let py0 = ((y0 * self.s).floor() as i32).max(0);
        let px1 = ((x1 * self.s).ceil() as i32).min(self.w);
        let py1 = ((y1 * self.s).ceil() as i32).min(self.h);
        for y in py0..py1 {
            for x in px0..px1 {
                let k = cover((x as f32 + 0.5) / self.s, (y as f32 + 0.5) / self.s) * a;
                if k > 0.0 {
                    let i = (y * self.w + x) as usize;
                    self.px[i] = mix(self.px[i], c, k);
                }
            }
        }
    }

    pub fn round_rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, c: u32, a: f32) {
        let s = self.s;
        let (cx, cy, hw, hh) = (x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0);
        self.shape((x - 1.0, y - 1.0, x + w + 1.0, y + h + 1.0), c, a, |lx, ly| {
            (0.5 - sd_round_rect(lx, ly, cx, cy, hw, hh, r) * s).clamp(0.0, 1.0)
        });
    }

    /// Stroke centered `width` inside the edge of the rounded rect.
    pub fn round_rect_stroke(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, width: f32, c: u32, a: f32) {
        let s = self.s;
        let (cx, cy, hw, hh) = (x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0);
        self.shape((x - 1.0, y - 1.0, x + w + 1.0, y + h + 1.0), c, a, |lx, ly| {
            let d = sd_round_rect(lx, ly, cx, cy, hw, hh, r) + width / 2.0;
            (0.5 - (d.abs() - width / 2.0) * s).clamp(0.0, 1.0)
        });
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, c: u32, a: f32) {
        let s = self.s;
        self.shape((cx - r - 1.0, cy - r - 1.0, cx + r + 1.0, cy + r + 1.0), c, a, |lx, ly| {
            (0.5 - ((lx - cx).hypot(ly - cy) - r) * s).clamp(0.0, 1.0)
        });
    }

    pub fn line(&mut self, a: (f32, f32), b: (f32, f32), width: f32, c: u32, alpha: f32) {
        let s = self.s;
        let pad = width + 1.0;
        let bbox = (a.0.min(b.0) - pad, a.1.min(b.1) - pad, a.0.max(b.0) + pad, a.1.max(b.1) + pad);
        self.shape(bbox, c, alpha, |lx, ly| (0.5 - (sd_segment(lx, ly, a, b) - width / 2.0) * s).clamp(0.0, 1.0));
    }

    /// Soft drop shadow of a rounded rect (drawn before the shape itself).
    pub fn shadow(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, blur: f32, dy: f32, c: u32, a: f32) {
        let (cx, cy, hw, hh) = (x + w / 2.0, y + h / 2.0 + dy, w / 2.0, h / 2.0);
        self.shape((x - blur, y - blur + dy, x + w + blur, y + h + blur + dy), c, a, |lx, ly| {
            let d = sd_round_rect(lx, ly, cx, cy, hw, hh, r);
            let t = ((d + blur * 0.3) / (blur * 1.3)).clamp(0.0, 1.0);
            1.0 - t * t * (3.0 - 2.0 * t)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_cover_inside_not_outside() {
        let mut px = vec![0xFFFFFFu32; 100 * 60];
        let mut c = Canvas { px: &mut px, w: 100, h: 60, s: 2.0 };
        c.round_rect(5.0, 5.0, 30.0, 20.0, 6.0, 0x000000, 1.0);
        c.circle(42.0, 15.0, 4.0, 0xFF0000, 1.0);
        let at = |x: f32, y: f32| px[((y * 2.0) as i32 * 100 + (x * 2.0) as i32) as usize];
        assert_eq!(at(20.0, 15.0), 0x000000);
        assert_eq!(at(1.0, 1.0), 0xFFFFFF);
        assert_eq!(at(5.4, 5.4), 0xFFFFFF, "rounded corner stays clear");
        assert_eq!(at(42.0, 15.0), 0xFF0000);
    }

    #[test]
    fn mix_endpoints() {
        assert_eq!(mix(0x000000, 0xFFFFFF, 0.0), 0x000000);
        assert_eq!(mix(0x000000, 0xFFFFFF, 1.0), 0xFFFFFF);
        assert_eq!(mix(0x102030, 0x102030, 0.5), 0x102030);
    }
}
