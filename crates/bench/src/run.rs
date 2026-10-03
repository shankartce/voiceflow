//! `vt-bench run`: benchmark every engine on the golden sets, each in a fresh
//! process (so peak memory is per engine), then score and write a report.

use crate::golden::ClipSet;
use crate::machine::Machine;
use crate::wer::{self, Errors};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

/// ROADMAP P1 decision rule thresholds.
pub const LATENCY_BUDGET_MS: f64 = 1_000.0;
pub const RSS_BUDGET_BYTES: u64 = 1 << 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipRun {
    pub set: String,
    pub id: String,
    pub audio_ms: u32,
    /// Hypothesis from the first timed run.
    pub hypothesis: String,
    pub infer_ms: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineRun {
    pub engine: String,
    pub threads: usize,
    pub load_ms: u64,
    pub peak_rss_bytes: Option<u64>,
    pub clips: Vec<ClipRun>,
}

/// Child-process side: load one engine and run every clip `runs` times.
pub fn run_one(
    store: &vt_models::ModelStore,
    engine_id: &str,
    set_dirs: &[PathBuf],
    runs: usize,
    threads: usize,
) -> Result<EngineRun> {
    // Decode all audio before loading the model so decoding never counts.
    let mut audio = Vec::new();
    for dir in set_dirs {
        let set = ClipSet::load(dir)?;
        for clip in set.clips {
            let pcm = vt_audio::load_file(&dir.join(&clip.file)).with_context(|| clip.id.clone())?;
            audio.push((set.name.clone(), clip, pcm));
        }
    }
    if audio.is_empty() {
        bail!("no clips to run");
    }

    let entry = vt_models::manifest()?.get(engine_id)?;
    let paths = store.paths(entry)?;
    let t = Instant::now();
    let mut engine = vt_stt::load(&paths, &vt_stt::EngineConfig { threads })?;
    let load_ms = t.elapsed().as_millis() as u64;

    let opts = vt_stt::SttOptions::default();
    // Warm-up (first inference allocates and JITs kernels).
    engine.transcribe(&audio[0].2.samples, &opts)?;

    let mut clips = Vec::new();
    for (i, (set, clip, pcm)) in audio.iter().enumerate() {
        eprint!("\r  {engine_id}: clip {}/{}   ", i + 1, audio.len());
        let mut infer_ms = Vec::new();
        let mut hypothesis = None;
        for _ in 0..runs.max(1) {
            let tr = engine.transcribe(&pcm.samples, &opts)?;
            infer_ms.push(tr.infer_ms);
            hypothesis.get_or_insert(tr.text);
        }
        clips.push(ClipRun {
            set: set.clone(),
            id: clip.id.clone(),
            audio_ms: pcm.duration_ms() as u32,
            hypothesis: hypothesis.unwrap_or_default(),
            infer_ms,
        });
    }
    eprintln!();
    drop(engine);
    Ok(EngineRun {
        engine: engine_id.to_string(),
        threads,
        load_ms,
        peak_rss_bytes: vt_platform::process::peak_rss_bytes(),
        clips,
    })
}

/// Parent side: spawn `vt-bench run-one` for one engine and parse its JSON.
pub fn spawn_one(
    data_dir: &Path,
    engine_id: &str,
    set_dirs: &[PathBuf],
    runs: usize,
    threads: usize,
) -> Result<EngineRun> {
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.arg("--data-dir").arg(data_dir).args([
        "run-one",
        "--engine",
        engine_id,
        "--runs",
        &runs.to_string(),
        "--threads",
        &threads.to_string(),
    ]);
    for d in set_dirs {
        cmd.arg("--set-dir").arg(d);
    }
    let out = cmd.stdin(Stdio::null()).stderr(Stdio::inherit()).output().context("spawning run-one")?;
    if !out.status.success() {
        bail!("{engine_id}: benchmark process failed ({})", out.status);
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout.lines().rev().find(|l| l.starts_with('{')).context("no JSON from run-one")?;
    Ok(serde_json::from_str(line)?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetScore {
    pub set: String,
    pub clips: usize,
    pub audio_s: f64,
    pub wer: f64,
    pub pc_wer: f64,
    pub rtf_p50: f64,
    pub rtf_p95: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineSummary {
    pub engine: String,
    pub load_ms: u64,
    pub peak_rss_bytes: Option<u64>,
    /// p95 real-time factor × 10 s: estimated wait after a 10 s dictation.
    pub est_latency_10s_p95_ms: f64,
    pub sets: Vec<SetScore>,
}

impl EngineSummary {
    pub fn set(&self, name: &str) -> Option<&SetScore> {
        self.sets.iter().find(|s| s.set == name)
    }
}

pub fn percentile(values: &[f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let rank = ((p / 100.0) * v.len() as f64).ceil() as usize;
    v[rank.clamp(1, v.len()) - 1]
}

pub fn summarize(run: &EngineRun, references: &[(String, String, String)]) -> EngineSummary {
    let mut set_names: Vec<String> = run.clips.iter().map(|c| c.set.clone()).collect();
    set_names.dedup();
    let mut all_rtf = Vec::new();
    let mut sets = Vec::new();
    for name in set_names {
        let (mut e, mut pc) = (Errors::default(), Errors::default());
        let mut rtf = Vec::new();
        let mut audio_ms = 0u64;
        let mut n = 0;
        for c in run.clips.iter().filter(|c| c.set == name) {
            let reference = references
                .iter()
                .find(|(s, id, _)| *s == c.set && *id == c.id)
                .map(|(_, _, r)| r.as_str())
                .unwrap_or("");
            e.add(wer::wer(reference, &c.hypothesis));
            pc.add(wer::pc_wer(reference, &c.hypothesis));
            audio_ms += u64::from(c.audio_ms);
            n += 1;
            for &ms in &c.infer_ms {
                if c.audio_ms > 0 {
                    rtf.push(f64::from(ms) / f64::from(c.audio_ms));
                }
            }
        }
        all_rtf.extend(&rtf);
        sets.push(SetScore {
            set: name,
            clips: n,
            audio_s: audio_ms as f64 / 1000.0,
            wer: e.rate(),
            pc_wer: pc.rate(),
            rtf_p50: percentile(&rtf, 50.0),
            rtf_p95: percentile(&rtf, 95.0),
        });
    }
    EngineSummary {
        engine: run.engine.clone(),
        load_ms: run.load_ms,
        peak_rss_bytes: run.peak_rss_bytes,
        est_latency_10s_p95_ms: percentile(&all_rtf, 95.0) * 10_000.0,
        sets,
    }
}

/// ROADMAP P1 rule: lowest WER on `primary_set` among engines whose estimated
/// p95 latency for 10 s of speech is ≤ 1.0 s and whose peak RSS is ≤ 1 GiB.
pub fn recommend<'a>(engines: &'a [EngineSummary], primary_set: &str) -> Option<&'a EngineSummary> {
    engines
        .iter()
        .filter(|e| e.est_latency_10s_p95_ms <= LATENCY_BUDGET_MS)
        .filter(|e| e.peak_rss_bytes.is_none_or(|b| b <= RSS_BUDGET_BYTES))
        .filter_map(|e| e.set(primary_set).map(|s| (e, s.wer)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(e, _)| e)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Report {
    pub date: String,
    pub label: String,
    pub version: String,
    pub threads: usize,
    pub runs: usize,
    pub machine: Machine,
    pub summaries: Vec<EngineSummary>,
    /// Raw runs incl. hypotheses. Kept in the local JSON only, never in the markdown.
    pub raw: Vec<EngineRun>,
}

fn mb(bytes: Option<u64>) -> String {
    bytes.map_or_else(|| "n/a".into(), |b| format!("{:.0} MB", b as f64 / 1_048_576.0))
}

pub fn render_markdown(r: &Report) -> String {
    let m = &r.machine;
    let mut s = String::new();
    let _ = writeln!(s, "# vt-bench results: {} ({})\n", r.label, r.date);
    let _ = writeln!(s, "| Machine | |\n|---|---|");
    let _ = writeln!(s, "| CPU | {} |", m.cpu);
    let _ = writeln!(
        s,
        "| Cores | {} physical / {} logical |",
        m.physical_cores.map_or("?".into(), |c| c.to_string()),
        m.logical_cores
    );
    let _ = writeln!(s, "| RAM | {:.1} GB |", m.ram_gb);
    let _ = writeln!(s, "| OS | {} |", m.os);
    let _ = writeln!(s, "| Inference threads | {} |", r.threads);
    let _ = writeln!(s, "| Timed runs per clip | {} (after 1 warm-up) |", r.runs);
    let _ = writeln!(s, "| vt-bench | {} |\n", r.version);

    let mut set_names: Vec<&str> = r.summaries.iter().flat_map(|e| e.sets.iter().map(|s| s.set.as_str())).collect();
    set_names.sort_unstable();
    set_names.dedup();
    // Personal first: it is the deciding set when present.
    set_names.sort_by_key(|n| *n != "personal");

    for name in &set_names {
        let first = r.summaries.iter().find_map(|e| e.set(name));
        let (n, secs) = first.map_or((0, 0.0), |s| (s.clips, s.audio_s));
        let _ = writeln!(s, "## Set `{name}`: {n} clips, {:.1} min of audio\n", secs / 60.0);
        let _ = writeln!(
            s,
            "| Engine | WER | P&C WER | RTF p50 | RTF p95 | Est. wait after 10 s (p95) | Load | Peak RAM |"
        );
        let _ = writeln!(s, "|---|---:|---:|---:|---:|---:|---:|---:|");
        let mut rows: Vec<&EngineSummary> = r.summaries.iter().filter(|e| e.set(name).is_some()).collect();
        rows.sort_by(|a, b| {
            let wa = a.set(name).map_or(f64::MAX, |s| s.wer);
            let wb = b.set(name).map_or(f64::MAX, |s| s.wer);
            wa.total_cmp(&wb)
        });
        for e in rows {
            if let Some(sc) = e.set(name) {
                let pc = if *name == "libri" { "n/a¹".to_string() } else { format!("{:.1}%", sc.pc_wer) };
                let _ = writeln!(
                    s,
                    "| `{}` | {:.1}% | {} | {:.3} | {:.3} | {:.0} ms | {:.1} s | {} |",
                    e.engine,
                    sc.wer,
                    pc,
                    sc.rtf_p50,
                    sc.rtf_p95,
                    e.est_latency_10s_p95_ms,
                    e.load_ms as f64 / 1000.0,
                    mb(e.peak_rss_bytes)
                );
            }
        }
        s.push('\n');
    }

    let primary = if set_names.contains(&"personal") { "personal" } else { "libri" };
    let _ = writeln!(s, "## Recommendation (ROADMAP P1 rule)\n");
    match recommend(&r.summaries, primary) {
        Some(e) => {
            let _ = writeln!(
                s,
                "**Default engine: `{}`**. It has the lowest WER on `{primary}` among engines with an estimated p95 wait ≤ {:.1} s after 10 s of speech and peak RAM ≤ 1 GB.",
                e.engine,
                LATENCY_BUDGET_MS / 1000.0
            );
        }
        None => {
            let fastest =
                r.summaries.iter().min_by(|a, b| a.est_latency_10s_p95_ms.total_cmp(&b.est_latency_10s_p95_ms));
            let _ = writeln!(
                s,
                "**No engine meets both budgets on this machine.** Fastest: `{}` ({:.0} ms est. p95 wait). Consider fewer/more threads or a smaller model.",
                fastest.map_or("-", |e| e.engine.as_str()),
                fastest.map_or(0.0, |e| e.est_latency_10s_p95_ms)
            );
        }
    }
    if primary == "libri" {
        let _ = writeln!(
            s,
            "\n_Decided on public LibriSpeech audio only. Record the personal set (`vt-bench record`) for the real decision._"
        );
    }
    let _ = writeln!(
        s,
        "\n**Notes**\n\
         - WER ignores case, punctuation and number formatting. P&C WER counts them (what you'd have to fix by hand).\n\
         - ¹ LibriSpeech references have no punctuation or casing, so P&C WER is only meaningful on the personal set.\n\
         - RTF = inference time ÷ audio length (lower is faster). The estimated wait for 10 s is the p95 RTF × 10 s.\n\
         - Peak RAM is the whole benchmark process (model + audio + runtime), measured in a fresh process per engine."
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(engine: &str, wer: f64, lat: f64, rss_mb: u64) -> EngineSummary {
        EngineSummary {
            engine: engine.into(),
            load_ms: 1000,
            peak_rss_bytes: Some(rss_mb * 1_048_576),
            est_latency_10s_p95_ms: lat,
            sets: vec![SetScore {
                set: "libri".into(),
                clips: 20,
                audio_s: 180.0,
                wer,
                pc_wer: 0.0,
                rtf_p50: lat / 12_000.0,
                rtf_p95: lat / 10_000.0,
            }],
        }
    }

    #[test]
    fn percentile_nearest_rank() {
        let v = [5.0, 1.0, 3.0, 2.0, 4.0];
        assert_eq!(percentile(&v, 50.0), 3.0);
        assert_eq!(percentile(&v, 95.0), 5.0);
        assert_eq!(percentile(&v, 0.0), 1.0);
        assert_eq!(percentile(&[], 50.0), 0.0);
    }

    #[test]
    fn recommendation_follows_the_roadmap_rule() {
        let engines = vec![
            summary("accurate-but-slow", 2.0, 1500.0, 800),
            summary("accurate-but-fat", 2.5, 500.0, 1500),
            summary("good", 3.0, 600.0, 700),
            summary("fast-but-sloppy", 6.0, 150.0, 200),
        ];
        assert_eq!(recommend(&engines, "libri").map(|e| e.engine.as_str()), Some("good"));
        assert!(recommend(&engines[..2], "libri").is_none());
        assert!(recommend(&engines, "personal").is_none());
    }

    #[test]
    fn summarize_scores_and_rtf() {
        let run = EngineRun {
            engine: "e".into(),
            threads: 4,
            load_ms: 10,
            peak_rss_bytes: Some(1),
            clips: vec![
                ClipRun {
                    set: "libri".into(),
                    id: "a".into(),
                    audio_ms: 10_000,
                    hypothesis: "hello world".into(),
                    infer_ms: vec![500, 600],
                },
                ClipRun {
                    set: "libri".into(),
                    id: "b".into(),
                    audio_ms: 5_000,
                    hypothesis: "good night".into(),
                    infer_ms: vec![250, 250],
                },
            ],
        };
        let refs = vec![
            ("libri".into(), "a".into(), "HELLO WORLD".into()),
            ("libri".into(), "b".into(), "GOOD MORNING".into()),
        ];
        let s = summarize(&run, &refs);
        let l = s.set("libri").unwrap();
        assert!((l.wer - 25.0).abs() < 1e-9);
        assert!((l.rtf_p95 - 0.06).abs() < 1e-9);
        assert!((s.est_latency_10s_p95_ms - 600.0).abs() < 1e-6);
    }

    #[test]
    fn markdown_never_contains_hypotheses() {
        let r = Report {
            date: "2026-10-03".into(),
            label: "test".into(),
            version: "dev".into(),
            threads: 4,
            runs: 3,
            machine: crate::machine::describe(),
            summaries: vec![summary("good", 3.0, 600.0, 700)],
            raw: vec![EngineRun {
                engine: "good".into(),
                threads: 4,
                load_ms: 1,
                peak_rss_bytes: None,
                clips: vec![ClipRun {
                    set: "personal".into(),
                    id: "p01".into(),
                    audio_ms: 1000,
                    hypothesis: "SECRET PERSONAL TEXT".into(),
                    infer_ms: vec![1],
                }],
            }],
        };
        let md = render_markdown(&r);
        assert!(!md.contains("SECRET PERSONAL TEXT"));
        assert!(md.contains("Default engine: `good`"));
    }
}
