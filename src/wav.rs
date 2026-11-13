//! Minimal WAV I/O for the benchmark tool: PCM16 / float32, any channel count.
//! Reading resamples to 16 kHz mono (linear: fine for test clips).

use std::path::Path;

use crate::audio::SAMPLE_RATE;

pub fn write_16k_mono(path: &Path, pcm: &[f32]) -> Result<(), String> {
    let data_len = (pcm.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    b.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for &s in pcm {
        b.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, b).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn read_16k_mono(path: &Path) -> Result<Vec<f32>, String> {
    let d = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let err = |m: &str| format!("{}: {m}", path.display());
    if d.len() < 12 || &d[0..4] != b"RIFF" || &d[8..12] != b"WAVE" {
        return Err(err("not a WAV file"));
    }
    let u16_at = |i: usize| u16::from_le_bytes([d[i], d[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]);
    let (mut fmt, mut data) = (None, None);
    let mut i = 12;
    while i + 8 <= d.len() {
        let id = &d[i..i + 4];
        let len = u32_at(i + 4) as usize;
        let body = i + 8;
        let end = body.saturating_add(len).min(d.len());
        if id == b"fmt " && len >= 16 {
            fmt = Some((u16_at(body), u16_at(body + 2), u32_at(body + 4), u16_at(body + 14)));
        } else if id == b"data" {
            data = Some(&d[body..end]);
        }
        i = body.saturating_add(len + (len & 1));
    }
    let (tag, ch, rate, bits) = fmt.ok_or_else(|| err("no fmt chunk"))?;
    let data = data.ok_or_else(|| err("no data chunk"))?;
    let ch = ch.max(1) as usize;
    let tag = if tag == 0xFFFE { if bits == 32 { 3 } else { 1 } } else { tag };
    let frames: Vec<f32> = match (tag, bits) {
        (1, 16) => data
            .chunks_exact(2 * ch)
            .map(|f| f.as_chunks::<2>().0.iter().map(|s| i16::from_le_bytes(*s) as f32 / 32768.0).sum::<f32>() / ch as f32)
            .collect(),
        (3, 32) => data
            .chunks_exact(4 * ch)
            .map(|f| f.as_chunks::<4>().0.iter().map(|s| f32::from_le_bytes(*s)).sum::<f32>() / ch as f32)
            .collect(),
        _ => return Err(err(&format!("unsupported format (tag {tag}, {bits} bit)"))),
    };
    Ok(resample_linear(&frames, rate, SAMPLE_RATE))
}

fn resample_linear(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || x.is_empty() {
        return x.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let n = (x.len() as f64 / ratio) as usize;
    (0..n)
        .map(|i| {
            let p = i as f64 * ratio;
            let j = p as usize;
            let t = (p - j as f64) as f32;
            let a = x[j.min(x.len() - 1)];
            let b = x[(j + 1).min(x.len() - 1)];
            a + (b - a) * t
        })
        .collect()
}
