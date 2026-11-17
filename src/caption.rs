//! Caption text for the bubble on macOS and Linux: a system font rasterized with
//! fontdue into a grayscale mask. Same API as the GDI version in `surface.rs`.

use std::sync::OnceLock;

use crate::bubble::CAP_LINES;

pub struct TextSurface {
    font: &'static fontdue::Font,
    px: f32,
    ascent: f32,
    pub line_px: i32,
}

#[cfg(target_os = "macos")]
const FONT_FILES: &[&str] = &[
    "/System/Library/Fonts/SFNS.ttf",
    "/System/Library/Fonts/Helvetica.ttc",
    "/System/Library/Fonts/Supplemental/Arial.ttf",
    "/Library/Fonts/Arial Unicode.ttf",
];
#[cfg(not(target_os = "macos"))]
const FONT_FILES: &[&str] = &[
    "/usr/share/fonts/truetype/ubuntu/Ubuntu[wdth,wght].ttf",
    "/usr/share/fonts/truetype/ubuntu/Ubuntu-R.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
];

/// fontconfig's pick for a sans-serif font that covers Cyrillic, if fc-match is there.
#[cfg(not(target_os = "macos"))]
fn fc_match() -> Option<String> {
    let out = std::process::Command::new("fc-match").args(["-f", "%{file}", "sans-serif:lang=ru"]).output().ok()?;
    let s = String::from_utf8(out.stdout).ok()?;
    (!s.is_empty()).then_some(s)
}
#[cfg(target_os = "macos")]
fn fc_match() -> Option<String> {
    None
}

fn load(path: &str) -> Option<fontdue::Font> {
    let bytes = std::fs::read(path).ok()?;
    let font = fontdue::Font::from_bytes(bytes, fontdue::FontSettings { scale: 40.0, ..Default::default() }).ok()?;
    // the bubble shows Russian, Polish, Ukrainian... a Latin-only font won't do
    ['ж', 'ą', 'ї', 'A'].iter().all(|&c| font.lookup_glyph_index(c) != 0).then_some(font)
}

fn font() -> Option<&'static fontdue::Font> {
    static FONT: OnceLock<Option<fontdue::Font>> = OnceLock::new();
    FONT.get_or_init(|| {
        let picked = fc_match();
        picked.iter().map(String::as_str).chain(FONT_FILES.iter().copied()).find_map(load)
    })
    .as_ref()
}

/// The caption font at this scale (11 pt at 96 DPI times `scale`, like on Windows).
pub fn make_text_surface(scale: f32) -> Option<TextSurface> {
    let font = font()?;
    let px = (11.0 * 96.0 / 72.0) * scale;
    let m = font.horizontal_line_metrics(px)?;
    Some(TextSurface { font, px, ascent: m.ascent, line_px: m.new_line_size.ceil().max(1.0) as i32 })
}

impl TextSurface {
    fn width(&self, s: &str) -> f32 {
        s.chars().map(|c| self.font.metrics(c, self.px).advance_width).sum()
    }

    /// Greedy word wrap; a word wider than the line is broken between characters.
    fn wrap(&self, text: &str, width: f32) -> Vec<String> {
        let mut lines = Vec::new();
        let mut line = String::new();
        for word in text.split(' ').filter(|w| !w.is_empty()) {
            let cand = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if self.width(&cand) <= width {
                line = cand;
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            for c in word.chars() {
                line.push(c);
                if self.width(&line) > width && line.chars().count() > 1 {
                    line.pop();
                    lines.push(std::mem::replace(&mut line, c.to_string()));
                }
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
        lines
    }
}

/// Lays the text out in at most CAP_LINES centered lines; when it doesn't fit,
/// keeps the end ("…" + latest words), since that is what is being said now.
pub fn draw_caption(ts: &mut TextSurface, text: &str, width: i32) -> (Vec<u8>, i32, i32) {
    let wf = width as f32;
    let words: Vec<&str> = text.split(' ').collect();
    let at = |s: usize| if s == 0 { text.to_string() } else { format!("…{}", words[s..].join(" ")) };
    let mut lines = ts.wrap(text, wf);
    if lines.len() > CAP_LINES {
        // smallest start index that fits
        let (mut lo, mut hi) = (1usize, words.len().saturating_sub(1).max(1));
        while lo < hi {
            let mid = (lo + hi) / 2;
            if ts.wrap(&at(mid), wf).len() <= CAP_LINES { hi = mid } else { lo = mid + 1 }
        }
        lines = ts.wrap(&at(lo), wf);
        lines.truncate(CAP_LINES);
    }
    if lines.is_empty() {
        return (vec![0], 1, 1);
    }
    let mw = lines.iter().map(|l| ts.width(l).ceil() as i32).max().unwrap_or(1).clamp(1, width.max(1));
    let mh = ts.line_px * lines.len() as i32;
    let mut mask = vec![0u8; (mw * mh) as usize];
    for (li, line) in lines.iter().enumerate() {
        let mut x = ((mw as f32 - ts.width(line)) / 2.0).max(0.0);
        let baseline = li as f32 * ts.line_px as f32 + ts.ascent;
        for c in line.chars() {
            let (m, bmp) = ts.font.rasterize(c, ts.px);
            let gx = (x + m.xmin as f32).round() as i32;
            let gy = (baseline - m.height as f32 - m.ymin as f32).round() as i32;
            for row in 0..m.height as i32 {
                for col in 0..m.width as i32 {
                    let (px, py) = (gx + col, gy + row);
                    if px < 0 || py < 0 || px >= mw || py >= mh {
                        continue;
                    }
                    let v = bmp[(row * m.width as i32 + col) as usize];
                    let d = &mut mask[(py * mw + px) as usize];
                    *d = (*d).max(v);
                }
            }
            x += m.advance_width;
        }
    }
    (mask, mw, mh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bubble::CAP_TEXT_W;

    #[test]
    fn long_caption_keeps_three_lines() {
        let Some(mut ts) = make_text_surface(1.0) else {
            eprintln!("no system font with Cyrillic here: skipped");
            return;
        };
        let width = CAP_TEXT_W as i32;
        let short = draw_caption(&mut ts, "Привет", width);
        assert!(short.1 > 0 && short.2 == ts.line_px);
        assert!(short.0.iter().any(|&m| m > 128));
        let long = "слово ".repeat(400);
        let (mask, w, h) = draw_caption(&mut ts, long.trim(), width);
        assert!(h <= ts.line_px * CAP_LINES as i32, "height {h} > 3 lines of {}", ts.line_px);
        assert!(w <= width && mask.iter().any(|&m| m > 128));
    }
}
