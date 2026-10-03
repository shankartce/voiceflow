//! Model manifest, on-disk store, verification and offline import.
//!
//! This crate is the **only** place network code may live (`download` feature).
//! Everything else in the workspace receives verified model paths from here.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[cfg(feature = "download")]
pub mod download;

/// Directory name under `%LOCALAPPDATA%` / `%APPDATA%` (Windows) or the XDG data
/// dir (Linux). The product name is not final; this is the one constant to change.
/// (Moves to `vt-storage` when that crate exists in P2.)
pub const APP_DIR_NAME: &str = "Murmur";

/// Name of the marker written next to a model's files once they are verified.
pub const VERIFIED_MARKER: &str = ".verified";

const MANIFEST_TOML: &str = include_str!("../models.toml");

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("unknown model id `{0}`")]
    UnknownModel(String),
    #[error("model manifest is invalid: {0}")]
    Manifest(String),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{file}: SHA-256 mismatch (expected {expected}, got {actual})")]
    HashMismatch { file: String, expected: String, actual: String },
    #[error("{file}: size mismatch (expected {expected} bytes, got {actual})")]
    SizeMismatch { file: String, expected: u64, actual: u64 },
    #[error("model `{0}` is not locked in the manifest (missing revision or SHA-256); run `vt-bench manifest lock`")]
    NotLocked(String),
    #[error(
        "model `{0}` is not downloaded or not verified; run `vt-bench fetch {0}` or `vt-bench import {0} <folder>`"
    )]
    NotVerified(String),
    #[error("model `{id}` has no file with role `{role}`")]
    MissingRole { id: String, role: String },
    #[error("could not determine a data directory; set MURMUR_DATA_DIR")]
    NoDataDir,
    #[error("download failed: {0}")]
    Http(String),
    #[error("refusing non-HTTPS URL: {0}")]
    InsecureUrl(String),
}

pub type Result<T, E = ModelError> = std::result::Result<T, E>;

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> ModelError + '_ {
    move |source| ModelError::Io { path: path.to_path_buf(), source }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelKind {
    Stt,
    Vad,
    Llm,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Source {
    /// Hugging Face repository (`owner/name`).
    pub hf_repo: Option<String>,
    /// Immutable commit SHA. Empty = not yet locked.
    #[serde(default)]
    pub revision: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelFile {
    pub name: String,
    /// What the engine uses this file for (`encoder`, `tokens`, `model`, …).
    pub role: String,
    /// Lower-case hex SHA-256. Empty = not yet locked.
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
    /// Explicit URL; overrides the one derived from `source`.
    pub url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    pub kind: ModelKind,
    /// Which `vt-stt` implementation loads it (`sherpa-nemo-transducer`, `whisper-cpp`, …).
    pub engine: String,
    pub license: String,
    pub attribution: String,
    #[serde(default)]
    pub approx_mb: u32,
    pub source: Source,
    #[serde(rename = "file")]
    pub files: Vec<ModelFile>,
}

impl ModelEntry {
    /// True when every file has a hash and the source a pinned revision.
    pub fn is_locked(&self) -> bool {
        let rev_ok = self.source.hf_repo.is_none() || !self.source.revision.is_empty();
        rev_ok && self.files.iter().all(|f| is_sha256_hex(&f.sha256) && f.size > 0)
    }

    /// Download URL for `file` at `revision` (falls back to the pinned revision, then `main`).
    pub fn url_at(&self, file: &ModelFile, revision: Option<&str>) -> Result<String> {
        if let Some(url) = &file.url {
            return Ok(url.clone());
        }
        let repo = self.source.hf_repo.as_deref().ok_or_else(|| {
            ModelError::Manifest(format!("{}: file {} has no url and no hf_repo", self.id, file.name))
        })?;
        let rev = match revision {
            Some(r) => r,
            None if !self.source.revision.is_empty() => self.source.revision.as_str(),
            None => "main",
        };
        Ok(format!("https://huggingface.co/{repo}/resolve/{rev}/{}", file.name))
    }

    pub fn file_by_role(&self, role: &str) -> Option<&ModelFile> {
        self.files.iter().find(|f| f.role == role)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    #[serde(rename = "model")]
    pub models: Vec<ModelEntry>,
}

impl Manifest {
    pub fn parse(toml_src: &str) -> Result<Self> {
        let m: Manifest = toml::from_str(toml_src).map_err(|e| ModelError::Manifest(e.to_string()))?;
        m.validate()?;
        Ok(m)
    }

    fn validate(&self) -> Result<()> {
        let mut seen = std::collections::HashSet::new();
        for m in &self.models {
            if !seen.insert(m.id.as_str()) {
                return Err(ModelError::Manifest(format!("duplicate id `{}`", m.id)));
            }
            if m.files.is_empty() {
                return Err(ModelError::Manifest(format!("`{}` lists no files", m.id)));
            }
            for f in &m.files {
                if f.name.contains(['/', '\\']) || f.name.starts_with('.') {
                    return Err(ModelError::Manifest(format!("`{}`: bad file name `{}`", m.id, f.name)));
                }
                if !f.sha256.is_empty() && !is_sha256_hex(&f.sha256) {
                    return Err(ModelError::Manifest(format!("`{}`: bad sha256 for `{}`", m.id, f.name)));
                }
            }
        }
        Ok(())
    }

    pub fn get(&self, id: &str) -> Result<&ModelEntry> {
        self.models.iter().find(|m| m.id == id).ok_or_else(|| ModelError::UnknownModel(id.to_string()))
    }
}

/// The manifest compiled into this binary.
pub fn manifest() -> Result<&'static Manifest> {
    static CELL: OnceLock<std::result::Result<Manifest, String>> = OnceLock::new();
    CELL.get_or_init(|| Manifest::parse(MANIFEST_TOML).map_err(|e| e.to_string()))
        .as_ref()
        .map_err(|e| ModelError::Manifest(e.clone()))
}

/// Raw manifest source (for `vt-bench manifest show`).
pub fn manifest_source() -> &'static str {
    MANIFEST_TOML
}

/// `MURMUR_DATA_DIR`, else `%LOCALAPPDATA%\Murmur` (Windows) / `~/.local/share/Murmur` (Linux).
pub fn default_data_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("MURMUR_DATA_DIR") {
        return Ok(PathBuf::from(dir));
    }
    directories::BaseDirs::new().map(|b| b.data_local_dir().join(APP_DIR_NAME)).ok_or(ModelError::NoDataDir)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelStatus {
    /// At least one file is absent.
    Missing,
    /// All files present, but not verified against the manifest.
    Unverified,
    /// Files present and the verification marker matches the manifest.
    Verified,
}

/// Resolved paths of a verified model, keyed by file role.
#[derive(Debug, Clone)]
pub struct ModelPaths {
    pub id: String,
    pub engine: String,
    pub dir: PathBuf,
    files: Vec<(String, PathBuf)>,
}

impl ModelPaths {
    pub fn role(&self, role: &str) -> Result<&Path> {
        self.files
            .iter()
            .find(|(r, _)| r == role)
            .map(|(_, p)| p.as_path())
            .ok_or_else(|| ModelError::MissingRole { id: self.id.clone(), role: role.to_string() })
    }
}

/// On-disk model store: `<root>/<model id>/<file name>`.
#[derive(Debug, Clone)]
pub struct ModelStore {
    root: PathBuf,
}

impl ModelStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `<data dir>/models`.
    pub fn default_location() -> Result<Self> {
        Ok(Self::new(default_data_dir()?.join("models")))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn dir(&self, entry: &ModelEntry) -> PathBuf {
        self.root.join(&entry.id)
    }

    pub fn file_path(&self, entry: &ModelEntry, file: &ModelFile) -> PathBuf {
        self.dir(entry).join(&file.name)
    }

    fn marker_path(&self, entry: &ModelEntry) -> PathBuf {
        self.dir(entry).join(VERIFIED_MARKER)
    }

    pub fn status(&self, entry: &ModelEntry) -> ModelStatus {
        if !entry.files.iter().all(|f| self.file_path(entry, f).is_file()) {
            return ModelStatus::Missing;
        }
        match fs::read_to_string(self.marker_path(entry)) {
            Ok(content) if entry.is_locked() && content == marker_content(entry) => ModelStatus::Verified,
            _ => ModelStatus::Unverified,
        }
    }

    /// Hash every file against the manifest and write the verification marker.
    /// Removes a stale marker first, so a failed verification never leaves one behind.
    pub fn verify(&self, entry: &ModelEntry) -> Result<()> {
        if !entry.is_locked() {
            return Err(ModelError::NotLocked(entry.id.clone()));
        }
        let marker = self.marker_path(entry);
        if marker.exists() {
            fs::remove_file(&marker).map_err(io_err(&marker))?;
        }
        for f in &entry.files {
            let path = self.file_path(entry, f);
            check_file(&path, f)?;
        }
        write_atomic(&marker, marker_content(entry).as_bytes())
    }

    /// Paths of a verified model. Cheap: checks the marker, does not re-hash.
    pub fn paths(&self, entry: &ModelEntry) -> Result<ModelPaths> {
        if self.status(entry) != ModelStatus::Verified {
            return Err(ModelError::NotVerified(entry.id.clone()));
        }
        Ok(ModelPaths {
            id: entry.id.clone(),
            engine: entry.engine.clone(),
            dir: self.dir(entry),
            files: entry.files.iter().map(|f| (f.role.clone(), self.file_path(entry, f))).collect(),
        })
    }

    /// Offline install: copy the model's files from `src` (a folder containing
    /// them by name) into the store, then verify.
    pub fn import_from_dir(&self, entry: &ModelEntry, src: &Path) -> Result<()> {
        if !entry.is_locked() {
            return Err(ModelError::NotLocked(entry.id.clone()));
        }
        let dir = self.dir(entry);
        fs::create_dir_all(&dir).map_err(io_err(&dir))?;
        for f in &entry.files {
            let from = src.join(&f.name);
            check_file(&from, f)?;
            let to = self.file_path(entry, f);
            let tmp = to.with_extension("import-tmp");
            fs::copy(&from, &tmp).map_err(io_err(&from))?;
            fs::rename(&tmp, &to).map_err(io_err(&to))?;
        }
        self.verify(entry)
    }
}

fn marker_content(entry: &ModelEntry) -> String {
    let mut s = String::new();
    for f in &entry.files {
        s.push_str(&f.sha256);
        s.push_str("  ");
        s.push_str(&f.name);
        s.push('\n');
    }
    s
}

fn check_file(path: &Path, f: &ModelFile) -> Result<()> {
    let (actual, size) = sha256_file(path)?;
    if size != f.size {
        return Err(ModelError::SizeMismatch { file: f.name.clone(), expected: f.size, actual: size });
    }
    if actual != f.sha256 {
        return Err(ModelError::HashMismatch { file: f.name.clone(), expected: f.sha256.clone(), actual });
    }
    Ok(())
}

/// Lower-case hex SHA-256 and byte length of a file.
pub fn sha256_file(path: &Path) -> Result<(String, u64)> {
    let mut file = File::open(path).map_err(io_err(path))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buf).map_err(io_err(path))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    Ok((hex::encode(hasher.finalize()), total))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = File::create(&tmp).map_err(io_err(&tmp))?;
        f.write_all(bytes).map_err(io_err(&tmp))?;
        f.sync_all().map_err(io_err(&tmp))?;
    }
    fs::rename(&tmp, path).map_err(io_err(path))
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub fn sha_hex(bytes: &[u8]) -> String {
        hex::encode(Sha256::digest(bytes))
    }

    /// A one-file entry whose content is `bytes`.
    pub fn entry_for(id: &str, name: &str, bytes: &[u8]) -> ModelEntry {
        ModelEntry {
            id: id.into(),
            kind: ModelKind::Stt,
            engine: "test".into(),
            license: "MIT".into(),
            attribution: "test".into(),
            approx_mb: 0,
            source: Source { hf_repo: Some("owner/repo".into()), revision: "0123456789abcdef".into() },
            files: vec![ModelFile {
                name: name.into(),
                role: "model".into(),
                sha256: sha_hex(bytes),
                size: bytes.len() as u64,
                url: None,
            }],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    #[test]
    fn embedded_manifest_parses_with_unique_ids() {
        let m = manifest().unwrap();
        assert!(m.models.len() >= 4);
        for id in
            ["parakeet-tdt-0.6b-v2-int8", "moonshine-base-en-int8", "whisper-base.en-q5_1", "whisper-small.en-q5_1"]
        {
            assert!(m.get(id).is_ok(), "{id} missing");
        }
    }

    #[test]
    fn engine_roles_present() {
        let m = manifest().unwrap();
        let p = m.get("parakeet-tdt-0.6b-v2-int8").unwrap();
        for role in ["encoder", "decoder", "joiner", "tokens"] {
            assert!(p.file_by_role(role).is_some(), "{role}");
        }
        assert!(m.get("whisper-base.en-q5_1").unwrap().file_by_role("model").is_some());
    }

    #[test]
    fn urls_use_pinned_revision_or_main() {
        let m = manifest().unwrap();
        let w = m.get("whisper-base.en-q5_1").unwrap();
        let f = &w.files[0];
        let url = w.url_at(f, Some("abc123")).unwrap();
        assert_eq!(url, "https://huggingface.co/ggerganov/whisper.cpp/resolve/abc123/ggml-base.en-q5_1.bin");
        let e = entry_for("x", "m.bin", b"hi");
        assert!(e.url_at(&e.files[0], None).unwrap().contains("/resolve/0123456789abcdef/"));
    }

    #[test]
    fn rejects_bad_manifests() {
        let dup = r#"
[[model]]
id = "a"
kind = "stt"
engine = "e"
license = "MIT"
attribution = "x"
source = { hf_repo = "o/r" }
  [[model.file]]
  name = "f"
  role = "model"
[[model]]
id = "a"
kind = "stt"
engine = "e"
license = "MIT"
attribution = "x"
source = { hf_repo = "o/r" }
  [[model.file]]
  name = "f"
  role = "model"
"#;
        assert!(matches!(Manifest::parse(dup), Err(ModelError::Manifest(_))));
        let traversal = dup.replacen("name = \"f\"", "name = \"../evil\"", 1);
        let traversal = traversal.replacen("id = \"a\"", "id = \"b\"", 1);
        assert!(matches!(Manifest::parse(&traversal), Err(ModelError::Manifest(_))));
    }

    #[test]
    fn shipped_manifest_is_fully_pinned() {
        for e in &manifest().unwrap().models {
            assert!(e.is_locked(), "{} has an empty revision/sha256/size; run `vt-bench manifest lock`", e.id);
            assert_eq!(e.source.revision.len(), 40, "{}: revision must be a full commit SHA", e.id);
        }
    }

    #[test]
    fn unlocked_entry_is_refused() {
        let m = manifest().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let store = ModelStore::new(tmp.path());
        let unlocked = m.models.iter().find(|e| !e.is_locked());
        if let Some(e) = unlocked {
            assert!(matches!(store.verify(e), Err(ModelError::NotLocked(_))));
        }
    }

    #[test]
    fn verify_writes_marker_and_detects_tampering() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ModelStore::new(tmp.path());
        let e = entry_for("m", "weights.bin", b"hello model");
        assert_eq!(store.status(&e), ModelStatus::Missing);
        assert!(matches!(store.paths(&e), Err(ModelError::NotVerified(_))));

        fs::create_dir_all(store.dir(&e)).unwrap();
        fs::write(store.file_path(&e, &e.files[0]), b"hello model").unwrap();
        assert_eq!(store.status(&e), ModelStatus::Unverified);
        store.verify(&e).unwrap();
        assert_eq!(store.status(&e), ModelStatus::Verified);
        let paths = store.paths(&e).unwrap();
        assert!(paths.role("model").unwrap().ends_with("weights.bin"));
        assert!(paths.role("encoder").is_err());

        // Same size, different bytes: verification must fail and drop the marker.
        fs::write(store.file_path(&e, &e.files[0]), b"HELLO MODEL").unwrap();
        assert!(matches!(store.verify(&e), Err(ModelError::HashMismatch { .. })));
        assert_eq!(store.status(&e), ModelStatus::Unverified);
    }

    #[test]
    fn import_copies_and_verifies() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        let e = entry_for("m", "weights.bin", b"offline bytes");
        fs::write(src.path().join("weights.bin"), b"offline bytes").unwrap();
        let store = ModelStore::new(dst.path());
        store.import_from_dir(&e, src.path()).unwrap();
        assert_eq!(store.status(&e), ModelStatus::Verified);

        let bad = entry_for("m2", "weights.bin", b"something else!");
        assert!(store.import_from_dir(&bad, src.path()).is_err());
        assert_eq!(store.status(&bad), ModelStatus::Missing);
    }
}
