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

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// A PNG file (RGBA, uncompressed deflate) of 0xAARRGGBB straight-alpha pixels:
/// tray hosts that only take icon files by name get the same picture.
pub fn png(px: &[u32], w: u32, h: u32) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let crc = crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }
    // raw scanlines: filter byte 0, then R G B A
    let mut raw = Vec::with_capacity((w as usize * 4 + 1) * h as usize);
    for y in 0..h as usize {
        raw.push(0);
        for &p in &px[y * w as usize..(y + 1) * w as usize] {
            raw.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8, (p >> 24) as u8]);
        }
    }
    // zlib stream with stored (uncompressed) deflate blocks
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, b) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        let n = b.len() as u16;
        z.extend_from_slice(&n.to_le_bytes());
        z.extend_from_slice(&(!n).to_le_bytes());
        z.extend_from_slice(b);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut out = PNG_SIGNATURE.to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
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

    #[test]
    fn png_has_the_right_shape() {
        let px = super::tray_pixels(64, super::LOOK_COLORS[1]);
        let png = super::png(&px, 64, 64);
        assert_eq!(&png[..8], &super::PNG_SIGNATURE);
        assert_eq!(&png[12..16], b"IHDR");
        assert!(png.ends_with(&[0xAE, 0x42, 0x60, 0x82])); // CRC of an empty IEND
        // header + IHDR + IDAT (zlib + stored blocks + raw rows) + IEND
        let raw = (64 * 4 + 1) * 64;
        assert_eq!(png.len(), 8 + 25 + 12 + 2 + 5 + raw + 4 + 12);
    }
}
