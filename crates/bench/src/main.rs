//! `vt-bench`: developer CLI for Phase 1 (docs/ROADMAP.md).
//!
//! Fetch models, record a personal golden set, transcribe files, and benchmark
//! every engine for accuracy (WER), speed (RTF) and memory.

mod golden;
mod machine;
#[cfg(feature = "record")]
mod record;
mod run;
mod wer;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;
use vt_models::download::{Downloader, Expected};
use vt_models::{ModelEntry, ModelKind, ModelStatus, ModelStore};

const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (", env!("VT_GIT_SHA"), ")");

#[derive(Parser)]
#[command(name = "vt-bench", version = VERSION, about = "Benchmark local speech-to-text engines (Murmur, Phase 1)")]
struct Cli {
    /// Data directory (models, golden sets, results). Default: %LOCALAPPDATA%\Murmur or ~/.local/share/Murmur.
    #[arg(long, global = true, env = "MURMUR_DATA_DIR")]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List models and whether they are downloaded.
    Models,
    /// Download and verify models (one-time; the only network access).
    Fetch {
        /// Model ids or aliases (parakeet, moonshine, whisper-base, whisper-small).
        ids: Vec<String>,
        /// Fetch every speech model.
        #[arg(long)]
        all: bool,
    },
    /// Install a model from a folder that already contains its files (offline).
    Import { id: String, folder: PathBuf },
    /// Re-hash downloaded models against the manifest.
    Verify { ids: Vec<String> },
    /// Manifest maintenance (dev/CI).
    Manifest {
        #[command(subcommand)]
        cmd: ManifestCmd,
    },
    /// Transcribe one audio file (.wav or .flac).
    Transcribe {
        #[arg(long, short)]
        engine: String,
        file: PathBuf,
        #[arg(long)]
        threads: Option<usize>,
    },
    /// Golden clip sets.
    Golden {
        #[command(subcommand)]
        cmd: GoldenCmd,
    },
    /// Record the personal golden set: read 40 prompts aloud (~15 minutes, resumable).
    #[cfg(feature = "record")]
    Record {
        /// Microphone name (substring). Default: the system default input.
        #[arg(long)]
        device: Option<String>,
        /// List microphones and exit.
        #[arg(long)]
        list_devices: bool,
        /// Re-record a single prompt id (e.g. p07).
        #[arg(long)]
        only: Option<String>,
    },
    /// Measure how long the microphone takes to deliver audio after opening.
    #[cfg(feature = "record")]
    MicLatency {
        #[arg(short, default_value_t = 20)]
        n: usize,
        #[arg(long)]
        device: Option<String>,
    },
    /// Benchmark engines on the golden sets and write a results report.
    Run {
        /// Engines to run (default: every downloaded speech model).
        #[arg(long, value_delimiter = ',')]
        engines: Vec<String>,
        #[arg(long, value_enum, default_value_t = SetChoice::All)]
        set: SetChoice,
        /// Timed runs per clip (after one warm-up).
        #[arg(long, default_value_t = 3)]
        runs: usize,
        /// Inference threads (default: physical cores).
        #[arg(long)]
        threads: Option<usize>,
        /// Output directory for the .md/.json report (default: <data dir>/results).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Label used in the report title and file name (default: host name).
        #[arg(long)]
        label: Option<String>,
    },
    /// (internal) Benchmark one engine and print JSON. Used by `run`.
    #[command(hide = true)]
    RunOne {
        #[arg(long)]
        engine: String,
        #[arg(long = "set-dir")]
        set_dirs: Vec<PathBuf>,
        #[arg(long)]
        runs: usize,
        #[arg(long)]
        threads: usize,
    },
}

#[derive(Subcommand)]
enum ManifestCmd {
    /// Resolve revisions and hash every model file + the LibriSpeech archive.
    /// Prints a regenerated models.toml and the libri.toml [dataset] block.
    Lock {
        /// Exit non-zero if the manifest differs from what was downloaded.
        #[arg(long)]
        check: bool,
        /// Re-resolve `main` even for models that already have a revision.
        #[arg(long)]
        relock: bool,
        /// Skip the LibriSpeech archive.
        #[arg(long)]
        skip_dataset: bool,
    },
    /// Print the manifest compiled into this binary.
    Show,
}

#[derive(Subcommand)]
enum GoldenCmd {
    /// Download LibriSpeech test-clean (~350 MB, once) and extract the 20 benchmark clips.
    FetchLibri,
    /// Show the golden sets on this machine.
    List,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum SetChoice {
    Libri,
    Personal,
    All,
}

fn main() {
    if let Err(e) = real_main() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let cli = Cli::parse();
    let data_dir = match cli.data_dir {
        Some(d) => d,
        None => vt_models::default_data_dir()?,
    };
    let store = ModelStore::new(data_dir.join("models"));
    let golden_dir = data_dir.join("golden");

    match cli.cmd {
        Cmd::Models => cmd_models(&store),
        Cmd::Fetch { ids, all } => cmd_fetch(&store, &ids, all),
        Cmd::Import { id, folder } => {
            let e = resolve(&id)?;
            store.import_from_dir(e, &folder)?;
            println!("✓ {} imported and verified", e.id);
            Ok(())
        }
        Cmd::Verify { ids } => cmd_verify(&store, &ids),
        Cmd::Manifest { cmd } => match cmd {
            ManifestCmd::Lock { check, relock, skip_dataset } => {
                cmd_lock(&store, &data_dir, check, relock, skip_dataset)
            }
            ManifestCmd::Show => {
                print!("{}", vt_models::manifest_source());
                Ok(())
            }
        },
        Cmd::Transcribe { engine, file, threads } => cmd_transcribe(&store, &engine, &file, threads),
        Cmd::Golden { cmd } => match cmd {
            GoldenCmd::FetchLibri => cmd_fetch_libri(&data_dir, &golden_dir),
            GoldenCmd::List => cmd_golden_list(&golden_dir),
        },
        #[cfg(feature = "record")]
        Cmd::Record { device, list_devices, only } => {
            if list_devices {
                record::list_devices()
            } else {
                record::record(&golden_dir.join("personal"), device.as_deref(), only.as_deref())
            }
        }
        #[cfg(feature = "record")]
        Cmd::MicLatency { n, device } => record::mic_latency(n, device.as_deref()),
        Cmd::Run { engines, set, runs, threads, out, label } => {
            cmd_run(&store, &data_dir, &golden_dir, &engines, set, runs, threads, out, label)
        }
        Cmd::RunOne { engine, set_dirs, runs, threads } => {
            let r = run::run_one(&store, &resolve(&engine)?.id, &set_dirs, runs, threads)?;
            println!("{}", serde_json::to_string(&r)?);
            Ok(())
        }
    }
}

/// Model id or a short alias.
fn resolve(name: &str) -> Result<&'static ModelEntry> {
    let id = match name {
        "parakeet" => "parakeet-tdt-0.6b-v2-int8",
        "moonshine" => "moonshine-base-en-int8",
        "whisper-base" => "whisper-base.en-q5_1",
        "whisper-small" => "whisper-small.en-q5_1",
        other => other,
    };
    Ok(vt_models::manifest()?.get(id)?)
}

fn stt_models() -> Result<Vec<&'static ModelEntry>> {
    Ok(vt_models::manifest()?.models.iter().filter(|m| m.kind == ModelKind::Stt).collect())
}

fn status_label(store: &ModelStore, e: &ModelEntry) -> &'static str {
    if !e.is_locked() {
        return "unpinned (run `manifest lock`)";
    }
    match store.status(e) {
        ModelStatus::Verified => "✓ ready",
        ModelStatus::Unverified => "downloaded, not verified",
        ModelStatus::Missing => "not downloaded",
    }
}

fn cmd_models(store: &ModelStore) -> Result<()> {
    println!("Models in {}\n", store.root().display());
    for e in &vt_models::manifest()?.models {
        println!(
            "  {:<28} {:<24} ~{:>4} MB  {:<10} {}",
            e.id,
            e.engine,
            e.approx_mb,
            e.license,
            status_label(store, e)
        );
    }
    Ok(())
}

fn progress_printer() -> impl FnMut(&str, u64, Option<u64>) {
    let mut last = Instant::now();
    move |name, done, total| {
        let finished = total.is_some_and(|t| done >= t);
        if last.elapsed().as_millis() < 200 && !finished {
            return;
        }
        last = Instant::now();
        let mb = done as f64 / 1_048_576.0;
        match total {
            Some(t) if t > 0 => eprint!(
                "\r  {name}: {mb:.0} / {:.0} MB ({:.0}%)   ",
                t as f64 / 1_048_576.0,
                100.0 * done as f64 / t as f64
            ),
            _ => eprint!("\r  {name}: {mb:.0} MB   "),
        }
        let _ = std::io::stderr().flush();
    }
}

fn cmd_fetch(store: &ModelStore, ids: &[String], all: bool) -> Result<()> {
    let entries: Vec<&ModelEntry> =
        if all || ids.is_empty() { stt_models()? } else { ids.iter().map(|i| resolve(i)).collect::<Result<_>>()? };
    let dl = Downloader::new();
    let mut failed = 0;
    for e in entries {
        if store.status(e) == ModelStatus::Verified {
            println!("✓ {} (already downloaded)", e.id);
            continue;
        }
        println!("↓ {} (~{} MB, {})", e.id, e.approx_mb, e.license);
        let mut p = progress_printer();
        match store.download(e, &dl, &mut p) {
            Ok(()) => println!("\n✓ {} verified", e.id),
            Err(err) => {
                println!("\n✗ {}: {err}", e.id);
                failed += 1;
            }
        }
    }
    if failed > 0 {
        bail!("{failed} model(s) failed");
    }
    Ok(())
}

fn cmd_verify(store: &ModelStore, ids: &[String]) -> Result<()> {
    let entries: Vec<&ModelEntry> =
        if ids.is_empty() { stt_models()? } else { ids.iter().map(|i| resolve(i)).collect::<Result<_>>()? };
    for e in entries {
        if store.status(e) == ModelStatus::Missing {
            println!("- {}: not downloaded", e.id);
            continue;
        }
        match store.verify(e) {
            Ok(()) => println!("✓ {}", e.id),
            Err(err) => println!("✗ {}: {err}", e.id),
        }
    }
    Ok(())
}

fn cmd_lock(store: &ModelStore, data_dir: &Path, check: bool, relock: bool, skip_dataset: bool) -> Result<()> {
    let dl = Downloader::new();
    let manifest = vt_models::manifest()?;
    let mut locked = Vec::new();
    let mut drift = Vec::new();
    for e in &manifest.models {
        eprintln!("locking {}", e.id);
        let mut p = progress_printer();
        // Keep going on failure so one CI run reports every problem.
        let l = match store.lock(e, &dl, relock, &mut p) {
            Ok(l) => l,
            Err(err) => {
                eprintln!("\n✗ {}: {err}", e.id);
                drift.push(format!("{} (failed: {err})", e.id));
                continue;
            }
        };
        eprintln!();
        let same = l.revision == e.source.revision
            && e.files
                .iter()
                .all(|f| l.files.iter().any(|(n, sha, size)| *n == f.name && *sha == f.sha256 && *size == f.size));
        if !same {
            drift.push(e.id.clone());
        }
        locked.push(l);
    }

    let mut dataset_block = None;
    if !skip_dataset {
        let cfg = golden::libri_config()?;
        let dest = data_dir.join("cache").join("test-clean.tar.gz");
        eprintln!("hashing {}", cfg.dataset.name);
        let mut p = progress_printer();
        match dl.download_file(&cfg.dataset.url, &dest, None, &mut |d, t| p("test-clean.tar.gz", d, t)) {
            Ok((sha, size)) => {
                eprintln!();
                if sha != cfg.dataset.sha256 || size != cfg.dataset.size {
                    drift.push("libri dataset".into());
                }
                dataset_block = Some(format!("sha256 = \"{sha}\"\nsize = {size}\n"));
            }
            Err(err) => {
                eprintln!("\n✗ libri dataset: {err}");
                drift.push(format!("libri dataset (failed: {err})"));
            }
        }
    }

    println!("===== models.toml (regenerated) =====");
    print!("{}", render_locked_manifest(manifest, &locked));
    if let Some(b) = dataset_block {
        println!("===== bench/golden/libri.toml [dataset] =====");
        print!("{b}");
    }
    println!("===== end =====");

    if drift.is_empty() {
        eprintln!("manifest is up to date");
        Ok(())
    } else if check {
        bail!("manifest is not locked/up to date for: {}. Paste the block above into the files.", drift.join(", "))
    } else {
        eprintln!("changed: {}", drift.join(", "));
        Ok(())
    }
}

/// Re-render models.toml with locked revisions/hashes, keeping the header comment.
fn render_locked_manifest(manifest: &vt_models::Manifest, locked: &[vt_models::download::LockedModel]) -> String {
    let src = vt_models::manifest_source();
    let header: String =
        src.lines().take_while(|l| l.starts_with('#') || l.trim().is_empty()).fold(String::new(), |mut acc, l| {
            acc.push_str(l);
            acc.push('\n');
            acc
        });
    let mut s = header;
    for e in &manifest.models {
        let l = locked.iter().find(|l| l.id == e.id);
        let _ = writeln!(s, "[[model]]");
        let _ = writeln!(s, "id = \"{}\"", e.id);
        let _ = writeln!(s, "kind = \"{}\"", format!("{:?}", e.kind).to_lowercase());
        let _ = writeln!(s, "engine = \"{}\"", e.engine);
        let _ = writeln!(s, "license = \"{}\"", e.license);
        let _ = writeln!(s, "attribution = \"{}\"", e.attribution);
        let _ = writeln!(s, "approx_mb = {}", e.approx_mb);
        let rev = l.map_or(e.source.revision.as_str(), |l| l.revision.as_str());
        match &e.source.hf_repo {
            Some(repo) => {
                let _ = writeln!(s, "source = {{ hf_repo = \"{repo}\", revision = \"{rev}\" }}");
            }
            None => {
                let _ = writeln!(s, "source = {{ revision = \"{rev}\" }}");
            }
        }
        for f in &e.files {
            let (sha, size) = l
                .and_then(|l| l.files.iter().find(|(n, _, _)| *n == f.name))
                .map_or((f.sha256.as_str(), f.size), |(_, sha, size)| (sha.as_str(), *size));
            let _ = writeln!(s, "\n  [[model.file]]");
            let _ = writeln!(s, "  name = \"{}\"", f.name);
            let _ = writeln!(s, "  role = \"{}\"", f.role);
            if let Some(u) = &f.url {
                let _ = writeln!(s, "  url = \"{u}\"");
            }
            let _ = writeln!(s, "  sha256 = \"{sha}\"");
            let _ = writeln!(s, "  size = {size}");
        }
        s.push('\n');
    }
    s
}

fn cmd_transcribe(store: &ModelStore, engine: &str, file: &Path, threads: Option<usize>) -> Result<()> {
    let e = resolve(engine)?;
    let pcm = vt_audio::load_file(file)?;
    let paths = store.paths(e)?;
    let threads = threads.unwrap_or_else(machine::default_threads);
    let t = Instant::now();
    let mut eng = vt_stt::load(&paths, &vt_stt::EngineConfig { threads })?;
    let load_ms = t.elapsed().as_millis();
    let opts = vt_stt::SttOptions::default();
    let cold = eng.transcribe(&pcm.samples, &opts)?;
    let warm = eng.transcribe(&pcm.samples, &opts)?;
    println!("{}", warm.text);
    eprintln!(
        "\n{}: audio {:.1} s · load {} ms · first run {} ms · warm run {} ms · RTF {:.3} · {} threads",
        e.id,
        pcm.duration_ms() as f64 / 1000.0,
        load_ms,
        cold.infer_ms,
        warm.infer_ms,
        f64::from(warm.infer_ms) / pcm.duration_ms().max(1) as f64,
        threads
    );
    Ok(())
}

fn cmd_fetch_libri(data_dir: &Path, golden_dir: &Path) -> Result<()> {
    let cfg = golden::libri_config()?;
    if cfg.dataset.sha256.is_empty() || cfg.dataset.size == 0 {
        bail!("bench/golden/libri.toml is not pinned yet (run `vt-bench manifest lock`)");
    }
    let tarball = data_dir.join("cache").join("test-clean.tar.gz");
    println!("↓ {} ({:.0} MB, {})", cfg.dataset.name, cfg.dataset.size as f64 / 1_048_576.0, cfg.dataset.license);
    let mut p = progress_printer();
    Downloader::new().download_file(
        &cfg.dataset.url,
        &tarball,
        Some(Expected { sha256: &cfg.dataset.sha256, size: cfg.dataset.size }),
        &mut |d, t| p("test-clean.tar.gz", d, t),
    )?;
    println!();
    let out = golden_dir.join("libri");
    let set = golden::build_libri_set(&tarball, &out, &cfg.selection, &mut |m| println!("  {m}"))?;
    let secs: u64 = set.clips.iter().map(|c| c.duration_ms).sum::<u64>() / 1000;
    println!("✓ {} clips ({} s of audio) in {}", set.clips.len(), secs, out.display());
    Ok(())
}

fn cmd_golden_list(golden_dir: &Path) -> Result<()> {
    for name in ["libri", "personal"] {
        let dir = golden_dir.join(name);
        match golden::ClipSet::load(&dir) {
            Ok(s) => {
                let secs: u64 = s.clips.iter().map(|c| c.duration_ms).sum::<u64>() / 1000;
                println!("  {name:<9} {:>3} clips, {:>4} s  ({})", s.clips.len(), secs, dir.display());
            }
            Err(_) => println!("  {name:<9} (none)"),
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn cmd_run(
    store: &ModelStore,
    data_dir: &Path,
    golden_dir: &Path,
    engines: &[String],
    set: SetChoice,
    runs: usize,
    threads: Option<usize>,
    out: Option<PathBuf>,
    label: Option<String>,
) -> Result<()> {
    let mut set_dirs = Vec::new();
    for (name, wanted) in [
        ("personal", matches!(set, SetChoice::Personal | SetChoice::All)),
        ("libri", matches!(set, SetChoice::Libri | SetChoice::All)),
    ] {
        let dir = golden_dir.join(name);
        if wanted && golden::ClipSet::manifest_path(&dir).is_file() {
            set_dirs.push(dir);
        } else if wanted && set != SetChoice::All {
            bail!("no `{name}` set found in {} (see `vt-bench golden list`)", golden_dir.display());
        }
    }
    if set_dirs.is_empty() {
        bail!("no golden sets found. Run `vt-bench golden fetch-libri` and/or `vt-bench record` first");
    }

    let entries: Vec<&ModelEntry> = if engines.is_empty() {
        stt_models()?.into_iter().filter(|e| store.status(e) == ModelStatus::Verified).collect()
    } else {
        engines.iter().map(|i| resolve(i)).collect::<Result<_>>()?
    };
    if entries.is_empty() {
        bail!("no downloaded models. Run `vt-bench fetch --all` first");
    }

    let mut references = Vec::new();
    for d in &set_dirs {
        let s = golden::ClipSet::load(d)?;
        for c in s.clips {
            references.push((s.name.clone(), c.id, c.reference));
        }
    }

    let threads = threads.unwrap_or_else(machine::default_threads);
    let mut raw = Vec::new();
    for e in &entries {
        eprintln!("▶ {} ({} threads, {} runs/clip)", e.id, threads, runs);
        match run::spawn_one(data_dir, &e.id, &set_dirs, runs, threads) {
            Ok(r) => raw.push(r),
            Err(err) => eprintln!("✗ {}: {err:#}", e.id),
        }
    }
    if raw.is_empty() {
        bail!("every engine failed");
    }
    let summaries = raw.iter().map(|r| run::summarize(r, &references)).collect();
    let m = machine::describe();
    let label = label.unwrap_or_else(|| m.host.clone());
    let report = run::Report {
        date: machine::today_utc(),
        label: label.clone(),
        version: VERSION.into(),
        threads,
        runs,
        machine: m,
        summaries,
        raw,
    };
    let md = run::render_markdown(&report);
    let out = out.unwrap_or_else(|| data_dir.join("results"));
    std::fs::create_dir_all(&out)?;
    let base = format!("{}-{}", report.date, machine::slug(&label));
    let md_path = out.join(format!("{base}.md"));
    let json_path = out.join(format!("{base}.json"));
    std::fs::write(&md_path, &md).with_context(|| md_path.display().to_string())?;
    std::fs::write(&json_path, serde_json::to_string_pretty(&report)?)?;
    println!("{md}");
    println!("Report: {}", md_path.display());
    println!("Details (includes transcripts, keep private): {}", json_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_resolve() {
        for a in ["parakeet", "moonshine", "whisper-base", "whisper-small"] {
            assert!(resolve(a).is_ok(), "{a}");
        }
        assert!(resolve("nope").is_err());
    }

    #[test]
    fn regenerated_manifest_round_trips() {
        let m = vt_models::manifest().unwrap();
        let locked: Vec<_> = m
            .models
            .iter()
            .map(|e| vt_models::download::LockedModel {
                id: e.id.clone(),
                revision: "deadbeef".into(),
                files: e.files.iter().map(|f| (f.name.clone(), "a".repeat(64), 42)).collect(),
            })
            .collect();
        let text = render_locked_manifest(m, &locked);
        let again = vt_models::Manifest::parse(&text).unwrap();
        assert_eq!(again.models.len(), m.models.len());
        assert!(again.models.iter().all(|e| e.is_locked() && e.source.revision == "deadbeef"));
        assert!(text.starts_with("# Model manifest"));
    }
}
