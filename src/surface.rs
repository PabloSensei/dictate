//! GDI surfaces shared by the bubble window and the bench tool: a 32-bit DIB
//! selected into a memory DC, and caption text rendered into a grayscale mask.

use std::ffi::c_void;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, CreateFontW,
    DEFAULT_CHARSET, DIB_RGB_COLORS, DT_CALCRECT, DT_CENTER, DT_NOPREFIX, DT_WORDBREAK, DeleteDC, DeleteObject,
    DrawTextW, FW_NORMAL, GdiFlush, HBITMAP, HDC, HFONT, HGDIOBJ, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};

use crate::bubble::{CAP_LINE_H, CAP_LINES, CAP_TEXT_W};
use crate::wide;

/// A 32-bit top-down DIB selected into its own memory DC.
pub struct Dib {
    pub dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    pub w: i32,
    pub h: i32,
}

impl Drop for Dib {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.bmp);
            DeleteDC(self.dc);
        }
    }
}

impl Dib {
    pub fn pixels(&mut self) -> &mut [u32] {
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.w * self.h) as usize) }
    }
}

/// GDI text surface for the caption, rebuilt when the DPI changes.
pub struct TextSurface {
    dib: Dib,
    font: HFONT,
    old_font: HGDIOBJ,
    line_px: i32,
}

impl Drop for TextSurface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dib.dc, self.old_font);
            DeleteObject(self.font);
        }
    }
}

pub fn make_dib(w: i32, h: i32) -> Option<Dib> {
    unsafe {
        let dc = CreateCompatibleDC(null_mut());
        if dc.is_null() {
            return None;
        }
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let mut bits: *mut c_void = null_mut();
        let bmp = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if bmp.is_null() || bits.is_null() {
            DeleteDC(dc);
            return None;
        }
        let old = SelectObject(dc, bmp);
        Some(Dib { dc, bmp, old, bits: bits as *mut u32, w, h })
    }
}

pub fn make_text_surface(scale: f32) -> Option<TextSurface> {
    let w = (CAP_TEXT_W * scale).ceil() as i32 + 4;
    let h = ((CAP_LINE_H * (CAP_LINES as f32 + 1.0)) * scale).ceil() as i32;
    let dib = make_dib(w, h)?;
    unsafe {
        let face = wide("Segoe UI");
        let font = CreateFontW(
            -((11.0 * 96.0 / 72.0) * scale).round() as i32,
            0,
            0,
            0,
            FW_NORMAL as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            0,
            0,
            ANTIALIASED_QUALITY as u32, // grayscale AA: the mask must not carry ClearType colors
            0,
            face.as_ptr(),
        );
        let old_font = SelectObject(dib.dc, font);
        SetTextColor(dib.dc, 0x00FF_FFFF);
        SetBkMode(dib.dc, TRANSPARENT as i32);
        let line_px = measure(dib.dc, "Ag", w).1.max(1);
        Some(TextSurface { dib, font, old_font, line_px })
    }
}

fn measure(dc: HDC, text: &str, width: i32) -> (i32, i32) {
    let w = wide(text);
    let mut r = RECT { left: 0, top: 0, right: width, bottom: 0 };
    unsafe { DrawTextW(dc, w.as_ptr(), -1, &mut r, DT_CALCRECT | DT_WORDBREAK | DT_CENTER | DT_NOPREFIX) };
    (r.right - r.left, r.bottom - r.top)
}

/// Lays the text out in at most CAP_LINES centered lines; when it doesn't fit,
/// keeps the end ("…" + latest words), since that is what is being said now.
pub fn draw_caption(ts: &mut TextSurface, text: &str, width: i32) -> (Vec<u8>, i32, i32) {
    let dc = ts.dib.dc;
    let max_h = ts.line_px * CAP_LINES as i32;
    let words: Vec<&str> = text.split(' ').collect();
    let fits = |s: usize| {
        let t = if s == 0 { text.to_string() } else { format!("…{}", words[s..].join(" ")) };
        let (_, h) = measure(dc, &t, width);
        (h <= max_h, t)
    };
    let shown = if fits(0).0 {
        text.to_string()
    } else {
        // smallest start index that fits
        let (mut lo, mut hi) = (1usize, words.len().saturating_sub(1).max(1));
        while lo < hi {
            let mid = (lo + hi) / 2;
            if fits(mid).0 { hi = mid } else { lo = mid + 1 }
        }
        fits(lo).1
    };
    let (mw, mh) = measure(dc, &shown, width);
    let (mw, mh) = (mw.clamp(1, ts.dib.w), mh.clamp(1, ts.dib.h));
    let sw = ts.dib.w;
    let px = ts.dib.pixels();
    px.fill(0);
    let wtext = wide(&shown);
    let mut r = RECT { left: 0, top: 0, right: mw, bottom: mh };
    unsafe {
        DrawTextW(dc, wtext.as_ptr(), -1, &mut r, DT_WORDBREAK | DT_CENTER | DT_NOPREFIX);
        GdiFlush();
    }
    let px = ts.dib.pixels();
    let mut mask = vec![0u8; (mw * mh) as usize];
    for y in 0..mh {
        for x in 0..mw {
            mask[(y * mw + x) as usize] = ((px[(y * sw + x) as usize] >> 8) & 0xFF) as u8;
        }
    }
    (mask, mw, mh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_caption_keeps_three_lines() {
        let mut ts = make_text_surface(1.0).expect("GDI surface");
        let width = CAP_TEXT_W as i32;
        let short = draw_caption(&mut ts, "Привет", width);
        assert!(short.1 > 0 && short.2 > 0 && short.2 <= ts.line_px + 2);
        let long = "слово ".repeat(400);
        let (mask, w, h) = draw_caption(&mut ts, long.trim(), width);
        assert!(h <= ts.line_px * CAP_LINES as i32, "height {h} > 3 lines of {}", ts.line_px);
        assert!(w <= width && mask.iter().any(|&m| m > 128));
    }
}
