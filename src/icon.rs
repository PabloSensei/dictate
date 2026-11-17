//! The tray icon: a microphone glyph in a colored disc, drawn at any size.

/// Disc colors for each tray state, in `app::Look` order.
pub const LOOK_COLORS: [(u8, u8, u8); 5] = [
    (0xF5, 0xA6, 0x23), // loading: amber
    (0x6E, 0x76, 0x80), // idle: slate
    (0xE5, 0x39, 0x35), // recording: red
    (0x1E, 0x88, 0xE5), // busy: blue
    (0x8E, 0x24, 0xAA), // error: purple
];

/// `s`×`s` pixels, rows top to bottom, 0xAARRGGBB with straight (not premultiplied) alpha.
pub fn tray_pixels(s: i32, (r, g, b): (u8, u8, u8)) -> Vec<u32> {
    let n = s.max(1) as usize;
    let f = n as f32;
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
    px
}

#[cfg(test)]
mod tests {
    #[test]
    fn icon_has_a_transparent_corner_and_a_white_mic() {
        let px = super::tray_pixels(32, super::LOOK_COLORS[2]);
        assert_eq!(px[0] >> 24, 0);
        let center = px[16 * 32 + 16 - 32 * 4]; // on the capsule
        assert_eq!(center, 0xFFFF_FFFF);
    }
}
