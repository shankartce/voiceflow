//! Golden clip sets: `<data>/golden/<set>/manifest.toml` + audio files.
//!
//! * `libri`: 20 LibriSpeech test-clean utterances, chosen deterministically.
//! * `personal`: the user's own recordings (`vt-bench record`). Never committed.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

#[cfg_attr(not(feature = "record"), allow(dead_code))]
pub const PROMPTS_TOML: &str = include_str!("../../../bench/golden/prompts.toml");
pub const LIBRI_TOML: &str = include_str!("../../../bench/golden/libri.toml");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Clip {
    pub id: String,
    /// File name relative to the set directory.
    pub file: String,
    pub reference: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClipSet {
    pub name: String,
    #[serde(default)]
    pub note: String,
    #[serde(default, rename = "clip")]
    pub clips: Vec<Clip>,
}

impl ClipSet {
    pub fn manifest_path(dir: &Path) -> PathBuf {
        dir.join("manifest.toml")
    }

    pub fn load(dir: &Path) -> Result<Self> {
        let path = Self::manifest_path(dir);
        let src = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&src).with_context(|| format!("parsing {}", path.display()))
    }

    #[cfg_attr(not(feature = "record"), allow(dead_code))]
    pub fn load_or_new(dir: &Path, name: &str) -> Result<Self> {
        if Self::manifest_path(dir).is_file() {
            Self::load(dir)
        } else {
            Ok(Self { name: name.into(), ..Self::default() })
        }
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        fs::create_dir_all(dir)?;
        let path = Self::manifest_path(dir);
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, toml::to_string_pretty(self)?)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }
}

#[cfg_attr(not(feature = "record"), allow(dead_code))]
#[derive(Debug, Clone, Deserialize)]
pub struct Prompt {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[cfg_attr(not(feature = "record"), allow(dead_code))]
#[derive(Debug, Deserialize)]
struct PromptFile {
    prompt: Vec<Prompt>,
}

#[cfg_attr(not(feature = "record"), allow(dead_code))]
pub fn prompts() -> Result<Vec<Prompt>> {
    let f: PromptFile = toml::from_str(PROMPTS_TOML).context("bench/golden/prompts.toml")?;
    Ok(f.prompt)
}

#[derive(Debug, Clone, Deserialize)]
pub struct LibriConfig {
    pub dataset: Dataset,
    pub selection: Selection,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Dataset {
    pub name: String,
    pub license: String,
    pub url: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Selection {
    pub count: usize,
    pub min_seconds: f64,
    pub max_seconds: f64,
}

pub fn libri_config() -> Result<LibriConfig> {
    toml::from_str(LIBRI_TOML).context("bench/golden/libri.toml")
}

/// LibriSpeech id `spk-chapter-utt` as a sortable numeric key.
type LibriKey = (u64, u64, u64);

fn libri_key(id: &str) -> Option<LibriKey> {
    let mut it = id.split('-').map(|p| p.parse::<u64>().ok());
    Some((it.next()??, it.next()??, it.next()??))
}

/// Deterministic selection: speakers in numeric order, and for each the first
/// utterance (numeric order) whose duration is within range; `count` speakers.
pub fn select_libri(durations: &[(String, f64)], sel: &Selection) -> Vec<String> {
    let mut by_speaker: BTreeMap<u64, Vec<(LibriKey, &str)>> = BTreeMap::new();
    for (id, secs) in durations {
        if *secs < sel.min_seconds || *secs > sel.max_seconds {
            continue;
        }
        if let Some(k) = libri_key(id) {
            by_speaker.entry(k.0).or_default().push((k, id.as_str()));
        }
    }
    by_speaker
        .into_values()
        .filter_map(|mut v| {
            v.sort();
            v.first().map(|(_, id)| id.to_string())
        })
        .take(sel.count)
        .collect()
}

fn flac_duration_secs(bytes: &[u8]) -> Option<f64> {
    let r = claxon::FlacReader::new(Cursor::new(bytes)).ok()?;
    let info = r.streaminfo();
    Some(info.samples? as f64 / f64::from(info.sample_rate))
}

fn open_tar(path: &Path) -> Result<tar::Archive<flate2::read::GzDecoder<fs::File>>> {
    let f = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    Ok(tar::Archive::new(flate2::read::GzDecoder::new(f)))
}

fn stem(path: &Path) -> Option<String> {
    path.file_name()?.to_str()?.split('.').next().map(String::from)
}

/// Build the `libri` clip set from a downloaded test-clean tarball.
/// Two passes over the archive: measure durations + read transcripts, then
/// extract only the selected files.
pub fn build_libri_set(tarball: &Path, out_dir: &Path, sel: &Selection, log: &mut dyn FnMut(&str)) -> Result<ClipSet> {
    let mut durations = Vec::new();
    let mut transcripts: HashMap<String, String> = HashMap::new();
    log("scanning archive (pass 1/2)…");
    for entry in open_tar(tarball)?.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let name = path.to_string_lossy().into_owned();
        if name.ends_with(".flac") {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            if let (Some(id), Some(secs)) = (stem(&path), flac_duration_secs(&bytes)) {
                durations.push((id, secs));
            }
        } else if name.ends_with(".trans.txt") {
            let mut text = String::new();
            entry.read_to_string(&mut text)?;
            for line in text.lines() {
                if let Some((id, t)) = line.split_once(' ') {
                    transcripts.insert(id.to_string(), t.trim().to_string());
                }
            }
        }
    }
    let chosen = select_libri(&durations, sel);
    if chosen.len() < sel.count {
        bail!("only {} utterances matched the selection rule", chosen.len());
    }
    let wanted: HashSet<&str> = chosen.iter().map(String::as_str).collect();
    let dur: HashMap<&str, f64> = durations.iter().map(|(i, d)| (i.as_str(), *d)).collect();

    log("extracting selected clips (pass 2/2)…");
    fs::create_dir_all(out_dir)?;
    for entry in open_tar(tarball)?.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        if let Some(id) = stem(&path).filter(|id| wanted.contains(id.as_str())) {
            if path.to_string_lossy().ends_with(".flac") {
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes)?;
                fs::write(out_dir.join(format!("{id}.flac")), bytes)?;
            }
        }
    }

    let mut set = ClipSet {
        name: "libri".into(),
        note: "LibriSpeech test-clean (CC-BY-4.0), selected by bench/golden/libri.toml".into(),
        clips: Vec::new(),
    };
    for id in &chosen {
        let reference = transcripts.get(id).with_context(|| format!("no transcript for {id}"))?.clone();
        set.clips.push(Clip {
            id: id.clone(),
            file: format!("{id}.flac"),
            reference,
            tags: vec!["libri".into()],
            duration_ms: (dur.get(id.as_str()).copied().unwrap_or(0.0) * 1000.0).round() as u64,
        });
    }
    set.save(out_dir)?;
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_configs_parse() {
        let p = prompts().unwrap();
        assert_eq!(p.len(), 40);
        let ids: HashSet<_> = p.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids.len(), p.len(), "prompt ids must be unique");
        assert!(p.iter().all(|p| !p.text.trim().is_empty() && !p.tags.is_empty()));
        let l = libri_config().unwrap();
        assert_eq!(l.selection.count, 20);
        assert!(l.dataset.url.starts_with("https://"));
    }

    #[test]
    fn selection_is_deterministic_and_one_per_speaker() {
        let sel = Selection { count: 2, min_seconds: 5.0, max_seconds: 15.0 };
        let d = vec![
            ("1089-134686-0002".to_string(), 7.0),
            ("1089-134686-0000".to_string(), 3.0), // too short
            ("1089-134686-0001".to_string(), 9.0),
            ("121-121726-0000".to_string(), 6.0),
            ("237-126133-0000".to_string(), 20.0), // too long
            ("237-126133-0001".to_string(), 8.0),
            ("61-70968-0000".to_string(), 10.0),
        ];
        // Numeric speaker order: 61, 121, 237, 1089.
        assert_eq!(select_libri(&d, &sel), vec!["61-70968-0000", "121-121726-0000"]);
        let mut shuffled = d.clone();
        shuffled.reverse();
        assert_eq!(select_libri(&shuffled, &sel), select_libri(&d, &sel));

        let all = Selection { count: 10, ..sel };
        assert_eq!(
            select_libri(&d, &all),
            vec!["61-70968-0000", "121-121726-0000", "237-126133-0001", "1089-134686-0001"]
        );
    }

    #[test]
    fn clip_set_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = ClipSet::load_or_new(tmp.path(), "personal").unwrap();
        assert!(s.clips.is_empty());
        s.clips.push(Clip {
            id: "p01".into(),
            file: "p01.wav".into(),
            reference: "Hello.".into(),
            tags: vec!["slack".into()],
            duration_ms: 1200,
        });
        s.save(tmp.path()).unwrap();
        let again = ClipSet::load(tmp.path()).unwrap();
        assert_eq!(again.clips.len(), 1);
        assert_eq!(again.clips[0].reference, "Hello.");
    }

    #[test]
    fn builds_libri_set_from_tarball() {
        // Two tiny synthetic "LibriSpeech" utterances in a tar.gz.
        let tmp = tempfile::tempdir().unwrap();
        let tarball = tmp.path().join("t.tar.gz");
        let mut wav_src = Vec::new();
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(&mut wav_src, flate2::Compression::fast()));
        let flac = include_bytes!("../tests/fixtures/silence-6s.flac");
        for (path, data) in [
            ("LibriSpeech/test-clean/61/70968/61-70968-0000.flac", &flac[..]),
            ("LibriSpeech/test-clean/121/121726/121-121726-0000.flac", &flac[..]),
            ("LibriSpeech/test-clean/61/70968/61-70968.trans.txt", b"61-70968-0000 HELLO THERE\n".as_slice()),
            ("LibriSpeech/test-clean/121/121726/121-121726.trans.txt", b"121-121726-0000 GOOD MORNING\n".as_slice()),
        ] {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            builder.append_data(&mut h, path, data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
        fs::write(&tarball, wav_src).unwrap();

        let out = tmp.path().join("libri");
        let sel = Selection { count: 2, min_seconds: 5.0, max_seconds: 15.0 };
        let set = build_libri_set(&tarball, &out, &sel, &mut |_| {}).unwrap();
        assert_eq!(set.clips.len(), 2);
        assert_eq!(set.clips[0].id, "61-70968-0000");
        assert_eq!(set.clips[0].reference, "HELLO THERE");
        assert!((5900..=6100).contains(&set.clips[0].duration_ms));
        assert!(out.join("121-121726-0000.flac").is_file());
        assert_eq!(ClipSet::load(&out).unwrap().clips.len(), 2);
    }
}
