//! Model package management: download (multi-source, resumable), verify
//! (pinned sha256), unpack (tar.bz2), and expose per-model state.

pub mod manifest;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context};
use manifest::ModelSpec;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

/// Externally visible state of one model package.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ModelState {
    Missing,
    Downloading { percent: u8 },
    Unpacking,
    Ready,
    Error { message: String },
}

pub struct ModelManager {
    root: PathBuf,
    client: reqwest::Client,
    states: std::sync::Mutex<HashMap<String, ModelState>>,
    /// Per-model ensure() serialization so concurrent requests download once.
    locks: tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl ModelManager {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            client: reqwest::Client::new(),
            states: std::sync::Mutex::new(HashMap::new()),
            locks: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    pub fn dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    /// Current state. Checks disk lazily: a present marker means Ready even
    /// after process restart (downloads survive restarts by design).
    pub fn state(&self, spec: &ModelSpec) -> ModelState {
        if let Some(s) = self.states.lock().unwrap_or_else(|e| e.into_inner()).get(spec.id) {
            return s.clone();
        }
        if self.dir(spec.id).join(spec.marker).exists() {
            ModelState::Ready
        } else {
            ModelState::Missing
        }
    }

    fn set_state(&self, id: &str, s: ModelState) {
        self.states.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), s);
    }

    /// Ensure the model is downloaded+unpacked. Safe to call concurrently.
    pub async fn ensure(&self, spec: &ModelSpec) -> anyhow::Result<PathBuf> {
        let lock = {
            let mut locks = self.locks.lock().await;
            locks
                .entry(spec.id.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock().await;

        let dir = self.dir(spec.id);
        if dir.join(spec.marker).exists() {
            self.set_state(spec.id, ModelState::Ready);
            return Ok(dir);
        }
        tokio::fs::create_dir_all(&self.root).await?;
        let part = self.root.join(format!("{}.tar.bz2.part", spec.id));

        let mut last_err = anyhow::anyhow!("no sources configured");
        for url in spec.urls {
            self.set_state(spec.id, ModelState::Downloading { percent: 0 });
            match self.download_resumable(url, &part, spec.id).await {
                Ok(()) => match verify_sha256(&part, spec.sha256).await {
                    Ok(()) => {
                        self.set_state(spec.id, ModelState::Unpacking);
                        let unpack_res = unpack_tar_bz2(&part, &dir, spec.marker).await;
                        let _ = tokio::fs::remove_file(&part).await;
                        match unpack_res {
                            Ok(()) => {
                                self.set_state(spec.id, ModelState::Ready);
                                return Ok(dir);
                            }
                            Err(e) => last_err = e.context(format!("unpack from {url}")),
                        }
                    }
                    Err(e) => {
                        // Corrupt download: drop the partial so the next source restarts clean.
                        let _ = tokio::fs::remove_file(&part).await;
                        last_err = e.context(format!("checksum from {url}"));
                    }
                },
                Err(e) => {
                    // Drop the partial: resuming it against a different source
                    // would splice two sources' bytes into one file.
                    let _ = tokio::fs::remove_file(&part).await;
                    last_err = e.context(format!("download from {url}"));
                }
            }
        }
        let message = format!("{last_err:#}");
        self.set_state(spec.id, ModelState::Error { message: message.clone() });
        bail!(message)
    }

    /// Download with HTTP Range resume into `dest`, updating percent state.
    async fn download_resumable(&self, url: &str, dest: &Path, id: &str) -> anyhow::Result<()> {
        let existing = tokio::fs::metadata(dest).await.map(|m| m.len()).unwrap_or(0);
        let mut req = self.client.get(url);
        if existing > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={existing}-"));
        }
        let resp = req.send().await?;
        let status = resp.status();
        let resumed = status == reqwest::StatusCode::PARTIAL_CONTENT;
        if !status.is_success() {
            bail!("HTTP {status}");
        }
        // Server ignored Range → restart from scratch.
        let mut written = if resumed { existing } else { 0 };
        let total = resp
            .content_length()
            .map(|l| l + if resumed { existing } else { 0 })
            .unwrap_or(0);
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(resumed)
            .write(true)
            .truncate(!resumed)
            .open(dest)
            .await?;
        let mut stream = resp.bytes_stream();
        use futures::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            written += chunk.len() as u64;
            if total > 0 {
                let pct = ((written * 100) / total).min(99) as u8;
                self.set_state(id, ModelState::Downloading { percent: pct });
            }
        }
        file.flush().await?;
        Ok(())
    }
}

async fn verify_sha256(path: &Path, expected: &str) -> anyhow::Result<()> {
    let path = path.to_path_buf();
    let expected = expected.to_string();
    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::File::open(&path)?;
        let mut hasher = Sha256::new();
        std::io::copy(&mut file, &mut hasher)?;
        let got = format!("{:x}", hasher.finalize());
        if got != expected.to_lowercase() {
            bail!("sha256 mismatch: got {got}, expected {expected}");
        }
        Ok(())
    })
    .await?
}

/// Unpack a .tar.bz2 into `dest`, stripping the single top-level directory
/// (sherpa packages all nest one root folder). Atomic via tmp dir + rename.
async fn unpack_tar_bz2(archive: &Path, dest: &Path, marker: &str) -> anyhow::Result<()> {
    let archive = archive.to_path_buf();
    let dest = dest.to_path_buf();
    let marker = marker.to_string();
    tokio::task::spawn_blocking(move || {
        // Append (not with_extension): ids like "kokoro-v1.1-zh" contain dots
        // and with_extension would truncate at the last one.
        let tmp_name = format!(
            "{}.unpack-tmp",
            dest.file_name().and_then(|n| n.to_str()).unwrap_or("model")
        );
        let tmp = dest.parent().unwrap_or_else(|| Path::new(".")).join(tmp_name);
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp)?;
        let file = std::fs::File::open(&archive)?;
        let bz = bzip2::read::BzDecoder::new(file);
        tar::Archive::new(bz).unpack(&tmp)?;
        // strip-components=1: find the single root dir (or use tmp directly).
        let mut entries: Vec<_> = std::fs::read_dir(&tmp)?.filter_map(Result::ok).collect();
        let src_root = if entries.len() == 1 && entries[0].path().is_dir() {
            entries.remove(0).path()
        } else {
            tmp.clone()
        };
        if !src_root.join(&marker).exists() {
            bail!("unpacked archive missing marker file {marker}");
        }
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::rename(&src_root, &dest).context("move unpacked model into place")?;
        let _ = std::fs::remove_dir_all(&tmp);
        Ok(())
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::State, http::HeaderMap, response::IntoResponse, routing::get, Router};

    /// Build a tiny valid tar.bz2 containing `root/<marker>` and return (bytes, sha256).
    fn fixture_archive(marker: &str) -> (Vec<u8>, String) {
        let mut tar_bytes = Vec::new();
        {
            let enc = bzip2::write::BzEncoder::new(&mut tar_bytes, bzip2::Compression::fast());
            let mut builder = tar::Builder::new(enc);
            let data = b"model-bytes";
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, format!("pkg/{marker}"), &data[..]).unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }
        let sha = format!("{:x}", Sha256::digest(&tar_bytes));
        (tar_bytes, sha)
    }

    /// Per-request observation: raw Range header (if any) and replied status.
    type RequestLog = Arc<std::sync::Mutex<Vec<(Option<String>, u16)>>>;

    #[derive(Clone)]
    struct ServeCtx {
        data: Arc<Vec<u8>>,
        log: RequestLog,
    }

    /// Range-aware fixture server (serves `bytes` honoring Range headers).
    /// Returns the URL, the server task handle, and a log of observed requests.
    async fn serve(bytes: Vec<u8>) -> (String, tokio::task::JoinHandle<()>, RequestLog) {
        async fn handler(State(ctx): State<ServeCtx>, headers: HeaderMap) -> impl IntoResponse {
            let raw_range = headers
                .get(reqwest::header::RANGE)
                .and_then(|v| v.to_str().ok())
                .map(String::from);
            let start = raw_range
                .as_deref()
                .and_then(|s| s.strip_prefix("bytes="))
                .and_then(|s| s.trim_end_matches('-').parse::<usize>().ok());
            let (status, body) = match start {
                Some(start) if start < ctx.data.len() => {
                    (axum::http::StatusCode::PARTIAL_CONTENT, ctx.data[start..].to_vec())
                }
                _ => (axum::http::StatusCode::OK, ctx.data.as_ref().clone()),
            };
            ctx.log
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((raw_range, status.as_u16()));
            (status, body).into_response()
        }
        let log: RequestLog = Arc::new(std::sync::Mutex::new(Vec::new()));
        let ctx = ServeCtx { data: Arc::new(bytes), log: log.clone() };
        let app = Router::new().route("/m.tar.bz2", get(handler)).with_state(ctx);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let h = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}/m.tar.bz2"), h, log)
    }

    fn leak(s: String) -> &'static str {
        Box::leak(s.into_boxed_str())
    }

    #[tokio::test]
    async fn downloads_verifies_unpacks() {
        let (bytes, sha) = fixture_archive("model.onnx");
        let (url, _h, _log) = serve(bytes).await;
        let tmp = tempfile::tempdir().unwrap();
        let mgr = ModelManager::new(tmp.path().to_path_buf());
        let spec = ModelSpec {
            id: "m1",
            urls: Box::leak(vec![leak(url)].into_boxed_slice()),
            sha256: leak(sha),
            marker: "model.onnx",
        };
        let dir = mgr.ensure(&spec).await.unwrap();
        assert!(dir.join("model.onnx").exists());
        assert_eq!(mgr.state(&spec), ModelState::Ready);
    }

    #[tokio::test]
    async fn resumes_partial_download() {
        let (bytes, sha) = fixture_archive("model.onnx");
        let (url, _h, log) = serve(bytes.clone()).await;
        let tmp = tempfile::tempdir().unwrap();
        let mgr = ModelManager::new(tmp.path().to_path_buf());
        // Pre-write a truncated .part to simulate an interrupted download.
        let half = bytes.len() / 2;
        std::fs::create_dir_all(tmp.path()).unwrap();
        std::fs::write(tmp.path().join("m2.tar.bz2.part"), &bytes[..half]).unwrap();
        let spec = ModelSpec {
            id: "m2",
            urls: Box::leak(vec![leak(url)].into_boxed_slice()),
            sha256: leak(sha),
            marker: "model.onnx",
        };
        let dir = mgr.ensure(&spec).await.unwrap();
        assert!(dir.join("model.onnx").exists(), "resume must complete the file");
        // The request must actually have resumed: Range from the half-point,
        // answered with 206 Partial Content.
        let requests = log.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(requests.len(), 1, "expected exactly one download request");
        assert_eq!(requests[0].0.as_deref(), Some(format!("bytes={half}-").as_str()));
        assert_eq!(requests[0].1, 206, "server must reply 206 to the resume");
    }

    #[tokio::test]
    async fn checksum_mismatch_falls_to_next_source_then_errors() {
        let (bytes, _) = fixture_archive("model.onnx");
        let (url1, _h1, _log1) = serve(bytes.clone()).await;
        let (url2, _h2, _log2) = serve(bytes).await;
        let tmp = tempfile::tempdir().unwrap();
        let mgr = ModelManager::new(tmp.path().to_path_buf());
        let spec = ModelSpec {
            id: "m3",
            urls: Box::leak(vec![leak(url1), leak(url2)].into_boxed_slice()),
            sha256: "deadbeef", // wrong on purpose
            marker: "model.onnx",
        };
        let err = mgr.ensure(&spec).await.unwrap_err();
        assert!(format!("{err:#}").contains("sha256 mismatch"));
        assert!(matches!(mgr.state(&spec), ModelState::Error { .. }));
    }

    #[tokio::test]
    async fn failed_source_drops_stale_part_before_next_source() {
        let (bytes, sha) = fixture_archive("model.onnx");
        let (good, _h, log) = serve(bytes.clone()).await;
        let tmp = tempfile::tempdir().unwrap();
        let mgr = ModelManager::new(tmp.path().to_path_buf());
        // A stale .part whose bytes do NOT match the real tarball prefix
        // (e.g. left over from a different/corrupt source). If the failed
        // first source does not clear it, the good source gets a Range
        // request and splices garbage + tail → checksum mismatch → failure.
        std::fs::create_dir_all(tmp.path()).unwrap();
        std::fs::write(tmp.path().join("m5.tar.bz2.part"), vec![0xAB; bytes.len() / 2]).unwrap();
        let spec = ModelSpec {
            id: "m5",
            urls: Box::leak(
                vec![leak("http://127.0.0.1:1/nope.tar.bz2".into()), leak(good)]
                    .into_boxed_slice(),
            ),
            sha256: leak(sha),
            marker: "model.onnx",
        };
        let dir = mgr.ensure(&spec).await.unwrap();
        assert!(dir.join("model.onnx").exists());
        // The good source must have been asked for the full file, not a resume.
        let requests = log.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, None, "stale .part must be gone → no Range header");
        assert_eq!(requests[0].1, 200);
    }

    #[tokio::test]
    async fn bad_source_falls_through_to_good_one() {
        let (bytes, sha) = fixture_archive("model.onnx");
        let (good, _h, _log) = serve(bytes).await;
        let tmp = tempfile::tempdir().unwrap();
        let mgr = ModelManager::new(tmp.path().to_path_buf());
        let spec = ModelSpec {
            id: "m4",
            urls: Box::leak(
                vec![leak("http://127.0.0.1:1/nope.tar.bz2".into()), leak(good)]
                    .into_boxed_slice(),
            ),
            sha256: leak(sha),
            marker: "model.onnx",
        };
        mgr.ensure(&spec).await.unwrap();
        assert_eq!(mgr.state(&spec), ModelState::Ready);
    }
}
