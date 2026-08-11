//! Node-side file commands (execution arm): `file.read` / `file.write`.
//!
//! Bytes go directly to the host filesystem (nodes are execution arms, R1
//! allows this), jailed inside the node session workspace directory: relative
//! paths are joined to the workspace, absolute paths must still fall within the
//! workspace, else rejected. Both sides enforce an 8 MB hard cap + sha256
//! integrity. No LLM reasoning (R7).

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use base64::Engine as _;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::builtin_tools::file_ops::{check_and_resolve_path, get_denied_paths};
use crate::cluster::{CommandDescriptor, NodeCommand};

/// Per-file hard cap (raw bytes). Both sides agree; exceeding it fails fast.
pub const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

/// Hex-encoded sha256. Both sides use the same algorithm for integrity checks.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Resolve the request `path` into the node workspace jail. Relative paths are
/// joined to the workspace; any final canonical path must still fall within the
/// workspace (absolute path escape → reject). Reuses `file_ops` canonicalize +
/// deny-list, then adds one more containment gate (`check_and_resolve_path`
/// itself doesn't enforce containment, only resolves relative paths against a
/// base).
async fn resolve_in_jail(path: &str, workspace_dir: &Path) -> Result<PathBuf, String> {
    tokio::fs::create_dir_all(workspace_dir)
        .await
        .map_err(|e| format!("workspace dir unavailable: {e}"))?;
    // (B4-01) Reject symlinks at the workspace root itself: an attacker who
    // can race the service to delete and re-create `workspace_dir` as a
    // symlink to `/etc` would otherwise have `canonicalize` resolve `root`
    // to `/etc`, and every subsequent `starts_with(&root)` check would
    // happily admit `/etc/passwd`. `symlink_metadata` does NOT follow
    // symlinks, so we can refuse the symlink root before canonicalizing.
    let root_meta = tokio::fs::symlink_metadata(workspace_dir)
        .await
        .map_err(|e| format!("workspace root unresolved: {e}"))?;
    if !root_meta.file_type().is_dir() {
        return Err("workspace root is not a directory".to_string());
    }
    let root = tokio::fs::canonicalize(workspace_dir)
        .await
        .map_err(|e| format!("workspace root unresolved: {e}"))?;
    let resolved = check_and_resolve_path(Path::new(path), &get_denied_paths(), Some(&root))
        .map_err(|e| format!("path rejected: {e}"))?;
    if !resolved.starts_with(&root) {
        return Err("path escapes node workspace".to_string());
    }
    Ok(resolved)
}

/// `file.write`: center pushes bytes to land in the node workspace.
pub struct FileWriteCommand {
    workspace_dir: PathBuf,
}

impl FileWriteCommand {
    #[must_use]
    pub const fn new(workspace_dir: PathBuf) -> Self {
        Self { workspace_dir }
    }
}

#[async_trait]
impl NodeCommand for FileWriteCommand {
    async fn run(&self, args: Value) -> Result<Value, String> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or("file.write: missing string field `path`")?;
        let content_b64 = args
            .get("content_b64")
            .and_then(|v| v.as_str())
            .ok_or("file.write: missing string field `content_b64`")?;
        let expected_sha = args
            .get("sha256")
            .and_then(|v| v.as_str())
            .ok_or("file.write: missing string field `sha256`")?;
        let overwrite = args
            .get("overwrite")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let max_b64_len = MAX_FILE_BYTES * 4 / 3 + 4;
        if content_b64.len() > max_b64_len {
            return Err(format!(
                "file.write: base64 payload exceeds {MAX_FILE_BYTES} byte cap"
            ));
        }

        let bytes = B64
            .decode(content_b64)
            .map_err(|e| format!("file.write: invalid base64: {e}"))?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(format!(
                "file.write: {} bytes exceeds {MAX_FILE_BYTES} cap",
                bytes.len()
            ));
        }
        if sha256_hex(&bytes) != expected_sha {
            return Err("file.write: sha256 mismatch".to_string());
        }

        let dest = resolve_in_jail(path, &self.workspace_dir).await?;
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("file.write: {e}"))?;
        }
        // Atomic create-or-truncate: open with `create_new(!overwrite)` so a
        // racing writer that beat us to the path either wins (we get `AlreadyExists`)
        // or loses (our exclusive create succeeds). This replaces the
        // `try_exists` → `tokio::fs::write` pair that had a TOCTOU window where
        // a concurrent file.write with `overwrite=false` could observe "absent"
        // and then succeed over a freshly-arrived file.
        let mut opts = tokio::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        if !overwrite {
            opts.create_new(true);
        }
        let open_result = opts.open(&dest).await;
        let mut file = match open_result {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err("file.write: target exists (set overwrite)".to_string());
            }
            Err(e) => return Err(format!("file.write: {e}")),
        };
        use tokio::io::AsyncWriteExt;
        file.write_all(&bytes)
            .await
            .map_err(|e| format!("file.write: {e}"))?;
        file.flush().await.map_err(|e| format!("file.write: {e}"))?;
        Ok(json!({ "written": bytes.len() }))
    }

    fn descriptor(&self) -> CommandDescriptor {
        CommandDescriptor {
            name: "file.write".to_string(),
            schema: json!({"type": "object"}),
        }
    }
}

/// `file.read`: center pulls bytes from the node workspace.
pub struct FileReadCommand {
    workspace_dir: PathBuf,
}

impl FileReadCommand {
    #[must_use]
    pub const fn new(workspace_dir: PathBuf) -> Self {
        Self { workspace_dir }
    }
}

#[async_trait]
impl NodeCommand for FileReadCommand {
    async fn run(&self, args: Value) -> Result<Value, String> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or("file.read: missing string field `path`")?;
        let src = resolve_in_jail(path, &self.workspace_dir).await?;
        let metadata = match tokio::fs::metadata(&src).await {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err("file.read: not found".to_string());
            }
            Err(e) => return Err(format!("file.read: {e}")),
        };
        let size = metadata.len();
        if size > MAX_FILE_BYTES as u64 {
            return Err(format!(
                "file.read: {size} bytes exceeds {MAX_FILE_BYTES} cap"
            ));
        }
        // (B4-04) `tokio::fs::read` allocates the full file into memory before
        // the size cap can reject it — an adversarial 10GB file at a known
        // path would drive the node's allocator. Use `File::take(MAX + 1)` so
        // the kernel only delivers `MAX_FILE_BYTES + 1` bytes regardless of
        // the file's actual size, and reject if the buffer is over the cap.
        use tokio::io::AsyncReadExt;
        let mut file = tokio::fs::File::open(&src)
            .await
            .map_err(|e| format!("file.read: {e}"))?;
        let mut buf = Vec::with_capacity(std::cmp::min(size, MAX_FILE_BYTES as u64) as usize);
        let capped = file
            .take((MAX_FILE_BYTES as u64) + 1)
            .read_to_end(&mut buf)
            .await
            .map_err(|e| format!("file.read: {e}"))?;
        let _ = capped; // capped is the byte count actually read into `buf`
        if buf.len() > MAX_FILE_BYTES {
            return Err(format!(
                "file.read: >{MAX_FILE_BYTES} bytes (cap enforced at read)"
            ));
        }
        Ok(json!({
            "content_b64": B64.encode(&buf),
            "sha256": sha256_hex(&buf),
            "size": buf.len(),
        }))
    }

    fn descriptor(&self) -> CommandDescriptor {
        CommandDescriptor {
            name: "file.read".to_string(),
            schema: json!({"type": "object"}),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write as _;

    fn b64(bytes: &[u8]) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[tokio::test]
    async fn file_write_then_read_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().to_path_buf();
        let writer = FileWriteCommand::new(ws.clone());
        let reader = FileReadCommand::new(ws.clone());

        let content = b"hello node bytes\x00\x01\x02";
        let res = writer
            .run(json!({
                "path": "out/data.bin",
                "content_b64": b64(content),
                "sha256": sha256_hex(content),
            }))
            .await
            .expect("write ok");
        assert_eq!(res["written"], content.len());

        let got = reader
            .run(json!({ "path": "out/data.bin" }))
            .await
            .expect("read ok");
        assert_eq!(got["size"], content.len());
        assert_eq!(got["sha256"], sha256_hex(content));
        use base64::Engine as _;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(got["content_b64"].as_str().unwrap())
            .unwrap();
        assert_eq!(decoded, content);
    }

    #[tokio::test]
    async fn file_write_rejects_oversize() {
        let dir = tempfile::tempdir().unwrap();
        let writer = FileWriteCommand::new(dir.path().to_path_buf());
        let big = vec![0u8; MAX_FILE_BYTES + 1];
        let err = writer
            .run(json!({ "path": "big.bin", "content_b64": b64(&big), "sha256": sha256_hex(&big) }))
            .await
            .expect_err("oversize rejected");
        assert!(err.contains("exceeds"), "{err}");
    }

    #[tokio::test]
    async fn file_write_rejects_sha_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let writer = FileWriteCommand::new(dir.path().to_path_buf());
        let err = writer
            .run(json!({ "path": "x.bin", "content_b64": b64(b"abc"), "sha256": "deadbeef" }))
            .await
            .expect_err("sha mismatch rejected");
        assert!(err.contains("sha256 mismatch"), "{err}");
    }

    #[tokio::test]
    async fn file_write_rejects_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let writer = FileWriteCommand::new(dir.path().to_path_buf());
        let err = writer
            .run(json!({
                "path": "../escape.bin",
                "content_b64": b64(b"x"),
                "sha256": sha256_hex(b"x"),
            }))
            .await
            .expect_err("traversal rejected");
        assert!(err.contains("escapes") || err.contains("rejected"), "{err}");
    }

    #[tokio::test]
    async fn file_write_respects_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let writer = FileWriteCommand::new(dir.path().to_path_buf());
        let args =
            json!({ "path": "f.bin", "content_b64": b64(b"v1"), "sha256": sha256_hex(b"v1") });
        writer.run(args.clone()).await.expect("first write ok");

        let err = writer.run(args).await.expect_err("no overwrite by default");
        assert!(err.contains("exists"), "{err}");

        writer
            .run(json!({ "path": "f.bin", "content_b64": b64(b"v2"), "sha256": sha256_hex(b"v2"), "overwrite": true }))
            .await
            .expect("overwrite ok");
    }

    #[tokio::test]
    async fn file_read_rejects_missing() {
        let dir = tempfile::tempdir().unwrap();
        let reader = FileReadCommand::new(dir.path().to_path_buf());
        let err = reader
            .run(json!({ "path": "nope.bin" }))
            .await
            .expect_err("missing rejected");
        assert!(err.contains("not found"), "{err}");
    }

    #[tokio::test]
    async fn file_read_rejects_oversize() {
        let dir = tempfile::tempdir().unwrap();
        let big_path = dir.path().join("big.bin");
        let mut f = std::fs::File::create(&big_path).unwrap();
        f.write_all(&vec![0u8; MAX_FILE_BYTES + 1]).unwrap();
        drop(f);
        let reader = FileReadCommand::new(dir.path().to_path_buf());
        let err = reader
            .run(json!({ "path": "big.bin" }))
            .await
            .expect_err("oversize read rejected");
        assert!(err.contains("exceeds"), "{err}");
    }
}
