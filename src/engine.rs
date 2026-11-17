//! whisper.cpp wrapper: model loading with integrity check, VAD, transcription.

use std::io::Read;
use std::path::Path;
use std::time::Instant;

use sha2::{Digest, Sha256};
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState, WhisperVadContext,
    WhisperVadContextParams, WhisperVadParams,
};

use crate::audio::SAMPLE_RATE;
use crate::config::Config;
use crate::replace::{self, Rule};

const SR: usize = SAMPLE_RATE as usize;
/// Whisper sees audio in 30 s windows; staying inside one avoids a junk tail window.
const WINDOW: usize = 30 * SR;

pub struct Engine {
    _ctx: WhisperContext,
    state: WhisperState,
    vad: Option<WhisperVadContext>,
    language: String,
    prompt: String,
    replace: Vec<Rule>,
    beam_size: i32,
    threads: i32,
}

pub struct Transcript {
    pub text: String,
    pub audio_s: f32,
    pub infer_ms: u128,
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn check_file(path: &Path, sha: Option<&String>, what: &str) -> Result<String, String> {
    if !path.is_file() {
        return Err(format!("{what} not found: {}\nRun download-models.cmd or fix the path in dictate.ini.", path.display()));
    }
    if let Some(expected) = sha {
        let got = sha256_file(path)?;
        if &got != expected {
            return Err(format!("{what} checksum mismatch: {}\nexpected {expected}\ngot      {got}", path.display()));
        }
    }
    path.to_str().map(str::to_owned).ok_or_else(|| format!("path is not valid UTF-8: {}", path.display()))
}

impl Engine {
    pub fn load(cfg: &Config) -> Result<Self, String> {
        let model = check_file(&cfg.model, cfg.model_sha256.as_ref(), "model")?;
        // Route whisper.cpp / ggml logging into the (disabled) Rust logger: no console spam.
        whisper_rs::install_logging_hooks();

        let vad = match &cfg.vad_model {
            Some(p) => {
                let path = check_file(p, cfg.vad_model_sha256.as_ref(), "VAD model")?;
                let mut vp = WhisperVadContextParams::new();
                vp.set_use_gpu(false); // tiny model; keep the GPU for whisper
                vp.set_n_threads(usable_threads(cfg.threads));
                Some(WhisperVadContext::new(&path, vp).map_err(|e| format!("VAD: {e}"))?)
            }
            None => None,
        };

        let p = WhisperContextParameters { use_gpu: cfg.use_gpu, flash_attn: cfg.use_gpu, ..Default::default() };
        let ctx = WhisperContext::new_with_params(&model, p).map_err(|e| format!("loading the model: {e}"))?;
        let state = ctx.create_state().map_err(|e| format!("whisper state: {e}"))?;
        let mut engine = Self {
            _ctx: ctx,
            state,
            vad,
            language: cfg.language.clone(),
            prompt: cfg.prompt.clone(),
            replace: cfg.replace.clone(),
            beam_size: cfg.beam_size,
            threads: usable_threads(cfg.threads),
        };
        // Warm-up so the first real phrase doesn't pay for CUDA kernel/graph setup.
        let _ = engine.run(&vec![0.0f32; SR]);
        Ok(engine)
    }

    /// Returns None when there is no speech (silence, noise, a too-short tap).
    pub fn transcribe(&mut self, pcm: &[f32]) -> Result<Option<Transcript>, String> {
        let audio_s = pcm.len() as f32 / SAMPLE_RATE as f32;
        let t0 = Instant::now();
        let Some(prepared) = self.prepare(pcm)? else {
            return Ok(None);
        };
        let raw = self.run(&prepared)?;
        let infer_ms = t0.elapsed().as_millis();
        let text = clean_text(&raw);
        if text.is_empty() || is_hallucination(&text) {
            return Ok(None);
        }
        let text = replace::apply(&text, &self.replace);
        Ok(Some(Transcript { text, audio_s, infer_ms }))
    }

    /// Speech-only crop (VAD) + level normalization + short silent margins.
    pub fn prepare(&mut self, pcm: &[f32]) -> Result<Option<Vec<f32>>, String> {
        if pcm.len() < SR * 3 / 10 {
            return Ok(None);
        }
        let mut pcm = pcm;
        if let Some(vad) = self.vad.as_mut() {
            let mut vp = WhisperVadParams::new();
            vp.set_threshold(0.5);
            vp.set_min_speech_duration(150);
            vp.set_speech_pad(100);
            let segs = vad.segments_from_samples(vp, pcm).map_err(|e| format!("VAD: {e}"))?;
            let spans: Vec<(f32, f32)> = segs.map(|s| (s.start, s.end)).collect();
            let (Some(first), Some(last)) = (spans.first(), spans.last()) else {
                return Ok(None);
            };
            // centiseconds -> samples, with a little extra margin
            let a = ((first.0 / 100.0 - 0.15).max(0.0) * SAMPLE_RATE as f32) as usize;
            let b = (((last.1 / 100.0 + 0.25) * SAMPLE_RATE as f32) as usize).min(pcm.len());
            if b <= a {
                return Ok(None);
            }
            pcm = &pcm[a..b];
        }
        Ok(normalize_and_pad(pcm))
    }

    /// Raw whisper pass returning (segment text, no-speech probability), for the bench tool.
    pub fn run_debug(&mut self, pcm: &[f32]) -> Result<Vec<(String, f32)>, String> {
        self.run(pcm)?;
        Ok(self
            .state
            .as_iter()
            .map(|s| (s.to_str_lossy().map(|c| c.into_owned()).unwrap_or_default(), s.no_speech_probability()))
            .collect())
    }

    /// Quick draft of what has been said so far, for the live preview: greedy decoding,
    /// no temperature fallback. Roughly 100–200 ms on the GPU for a short phrase.
    pub fn preview(&mut self, pcm: &[f32]) -> Result<Option<String>, String> {
        let Some(prepared) = self.prepare(pcm)? else {
            return Ok(None);
        };
        let raw = self.run_with(&prepared, 1, false)?;
        let text = clean_text(&raw);
        if text.is_empty() || is_hallucination(&text) {
            return Ok(None);
        }
        Ok(Some(replace::apply(&text, &self.replace)))
    }

    fn run(&mut self, pcm: &[f32]) -> Result<String, String> {
        self.run_with(pcm, self.beam_size, true)
    }

    fn run_with(&mut self, pcm: &[f32], beam_size: i32, fallback: bool) -> Result<String, String> {
        let strategy = if beam_size > 1 {
            SamplingStrategy::BeamSearch { beam_size, patience: -1.0 }
        } else {
            SamplingStrategy::Greedy { best_of: 1 }
        };
        let language = self.language.clone();
        let prompt = self.prompt.clone();
        let mut fp = FullParams::new(strategy);
        fp.set_n_threads(self.threads);
        fp.set_language(Some(&language));
        fp.set_translate(false);
        fp.set_no_context(true);
        // Timestamps let whisper seek properly across 30 s windows; not needed below that.
        fp.set_no_timestamps(pcm.len() <= WINDOW);
        fp.set_print_special(false);
        fp.set_print_progress(false);
        fp.set_print_realtime(false);
        fp.set_print_timestamps(false);
        fp.set_suppress_blank(true);
        fp.set_suppress_nst(true);
        fp.set_temperature(0.0);
        fp.set_temperature_inc(if fallback { 0.2 } else { 0.0 });
        if !prompt.is_empty() {
            fp.set_initial_prompt(&prompt);
        }
        self.state.full(fp, pcm).map_err(|e| format!("recognition: {e}"))?;
        let mut out = String::new();
        for seg in self.state.as_iter() {
            if let Ok(s) = seg.to_str_lossy() {
                out.push_str(&s);
            }
        }
        Ok(out)
    }
}

/// Rejects near-silence, normalizes level, adds short silent margins
/// (but never pushes the clip over a 30 s window boundary).
pub fn normalize_and_pad(pcm: &[f32]) -> Option<Vec<f32>> {
    if pcm.len() < SR * 3 / 10 {
        return None;
    }
    let peak = pcm.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
    let rms = (pcm.iter().map(|&s| s * s).sum::<f32>() / pcm.len() as f32).sqrt();
    if peak < 0.02 || rms < 0.002 {
        return None;
    }
    let gain = (0.7 / peak).clamp(1.0, 8.0);
    let (head, tail) = (SR / 10, SR * 3 / 10);
    let room = WINDOW.saturating_sub(pcm.len() % WINDOW);
    let (head, tail) = if pcm.len() > WINDOW || head + tail > room { (0, 0) } else { (head, tail) };
    let mut v = Vec::with_capacity(pcm.len() + head + tail);
    v.extend(std::iter::repeat_n(0.0, head));
    v.extend(pcm.iter().map(|&s| (s * gain).clamp(-1.0, 1.0)));
    v.extend(std::iter::repeat_n(0.0, tail));
    Some(v)
}

pub fn clean_text(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whole-output matches against phrases Whisper invents on silence/noise
/// (learned from subtitle credits in its training data).
pub fn is_hallucination(text: &str) -> bool {
    let n: String = text
        .to_lowercase()
        .replace('ё', "е")
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect();
    let n = n.split_whitespace().collect::<Vec<_>>().join(" ");
    const BAD: &[&str] = &[
        "продолжение следует",
        "спасибо за просмотр",
        "спасибо за внимание",
        "подписывайтесь на канал",
        "thank you for watching",
        "thanks for watching",
        "please subscribe",
        "you",
    ];
    BAD.contains(&n.as_str()) || ["субтитры", "редактор субтитров", "subtitles by"].iter().any(|p| n.starts_with(p))
}

/// The configured thread count, but never more than the CPUs this process may use:
/// ggml's threads spin while they wait, so oversubscribing a small machine or VM makes
/// recognition many times slower instead of a little.
pub fn usable_threads(configured: i32) -> i32 {
    let cpus = std::thread::available_parallelism().map(|n| n.get() as i32).unwrap_or(4);
    configured.clamp(1, cpus.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hallucinations() {
        assert!(is_hallucination("Продолжение следует..."));
        assert!(is_hallucination(" Субтитры сделал DimaTorzok"));
        assert!(!is_hallucination("Продолжение следует завтра утром"));
    }

    #[test]
    fn silence_rejected() {
        assert!(normalize_and_pad(&vec![0.0; SR]).is_none());
        assert!(normalize_and_pad(&vec![0.5; 1000]).is_none());
        let tone: Vec<f32> = (0..SR).map(|i| (i as f32 * 0.05).sin() * 0.1).collect();
        assert!(normalize_and_pad(&tone).unwrap().len() > tone.len());
    }

    #[test]
    fn padding_stays_inside_window() {
        let near: Vec<f32> = (0..WINDOW - 100).map(|i| (i as f32 * 0.05).sin() * 0.1).collect();
        assert_eq!(normalize_and_pad(&near).unwrap().len(), near.len());
    }
}
