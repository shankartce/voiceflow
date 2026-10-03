//! `vt-bench record` (guided golden-set recorder) and `vt-bench mic-latency`.
//!
//! This is a developer tool. Unlike the app, it writes audio to disk, but only
//! clips the user explicitly keeps, and only under their own data directory.

use crate::golden::{prompts, Clip, ClipSet};
use crate::run::percentile;
use anyhow::{bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

/// Longest take we keep (the PRD's hold-to-talk cap). Bounds memory if Enter is never pressed.
const MAX_TAKE_SECS: usize = 120;

fn device_name(d: &cpal::Device) -> String {
    d.description().map(|desc| desc.name().to_string()).unwrap_or_else(|_| "unknown device".into())
}

pub fn list_devices() -> Result<()> {
    let host = cpal::default_host();
    let default = host.default_input_device().map(|d| device_name(&d));
    for d in host.input_devices().context("listing input devices")? {
        let name = device_name(&d);
        let mark = if Some(&name) == default.as_ref() { " (default)" } else { "" };
        println!("  {name}{mark}");
    }
    Ok(())
}

fn pick_device(name: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    match name {
        None => host.default_input_device().context("no default microphone found"),
        Some(want) => {
            let want = want.to_lowercase();
            host.input_devices()?
                .find(|d| device_name(d).to_lowercase().contains(&want))
                .with_context(|| format!("no input device matching `{want}` (see `vt-bench record --list-devices`)"))
        }
    }
}

/// An open capture stream appending interleaved f32 samples to `buf`.
struct Capture {
    _stream: cpal::Stream,
    buf: Arc<Mutex<Vec<f32>>>,
    channels: u16,
    rate: u32,
}

fn build_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    buf: Arc<Mutex<Vec<f32>>>,
    max_samples: usize,
    first: Option<mpsc::SyncSender<Instant>>,
) -> Result<cpal::Stream>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let fired = AtomicBool::new(false);
    let stream = device.build_input_stream::<T, _, _>(
        config,
        move |data: &[T], _| {
            if let Some(tx) = &first {
                if !fired.swap(true, Ordering::Relaxed) {
                    let _ = tx.try_send(Instant::now());
                }
            }
            if let Ok(mut b) = buf.lock() {
                let room = max_samples.saturating_sub(b.len());
                b.extend(data.iter().take(room).map(|&s| s.to_sample::<f32>()));
            }
        },
        |e| eprintln!("audio stream error: {e}"),
        None,
    )?;
    Ok(stream)
}

fn open(device: &cpal::Device, first: Option<mpsc::SyncSender<Instant>>) -> Result<Capture> {
    let supported = device.default_input_config().context("reading microphone format")?;
    let channels = supported.channels();
    let rate = supported.sample_rate();
    let config = supported.config();
    let max = rate as usize * usize::from(channels) * MAX_TAKE_SECS;
    let buf = Arc::new(Mutex::new(Vec::with_capacity(max / 4)));
    let b = buf.clone();
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build_stream::<f32>(device, config, b, max, first)?,
        SampleFormat::I16 => build_stream::<i16>(device, config, b, max, first)?,
        SampleFormat::U16 => build_stream::<u16>(device, config, b, max, first)?,
        SampleFormat::I32 => build_stream::<i32>(device, config, b, max, first)?,
        other => bail!("unsupported microphone sample format {other:?}"),
    };
    stream.play()?;
    Ok(Capture { _stream: stream, buf, channels, rate })
}

fn read_line(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut line = String::new();
    if io::stdin().lock().read_line(&mut line)? == 0 {
        bail!("stdin closed");
    }
    Ok(line.trim().to_lowercase())
}

enum Take {
    Keep(Vec<f32>),
    Redo,
    Skip,
    Quit,
}

fn record_one(device: &cpal::Device) -> Result<Take> {
    let cap = open(device, None)?;
    let _ = read_line("  ● Recording. Press Enter when you finish speaking… ")?;
    // Small tail so the last word isn't clipped by an early Enter.
    std::thread::sleep(Duration::from_millis(250));
    let interleaved = cap.buf.lock().map(|b| b.clone()).unwrap_or_default();
    let (channels, rate) = (cap.channels, cap.rate);
    drop(cap);

    let pcm = vt_audio::to_mono_16k(&interleaved, channels, rate)?;
    let secs = pcm.len() as f32 / vt_audio::TARGET_RATE as f32;
    let peak = pcm.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let mut note = String::new();
    if peak < 0.03 {
        note.push_str("  ⚠ very quiet. Is the right mic selected?");
    } else if peak > 0.99 {
        note.push_str("  ⚠ clipping. Move back from the mic a little.");
    }
    println!("  {secs:.1} s captured, peak level {:.0}%{note}", peak * 100.0);
    if secs < 0.5 {
        println!("  Too short; let's redo it.");
        return Ok(Take::Redo);
    }
    loop {
        match read_line("  [Enter] keep · [r] redo · [s] skip · [q] quit: ")?.as_str() {
            "" | "k" => return Ok(Take::Keep(pcm)),
            "r" => return Ok(Take::Redo),
            "s" => return Ok(Take::Skip),
            "q" => return Ok(Take::Quit),
            _ => {}
        }
    }
}

pub fn record(set_dir: &Path, device: Option<&str>, only: Option<&str>) -> Result<()> {
    let device = pick_device(device)?;
    let mic = device_name(&device);
    let mut set = ClipSet::load_or_new(set_dir, "personal")?;
    if set.note.is_empty() {
        set.note = "Personal recordings. Stay on this machine and are never committed.".into();
    }
    let all = prompts()?;
    let pending: Vec<_> = all
        .iter()
        .filter(|p| only.is_none_or(|o| o == p.id))
        .filter(|p| only.is_some() || !set.clips.iter().any(|c| c.id == p.id))
        .collect();

    println!("Microphone: {mic}");
    println!("Saving to:  {}", set_dir.display());
    println!(
        "{} of {} prompts recorded, {} to go.\n",
        all.len() - pending.len().min(all.len()),
        all.len(),
        pending.len()
    );
    println!("Read each sentence naturally, as if you were dictating it. Punctuation is not spoken.\n");

    for p in pending {
        let idx = all.iter().position(|q| q.id == p.id).unwrap_or(0) + 1;
        println!("[{idx}/{}] ({})\n  “{}”", all.len(), p.tags.join(", "), p.text);
        loop {
            match read_line("  Press Enter to start (s = skip, q = quit): ")?.as_str() {
                "q" => return finish(&set),
                "s" => break,
                _ => {}
            }
            match record_one(&device)? {
                Take::Redo => continue,
                Take::Skip => break,
                Take::Quit => return finish(&set),
                Take::Keep(pcm) => {
                    let file = format!("{}.wav", p.id);
                    std::fs::create_dir_all(set_dir)?;
                    vt_audio::write_wav_16k(&set_dir.join(&file), &pcm)?;
                    set.clips.retain(|c| c.id != p.id);
                    let mut tags = p.tags.clone();
                    tags.push(format!("mic:{mic}"));
                    set.clips.push(Clip {
                        id: p.id.clone(),
                        file,
                        reference: p.text.clone(),
                        tags,
                        duration_ms: (pcm.len() as u64 * 1000) / u64::from(vt_audio::TARGET_RATE),
                    });
                    set.clips.sort_by(|a, b| a.id.cmp(&b.id));
                    set.save(set_dir)?;
                    println!("  ✓ saved\n");
                    break;
                }
            }
        }
    }
    finish(&set)
}

fn finish(set: &ClipSet) -> Result<()> {
    println!("\n{} clips in the personal set. Next: `vt-bench run --set all`.", set.clips.len());
    Ok(())
}

/// Time from "open the mic" to the first audio callback: what a user waits for
/// before their first syllable is captured (docs/AUDIO_PIPELINE.md §3).
pub fn mic_latency(n: usize, device: Option<&str>) -> Result<()> {
    let device = pick_device(device)?;
    println!("Microphone: {}", device_name(&device));
    let mut samples = Vec::new();
    for i in 0..n.max(1) {
        let (tx, rx) = mpsc::sync_channel(1);
        let t0 = Instant::now();
        let cap = open(&device, Some(tx))?;
        let first = rx.recv_timeout(Duration::from_secs(3)).context("no audio arrived within 3 s")?;
        let ms = first.duration_since(t0).as_secs_f64() * 1000.0;
        drop(cap);
        samples.push(ms);
        eprint!("\r  {}/{n}: {ms:.0} ms   ", i + 1);
        std::thread::sleep(Duration::from_millis(300));
    }
    eprintln!();
    println!(
        "Mic open → first audio: p50 {:.0} ms · p95 {:.0} ms · max {:.0} ms",
        percentile(&samples, 50.0),
        percentile(&samples, 95.0),
        percentile(&samples, 100.0)
    );
    println!("(Decision rule: p50 ≤ 80 ms → open the mic on key press; otherwise default \"warm mic\" to 30 s.)");
    Ok(())
}
