//! dictate-bench: record your own test phrases and compare models on them.
//!
//!   dictate-bench record <dir> <phrases.txt>     record each phrase (Enter = start/stop)
//!   dictate-bench run <dir> <model.bin>...        WER/CER/latency per model
//!   dictate-bench file <audio.wav> <model.bin>    transcribe one file
//!   dictate-bench bubble <out.bmp>                render bubble frames (light + dark)
//!   dictate-bench demo <dir> [0xRRGGBB]           frames of the README animation
//! Options: --prompt "<terms>"  --beam N  --lang en  --vad <silero.bin>  --cpu
//! Env: DICTATE_NO_VAD=1, DICTATE_PREVIEW=1 (simulate live preview), DICTATE_DEBUG=1

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use dictate::audio::Recorder;
use dictate::config::Config;
use dictate::engine::Engine;
use dictate::{metrics, wav};

fn main() {
    if let Err(e) = real_main() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

struct Opts {
    pos: Vec<String>,
    cfg: Config,
}

fn parse_args() -> Result<Opts, String> {
    let mut defaults = Config::default();
    // Relative model paths in the template are relative to the exe, as for the app.
    if let Some(dir) = dictate::config::path_next_to_exe().parent() {
        defaults.resolve_paths(dir);
    }
    let vad_model = if std::env::var_os("DICTATE_NO_VAD").is_some() { None } else { defaults.vad_model.clone() };
    // No prompt unless --prompt is given, so runs are comparable.
    let mut cfg = Config { model_sha256: None, vad_model, prompt: String::new(), ..defaults };
    let mut pos = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut val = |name: &str| it.next().ok_or(format!("{name}: value expected"));
        match a.as_str() {
            "--prompt" => cfg.prompt = val("--prompt")?,
            "--beam" => cfg.beam_size = val("--beam")?.parse().map_err(|_| "--beam: a number expected")?,
            "--lang" => cfg.language = val("--lang")?,
            "--vad" => cfg.vad_model = Some(PathBuf::from(val("--vad")?)),
            "--cpu" => cfg.use_gpu = false,
            _ => pos.push(a),
        }
    }
    Ok(Opts { pos, cfg })
}

fn real_main() -> Result<(), String> {
    let o = parse_args()?;
    match o.pos.first().map(String::as_str) {
        Some("record") if o.pos.len() == 3 => record(Path::new(&o.pos[1]), Path::new(&o.pos[2]), &o.cfg),
        Some("run") if o.pos.len() >= 3 => run(Path::new(&o.pos[1]), &o.pos[2..], o.cfg),
        Some("file") if o.pos.len() == 3 => file(Path::new(&o.pos[1]), &o.pos[2], o.cfg),
        // self-test of text injection: types the given text into the focused window
        Some("type") if o.pos.len() == 2 => dictate::inject::type_text(&o.pos[1]),
        // renders bubble animation frames over light and dark backgrounds to a BMP
        Some("bubble") if o.pos.len() == 2 => bubble_sheet(Path::new(&o.pos[1])),
        // frames of the README demo animation: demo <dir> [background 0xRRGGBB]
        Some("demo") if (2..=3).contains(&o.pos.len()) => {
            let bg = o.pos.get(2).map(|v| u32::from_str_radix(v.trim_start_matches("0x"), 16)).transpose().map_err(|e| e.to_string())?;
            demo_frames(Path::new(&o.pos[1]), bg.unwrap_or(0x1E1E20))
        }
        _ => Err("usage:\n  dictate-bench record <dir> <phrases.txt>\n  dictate-bench run <dir> <model.bin>...\n  dictate-bench file <audio.wav> <model.bin>\n  dictate-bench bubble <out.bmp>\noptions: --prompt \"...\" --beam N --lang en --vad <silero.bin> --cpu".into()),
    }
}

fn bubble_sheet(out: &Path) -> Result<(), String> {
    use dictate::bubble::{self, CAP_TEXT_W, Caption, Frame, N_BARS, Phase};
    use dictate::surface;
    let s = 1.5;
    let (w, h) = bubble::size(s);
    let mut ts = surface::make_text_surface(s).ok_or("GDI text surface")?;
    let wave: Vec<f32> = (0..N_BARS).map(|i| 1.0 - (i as f32 - 5.0).abs() / 6.0).collect();
    let long = "Quick update on the release: the settings window is done, the bubble follows the mouse across monitors, and the live preview keeps up while you speak.";
    // (phase, t, appear, bars, caption, caption above)
    type Shot<'a> = (Phase, f32, f32, &'a [f32], &'a str, bool); // phase, t, appear, bars, caption, above
    let frames: Vec<Shot> = vec![
        (Phase::Recording, 0.4, 1.0, &wave, "", false),
        (Phase::Recording, 0.8, 1.0, &wave, "Hey, I'll be a bit late today", false),
        (Phase::Recording, 1.2, 1.0, &wave, long, false),
        (Phase::Recording, 0.8, 1.0, &wave, "Open a pull request on GitHub", true),
        (Phase::Busy, 1.0, 0.6, &[], "Restart the Docker container and check the logs.", false),
    ];
    let rows = frames.len() as i32;
    let (sw, sh) = (w * 2, h * rows);
    let mut sheet = vec![0u32; (sw * sh) as usize];
    let mut buf = vec![0u32; (w * h) as usize];
    for (ri, (phase, t, appear, bars, text, above)) in frames.iter().enumerate() {
        let (mask, mw, mh) = if text.is_empty() {
            (Vec::new(), 0, 0)
        } else {
            surface::draw_caption(&mut ts, text, (CAP_TEXT_W * s) as i32)
        };
        let caption = (mw > 0).then(|| Caption { mask: &mask, w: mw, h: mh, appear: 1.0 });
        let f = Frame { phase: *phase, t: *t, appear: *appear, bars, caption, caption_above: *above };
        bubble::render(&mut buf, w, h, s, &f);
        for (col, bg) in [(0, 0xF3F3F3u32), (1, 0x202124u32)].into_iter() {
            for y in 0..h {
                for x in 0..w {
                    let p = buf[(y * w + x) as usize];
                    let a = (p >> 24) as f32 / 255.0;
                    let ch = |sh: u32| {
                        let src = ((p >> sh) & 0xFF) as f32;
                        let dst = ((bg >> sh) & 0xFF) as f32;
                        (src + dst * (1.0 - a)).round().min(255.0) as u32
                    };
                    let px = (ch(16) << 16) | (ch(8) << 8) | ch(0);
                    sheet[((ri as i32 * h + y) * sw + col * w + x) as usize] = px;
                }
            }
        }
    }
    write_bmp(out, sw, sh, &sheet)
}

/// Writes the frames of a short demo animation (bubble listening with live text,
/// then recognizing, then hiding) over `bg` as numbered BMPs, for the README GIF.
fn demo_frames(dir: &Path, bg: u32) -> Result<(), String> {
    use dictate::bubble::{self, CAP_TEXT_W, Caption, Frame, N_BARS, Phase};
    use dictate::surface;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let s = 1.5; // crisp on HiDPI screens; the README shows it at 2/3 size
    let (w, h) = bubble::size(s);
    let mut ts = surface::make_text_surface(s).ok_or("GDI text surface")?;
    let words: Vec<&str> = "Hey team, quick update: the new release is ready, the installer works on a clean machine, and I'll send the notes after lunch."
        .split(' ')
        .collect();
    let fps = 25.0;
    let (speak_from, speak_to, busy_to, hide_to, end) = (0.5, 4.6, 5.5, 5.8, 6.6);
    // speech loudness: syllable-ish bumps with short pauses
    let level = |t: f32| {
        if !(0.3..speak_to).contains(&t) {
            return 0.04;
        }
        let syl = (t * 9.0).sin().abs() * (0.6 + 0.4 * (t * 2.3 + 1.0).sin());
        let pause = if (t * 1.3).sin() > 0.93 { 0.2 } else { 1.0 };
        (0.08 + 0.9 * syl.sqrt() * pause).min(1.0)
    };
    let ease = |x: f32| 1.0 - (1.0 - x.clamp(0.0, 1.0)).powi(3);
    let mut buf = vec![0u32; (w * h) as usize];
    let mut cap_text = String::new();
    let mut mask: (Vec<u8>, i32, i32) = (Vec::new(), 0, 0);
    let n = (end * fps) as usize;
    for i in 0..n {
        let t = i as f32 / fps;
        // the live preview refreshes about every 0.4 s
        let shown = if t < speak_from {
            0
        } else {
            let k = ((t - speak_from) / 0.4).floor() * 0.4 / (speak_to - speak_from);
            ((k * words.len() as f32).ceil() as usize).min(words.len())
        };
        let text = words[..shown].join(" ");
        if text != cap_text {
            cap_text = text;
            mask = surface::draw_caption(&mut ts, &cap_text, (CAP_TEXT_W * s) as i32);
        }
        let phase = if t < speak_to { Phase::Recording } else { Phase::Busy };
        let appear = if t < busy_to { ease(t / 0.16) } else { 1.0 - ease((t - busy_to) / (hide_to - busy_to)) };
        let cap_appear = if t < busy_to { ease((t - speak_from) / 0.18) } else { appear };
        let bars: Vec<f32> = (0..N_BARS).map(|b| level(t - b.abs_diff(N_BARS / 2) as f32 * 0.05)).collect();
        let caption = (mask.1 > 0 && cap_appear > 0.0).then(|| Caption { mask: &mask.0, w: mask.1, h: mask.2, appear: cap_appear });
        let f = Frame { phase, t, appear: appear.max(0.0), bars: &bars, caption, caption_above: false };
        bubble::render(&mut buf, w, h, s, &f);
        let px: Vec<u32> = buf
            .iter()
            .map(|&p| {
                let a = (p >> 24) as f32 / 255.0;
                let ch = |sh: u32| {
                    let src = ((p >> sh) & 0xFF) as f32;
                    let dst = ((bg >> sh) & 0xFF) as f32;
                    (src + dst * (1.0 - a)).round().min(255.0) as u32
                };
                (ch(16) << 16) | (ch(8) << 8) | ch(0)
            })
            .collect();
        write_bmp(&dir.join(format!("{i:03}.bmp")), w, h, &px)?;
    }
    Ok(())
}

/// 32-bit top-down BMP from 0x00RRGGBB pixels.
fn write_bmp(out: &Path, w: i32, h: i32, px: &[u32]) -> Result<(), String> {
    let mut b = Vec::with_capacity(54 + px.len() * 4);
    let data = (w * h * 4) as u32;
    b.extend_from_slice(b"BM");
    b.extend_from_slice(&(54 + data).to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(&54u32.to_le_bytes());
    b.extend_from_slice(&40u32.to_le_bytes());
    b.extend_from_slice(&w.to_le_bytes());
    b.extend_from_slice(&(-h).to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&32u16.to_le_bytes());
    b.extend_from_slice(&[0u8; 24]);
    for p in px {
        b.extend_from_slice(&(p | 0xFF00_0000).to_le_bytes());
    }
    std::fs::write(out, b).map_err(|e| format!("{}: {e}", out.display()))
}

fn read_line() -> String {
    let mut s = String::new();
    let _ = std::io::stdin().lock().read_line(&mut s);
    s.trim().to_string()
}

fn record(dir: &Path, phrases: &Path, cfg: &Config) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let text = std::fs::read_to_string(phrases).map_err(|e| format!("{}: {e}", phrases.display()))?;
    let list: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
    println!("{} phrases. For each: Enter starts recording, Enter stops. s skips, q quits.\n", list.len());
    let mut i = 0;
    while i < list.len() {
        let stem = dir.join(format!("{:03}", i + 1));
        let exists = stem.with_extension("wav").exists();
        println!("[{}/{}] {}{}", i + 1, list.len(), list[i], if exists { "  (already recorded: Enter re-records, s keeps it)" } else { "" });
        print!("  Enter to start > ");
        let _ = std::io::stdout().flush();
        match read_line().as_str() {
            "q" => break,
            "s" => {
                i += 1;
                continue;
            }
            _ => {}
        }
        let rec = Recorder::start(cfg.max_record_s)?;
        print!("  ● recording... Enter to stop ");
        let _ = std::io::stdout().flush();
        read_line();
        let pcm = rec.stop()?;
        let secs = pcm.len() as f32 / 16000.0;
        if secs < 0.5 {
            println!("  too short ({secs:.1} s), again");
            continue;
        }
        wav::write_16k_mono(&stem.with_extension("wav"), &pcm)?;
        std::fs::write(stem.with_extension("txt"), list[i]).map_err(|e| e.to_string())?;
        println!("  saved, {secs:.1} s\n");
        i += 1;
    }
    Ok(())
}

fn load(model: &str, mut cfg: Config) -> Result<(Engine, f32), String> {
    cfg.model = PathBuf::from(model);
    let t = Instant::now();
    let e = Engine::load(&cfg)?;
    Ok((e, t.elapsed().as_secs_f32()))
}

fn file(path: &Path, model: &str, cfg: Config) -> Result<(), String> {
    let pcm = wav::read_16k_mono(path)?;
    let (mut eng, load_s) = load(model, cfg)?;
    println!("model loaded in {load_s:.1} s");
    if std::env::var_os("DICTATE_PREVIEW").is_some() {
        // Simulate the live preview: what the bubble would show every 0.5 s while speaking.
        let step = 8000;
        let mut end = step;
        while end <= pcm.len() {
            let start = end.saturating_sub(16000 * 20);
            let t = Instant::now();
            let text = eng.preview(&pcm[start..end])?;
            println!("  [{:>4.1} s] {:>4} ms  {}", end as f32 / 16000.0, t.elapsed().as_millis(), text.unwrap_or_default());
            end += step;
        }
    }
    if std::env::var_os("DICTATE_DEBUG").is_some() {
        let prepared = eng.prepare(&pcm)?;
        println!("  prepared: {:?} s", prepared.as_ref().map(|p| p.len() as f32 / 16000.0));
        for (label, a) in [("raw", Some(pcm.clone())), ("prepared", prepared)] {
            if let Some(a) = a {
                for (text, ns) in eng.run_debug(&a)? {
                    println!("  [{label}] no_speech={ns:.2} {text:?}");
                }
            }
        }
    }
    match eng.transcribe(&pcm)? {
        Some(t) => println!("[{:.1} s of audio, {} ms]\n{}", t.audio_s, t.infer_ms, t.text),
        None => println!("(silence / nothing recognized)"),
    }
    Ok(())
}

fn run(dir: &Path, models: &[String], cfg: Config) -> Result<(), String> {
    let mut clips: Vec<(PathBuf, String)> = Vec::new();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "wav"))
        .collect();
    entries.sort();
    for w in entries {
        if let Ok(r) = std::fs::read_to_string(w.with_extension("txt")) {
            clips.push((w, r.trim().to_string()));
        }
    }
    if clips.is_empty() {
        return Err(format!("no NNN.wav + NNN.txt pairs in {}", dir.display()));
    }
    let audio: Vec<Vec<f32>> = clips.iter().map(|(w, _)| wav::read_16k_mono(w)).collect::<Result<_, _>>()?;

    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let report = dir.join(format!("results-{stamp}.tsv"));
    let mut tsv = String::from("model\tclip\tref\thyp\twer_err\tref_words\tms\n");
    let mut summary = Vec::new();

    for model in models {
        let name = Path::new(model).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        eprintln!("== {name}");
        let (mut eng, load_s) = load(model, cfg.clone())?;
        let (mut we, mut wn, mut ce, mut cn, mut ms, mut audio_s) = (0, 0, 0, 0, 0u128, 0f32);
        for ((w, reference), pcm) in clips.iter().zip(&audio) {
            let t = eng.transcribe(pcm)?;
            let (hyp, infer) = t.as_ref().map(|t| (t.text.clone(), t.infer_ms)).unwrap_or_default();
            let (a, b, c, d) = metrics::errors(reference, &hyp);
            we += a;
            wn += b;
            ce += c;
            cn += d;
            ms += infer;
            audio_s += pcm.len() as f32 / 16000.0;
            let clip = w.file_stem().unwrap_or_default().to_string_lossy();
            eprintln!("  {clip} {a}/{b}  {hyp}");
            tsv.push_str(&format!("{name}\t{clip}\t{reference}\t{hyp}\t{a}\t{b}\t{infer}\n"));
        }
        summary.push((name, 100.0 * we as f32 / wn.max(1) as f32, 100.0 * ce as f32 / cn.max(1) as f32, ms as f32 / clips.len() as f32, audio_s, load_s));
    }
    std::fs::write(&report, tsv).map_err(|e| e.to_string())?;

    println!("\n{} phrases, beam {}, prompt: {:?}\n", clips.len(), cfg.beam_size, cfg.prompt);
    println!("{:<28} {:>7} {:>7} {:>12} {:>9}", "model", "WER %", "CER %", "ms/phrase", "load");
    for (n, wer, cer, ms, _, load_s) in &summary {
        println!("{n:<28} {wer:>7.1} {cer:>7.1} {ms:>12.0} {load_s:>8.1}s");
    }
    println!("\ndetails: {}", report.display());
    Ok(())
}
