//! One-time model download. **The only network code in the workspace.**
//!
//! HTTPS only, resumable (`Range`), SHA-256 checked while streaming, written to
//! `<file>.part` and renamed into place only after the hash matches.

use crate::{io_err, ModelEntry, ModelError, ModelStore, Result};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const USER_AGENT: &str = concat!("murmur-models/", env!("CARGO_PKG_VERSION"));

/// Progress callback: `(bytes done, total bytes if known)`.
pub type Progress<'a> = &'a mut dyn FnMut(u64, Option<u64>);

/// What a file is expected to hash to. `None` = unpinned (only `lock` uses that).
#[derive(Debug, Clone, Copy)]
pub struct Expected<'a> {
    pub sha256: &'a str,
    pub size: u64,
}

pub struct Downloader {
    agent: ureq::Agent,
    allow_http: bool,
}

impl Default for Downloader {
    fn default() -> Self {
        Self::new()
    }
}

impl Downloader {
    pub fn new() -> Self {
        Self::build(false)
    }

    /// Plain-HTTP allowed. For tests against a local server only.
    #[doc(hidden)]
    pub fn insecure_for_tests() -> Self {
        Self::build(true)
    }

    fn build(allow_http: bool) -> Self {
        // Trust the OS certificate store (works behind corporate TLS-inspecting proxies).
        let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
        let config = ureq::Agent::config_builder()
            .tls_config(tls)
            .https_only(!allow_http)
            .user_agent(USER_AGENT)
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            // Status codes are handled explicitly (206 vs 200 vs 416).
            .http_status_as_error(false)
            .build();
        Self { agent: config.into(), allow_http }
    }

    /// Download `url` to `dest`. If `dest` already exists and matches `expected`,
    /// nothing is fetched. Returns the file's `(sha256, size)`.
    pub fn download_file(
        &self,
        url: &str,
        dest: &Path,
        expected: Option<Expected<'_>>,
        progress: Progress<'_>,
    ) -> Result<(String, u64)> {
        if !url.starts_with("https://") && !(self.allow_http && url.starts_with("http://")) {
            return Err(ModelError::InsecureUrl(url.to_string()));
        }
        if dest.is_file() {
            let (sha, size) = crate::sha256_file(dest)?;
            match expected {
                Some(e) if e.sha256 == sha && e.size == size => return Ok((sha, size)),
                None => return Ok((sha, size)),
                Some(_) => fs::remove_file(dest).map_err(io_err(dest))?,
            }
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(io_err(parent))?;
        }

        let part = part_path(dest);
        let (sha, size) = self.fetch_to_part(url, &part, expected.map(|e| e.size), progress)?;

        if let Some(e) = expected {
            if size != e.size || sha != e.sha256 {
                let _ = fs::remove_file(&part);
                let file = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                return Err(if size != e.size {
                    ModelError::SizeMismatch { file, expected: e.size, actual: size }
                } else {
                    ModelError::HashMismatch { file, expected: e.sha256.to_string(), actual: sha }
                });
            }
        }
        fs::rename(&part, dest).map_err(io_err(dest))?;
        Ok((sha, size))
    }

    /// Stream `url` into `part`, resuming from its current length when the server
    /// supports ranges. Returns the hash and size of the complete `.part` file.
    fn fetch_to_part(
        &self,
        url: &str,
        part: &Path,
        expected_size: Option<u64>,
        progress: Progress<'_>,
    ) -> Result<(String, u64)> {
        let mut have = fs::metadata(part).map(|m| m.len()).unwrap_or(0);
        if let Some(size) = expected_size {
            if have > size {
                fs::remove_file(part).map_err(io_err(part))?;
                have = 0;
            }
        }

        let mut req = self.agent.get(url);
        if have > 0 {
            req = req.header("Range", format!("bytes={have}-"));
        }
        let resp = req.call().map_err(|e| ModelError::Http(format!("{url}: {e}")))?;
        let status = resp.status().as_u16();

        let (mut file, mut hasher, start) = match status {
            206 if have > 0 => {
                let hasher = hash_prefix(part)?;
                let file = OpenOptions::new().append(true).open(part).map_err(io_err(part))?;
                (file, hasher, have)
            }
            // Range not satisfiable: the part file is already complete.
            416 if have > 0 => return crate::sha256_file(part),
            200 => (File::create(part).map_err(io_err(part))?, Sha256::new(), 0),
            other => return Err(ModelError::Http(format!("{url}: HTTP {other}"))),
        };

        let total = resp
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .map(|len| len + start)
            .or(expected_size);

        let mut reader = resp.into_body().into_reader();
        let mut buf = vec![0u8; 1 << 16];
        let mut done = start;
        progress(done, total);
        loop {
            let n = reader.read(&mut buf).map_err(|e| ModelError::Http(format!("{url}: {e}")))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).map_err(io_err(part))?;
            hasher.update(&buf[..n]);
            done += n as u64;
            progress(done, total);
        }
        file.sync_all().map_err(io_err(part))?;
        Ok((hex::encode(hasher.finalize()), done))
    }

    /// Resolve a Hugging Face branch/tag (e.g. `main`) to its commit SHA.
    pub fn hf_resolve_revision(&self, repo: &str, rev: &str) -> Result<String> {
        let url = format!("https://huggingface.co/api/models/{repo}/revision/{rev}");
        let mut resp = self.agent.get(&url).call().map_err(|e| ModelError::Http(format!("{url}: {e}")))?;
        if resp.status().as_u16() != 200 {
            return Err(ModelError::Http(format!("{url}: HTTP {}", resp.status())));
        }
        let body = resp.body_mut().read_to_string().map_err(|e| ModelError::Http(format!("{url}: {e}")))?;
        let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| ModelError::Http(format!("{url}: {e}")))?;
        v.get("sha")
            .and_then(|s| s.as_str())
            .map(str::to_string)
            .ok_or_else(|| ModelError::Http(format!("{url}: no `sha` in response")))
    }
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".part");
    dest.with_file_name(name)
}

fn hash_prefix(path: &Path) -> Result<Sha256> {
    let mut f = File::open(path).map_err(io_err(path))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(io_err(path))?;
        if n == 0 {
            return Ok(hasher);
        }
        hasher.update(&buf[..n]);
    }
}

/// The locked values for one model, as discovered by [`ModelStore::lock`].
#[derive(Debug, Clone)]
pub struct LockedModel {
    pub id: String,
    pub revision: String,
    /// `(file name, sha256, size)`.
    pub files: Vec<(String, String, u64)>,
}

impl ModelStore {
    /// Download + verify every file of a locked model, then write the marker.
    pub fn download(
        &self,
        entry: &ModelEntry,
        dl: &Downloader,
        progress: &mut dyn FnMut(&str, u64, Option<u64>),
    ) -> Result<()> {
        if !entry.is_locked() {
            return Err(ModelError::NotLocked(entry.id.clone()));
        }
        for f in &entry.files {
            let url = entry.url_at(f, None)?;
            let expected = Expected { sha256: &f.sha256, size: f.size };
            let name = f.name.as_str();
            dl.download_file(&url, &self.file_path(entry, f), Some(expected), &mut |d, t| progress(name, d, t))?;
        }
        self.verify(entry)
    }

    /// Dev/CI only: pin a model. Resolves the source revision (unless already
    /// pinned and `relock` is false), downloads every file unverified into the
    /// store, and reports their hashes. Never writes the verification marker.
    pub fn lock(
        &self,
        entry: &ModelEntry,
        dl: &Downloader,
        relock: bool,
        progress: &mut dyn FnMut(&str, u64, Option<u64>),
    ) -> Result<LockedModel> {
        let revision = match (&entry.source.hf_repo, entry.source.revision.as_str()) {
            (Some(repo), rev) if rev.is_empty() || relock => dl.hf_resolve_revision(repo, "main")?,
            (_, rev) => rev.to_string(),
        };
        let mut files = Vec::new();
        for f in &entry.files {
            let url = entry.url_at(f, Some(&revision))?;
            let dest = self.file_path(entry, f);
            let name = f.name.as_str();
            let (sha, size) = dl.download_file(&url, &dest, None, &mut |d, t| progress(name, d, t))?;
            files.push((f.name.clone(), sha, size));
        }
        Ok(LockedModel { id: entry.id.clone(), revision, files })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use crate::ModelStatus;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;

    /// Serves `body` at every path, honouring `Range: bytes=N-`. Counts requests.
    fn serve(body: Vec<u8>) -> (String, Arc<AtomicUsize>, thread::JoinHandle<()>) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr().to_ip().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits2 = hits.clone();
        let handle = thread::spawn(move || {
            for req in server.incoming_requests() {
                if req.url() == "/stop" {
                    let _ = req.respond(tiny_http::Response::empty(200));
                    break;
                }
                hits2.fetch_add(1, Ordering::SeqCst);
                let range_start =
                    req.headers().iter().find(|h| h.field.equiv("Range")).and_then(|h| {
                        h.value.as_str().strip_prefix("bytes=")?.strip_suffix('-')?.parse::<usize>().ok()
                    });
                let resp = match range_start {
                    Some(s) if s >= body.len() => tiny_http::Response::from_data(Vec::new()).with_status_code(416),
                    Some(s) => tiny_http::Response::from_data(body[s..].to_vec()).with_status_code(206),
                    None => tiny_http::Response::from_data(body.clone()),
                };
                let _ = req.respond(resp);
            }
        });
        (format!("http://{addr}"), hits, handle)
    }

    fn stop(base: &str, handle: thread::JoinHandle<()>) {
        let _ = Downloader::insecure_for_tests().agent.get(format!("{base}/stop")).call();
        handle.join().unwrap();
    }

    fn body() -> Vec<u8> {
        (0..200_000u32).map(|i| (i % 251) as u8).collect()
    }

    /// Real HTTPS through the OS trust store and any HTTPS_PROXY. Network: run with `--ignored`.
    #[test]
    #[ignore]
    fn live_https_download() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("LICENSE");
        let (sha, size) = Downloader::new()
            .download_file(
                "https://raw.githubusercontent.com/rust-lang/rust/master/LICENSE-MIT",
                &dest,
                None,
                &mut |_, _| {},
            )
            .unwrap();
        assert!(size > 500 && sha.len() == 64);
        assert!(fs::read_to_string(&dest).unwrap().contains("Permission is hereby granted"));
    }

    #[test]
    fn refuses_plain_http_by_default() {
        let tmp = tempfile::tempdir().unwrap();
        let err = Downloader::new()
            .download_file("http://example.com/x", &tmp.path().join("x"), None, &mut |_, _| {})
            .unwrap_err();
        assert!(matches!(err, ModelError::InsecureUrl(_)));
    }

    #[test]
    fn downloads_verifies_and_skips_when_present() {
        let data = body();
        let (base, hits, h) = serve(data.clone());
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("m.bin");
        let sha = sha_hex(&data);
        let exp = Expected { sha256: &sha, size: data.len() as u64 };
        let dl = Downloader::insecure_for_tests();
        let mut last = 0;
        let (got, size) = dl.download_file(&format!("{base}/m.bin"), &dest, Some(exp), &mut |d, _| last = d).unwrap();
        assert_eq!((got.as_str(), size), (sha.as_str(), data.len() as u64));
        assert_eq!(last, data.len() as u64);
        assert_eq!(fs::read(&dest).unwrap(), data);
        assert!(!part_path(&dest).exists());

        dl.download_file(&format!("{base}/m.bin"), &dest, Some(exp), &mut |_, _| {}).unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 1, "second call must not hit the network");
        stop(&base, h);
    }

    #[test]
    fn resumes_from_partial_file() {
        let data = body();
        let (base, _hits, h) = serve(data.clone());
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("m.bin");
        fs::write(part_path(&dest), &data[..70_000]).unwrap();
        let sha = sha_hex(&data);
        let exp = Expected { sha256: &sha, size: data.len() as u64 };
        let mut first = None;
        Downloader::insecure_for_tests()
            .download_file(&format!("{base}/m.bin"), &dest, Some(exp), &mut |d, _| {
                first.get_or_insert(d);
            })
            .unwrap();
        assert_eq!(first, Some(70_000), "should resume, not restart");
        assert_eq!(fs::read(&dest).unwrap(), data);
        stop(&base, h);
    }

    #[test]
    fn hash_mismatch_deletes_part_and_never_installs() {
        let data = body();
        let (base, _hits, h) = serve(data.clone());
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("m.bin");
        let wrong = sha_hex(b"not the file");
        let err = Downloader::insecure_for_tests()
            .download_file(
                &format!("{base}/m.bin"),
                &dest,
                Some(Expected { sha256: &wrong, size: data.len() as u64 }),
                &mut |_, _| {},
            )
            .unwrap_err();
        assert!(matches!(err, ModelError::HashMismatch { .. }));
        assert!(!dest.exists());
        assert!(!part_path(&dest).exists());
        stop(&base, h);
    }

    #[test]
    fn store_download_writes_marker() {
        let data = body();
        let (base, _hits, h) = serve(data.clone());
        let tmp = tempfile::tempdir().unwrap();
        let store = ModelStore::new(tmp.path());
        let mut e = entry_for("m", "weights.bin", &data);
        e.files[0].url = Some(format!("{base}/weights.bin"));
        store.download(&e, &Downloader::insecure_for_tests(), &mut |_, _, _| {}).unwrap();
        assert_eq!(store.status(&e), ModelStatus::Verified);
        stop(&base, h);
    }
}
