//! Media cache — downloads/resolves channel attachments to local temp files.
//!
//! Resolution priority:
//! 1. `attachment.data` (inline bytes) → write to temp file
//! 2. `attachment.path` (local path) → use directly (no copy)
//! 3. `attachment.url` (remote) → HTTP GET download (30s timeout)

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use tracing::{debug, warn};

use crate::gateway::channel::Attachment;

/// Maximum file size allowed (20 MB).
const MAX_FILE_SIZE: u64 = 20 * 1024 * 1024;

/// HTTP download timeout.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30);

/// Stale session threshold (1 hour).
const STALE_THRESHOLD: Duration = Duration::from_secs(3600);

/// A resolved attachment cached on disk.
#[derive(Debug, Clone)]
pub struct CachedMedia {
    /// Path to the local file (temp dir or original path).
    pub local_path: PathBuf,
    /// MIME type carried from the original attachment.
    pub mime_type: String,
    /// File size in bytes.
    pub size: u64,
}

/// Errors from cache operations.
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("No source available (data/path/url all None)")]
    NoSource,

    #[error("File exceeds 20 MB limit: {size} bytes")]
    TooLarge { size: u64 },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP download failed: {0}")]
    Download(String),
}

/// Downloads/resolves channel attachments to local temp files and provides
/// base64 encoding for LLM injection.
pub struct MediaCache {
    /// HTTP client reused across downloads.
    client: reqwest::Client,
}

impl MediaCache {
    /// Create a new cache with a shared HTTP client.
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(DOWNLOAD_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self { client }
    }

    /// Resolve an attachment to a local file.
    ///
    /// Returns a [`CachedMedia`] pointing to either the original local path
    /// or a newly created temp file under `<temp_dir>/aleph/media/<session_id>/`.
    pub async fn resolve(
        &self,
        attachment: &Attachment,
        session_id: &str,
    ) -> Result<CachedMedia, CacheError> {
        // Priority 1: inline bytes
        if let Some(ref data) = attachment.data {
            let size = data.len() as u64;
            if size > MAX_FILE_SIZE {
                return Err(CacheError::TooLarge { size });
            }
            let dir = session_dir(session_id);
            std::fs::create_dir_all(&dir)?;
            let filename = attachment
                .filename
                .as_deref()
                .unwrap_or(&attachment.id);
            let path = dir.join(filename);
            std::fs::write(&path, data)?;
            debug!(path = %path.display(), "cached inline attachment");
            return Ok(CachedMedia {
                local_path: path,
                mime_type: attachment.mime_type.clone(),
                size,
            });
        }

        // Priority 2: local path — use directly
        if let Some(ref p) = attachment.path {
            let path = PathBuf::from(p);
            let meta = std::fs::metadata(&path)?;
            let size = meta.len();
            if size > MAX_FILE_SIZE {
                return Err(CacheError::TooLarge { size });
            }
            return Ok(CachedMedia {
                local_path: path,
                mime_type: attachment.mime_type.clone(),
                size,
            });
        }

        // Priority 3: URL download
        if let Some(ref url) = attachment.url {
            let dir = session_dir(session_id);
            std::fs::create_dir_all(&dir)?;

            let resp = self
                .client
                .get(url)
                .send()
                .await
                .map_err(|e| CacheError::Download(e.to_string()))?;

            if !resp.status().is_success() {
                return Err(CacheError::Download(format!(
                    "HTTP {}",
                    resp.status()
                )));
            }

            let bytes = resp
                .bytes()
                .await
                .map_err(|e| CacheError::Download(e.to_string()))?;

            let size = bytes.len() as u64;
            if size > MAX_FILE_SIZE {
                return Err(CacheError::TooLarge { size });
            }

            let filename = attachment
                .filename
                .as_deref()
                .unwrap_or(&attachment.id);
            let path = dir.join(filename);
            std::fs::write(&path, &bytes)?;
            debug!(path = %path.display(), size, "downloaded attachment from URL");
            return Ok(CachedMedia {
                local_path: path,
                mime_type: attachment.mime_type.clone(),
                size,
            });
        }

        Err(CacheError::NoSource)
    }

    /// Read a cached file and return its base64-encoded content.
    pub fn to_base64(cached: &CachedMedia) -> Result<String, CacheError> {
        let bytes = std::fs::read(&cached.local_path)?;
        Ok(BASE64.encode(&bytes))
    }

    /// Remove all cached files for a session.
    pub fn cleanup_session(session_id: &str) -> Result<(), CacheError> {
        let dir = session_dir(session_id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
            debug!(session_id, "cleaned up media cache");
        }
        Ok(())
    }

    /// Remove session directories older than 1 hour (call at startup).
    pub fn cleanup_stale() {
        let base = base_dir();
        let entries = match std::fs::read_dir(&base) {
            Ok(e) => e,
            Err(_) => return, // dir doesn't exist yet — nothing to clean
        };

        let now = SystemTime::now();
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if !meta.is_dir() {
                continue;
            }
            let age = meta
                .modified()
                .ok()
                .and_then(|t| now.duration_since(t).ok())
                .unwrap_or(Duration::ZERO);
            if age > STALE_THRESHOLD {
                if let Err(e) = std::fs::remove_dir_all(entry.path()) {
                    warn!(path = %entry.path().display(), error = %e, "failed to remove stale media dir");
                } else {
                    debug!(path = %entry.path().display(), "removed stale media dir");
                }
            }
        }
    }
}

impl Default for MediaCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Base directory: `<temp_dir>/aleph/media`
fn base_dir() -> PathBuf {
    std::env::temp_dir().join("aleph").join("media")
}

/// Per-session directory: `<temp_dir>/aleph/media/<session_id>`
fn session_dir(session_id: &str) -> PathBuf {
    base_dir().join(session_id)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to build a minimal Attachment with all sources None.
    fn empty_attachment() -> Attachment {
        Attachment {
            id: "test-att-1".into(),
            mime_type: "application/octet-stream".into(),
            filename: Some("test.bin".into()),
            size: None,
            url: None,
            path: None,
            data: None,
        }
    }

    #[tokio::test]
    async fn test_resolve_inline_data() {
        let cache = MediaCache::new();
        let mut att = empty_attachment();
        att.data = Some(vec![1, 2, 3, 4]);
        att.mime_type = "application/octet-stream".into();

        let session_id = "test-inline-resolve";
        let cached = cache.resolve(&att, session_id).await.unwrap();

        assert!(cached.local_path.exists(), "temp file must exist");
        assert_eq!(cached.size, 4);
        assert_eq!(cached.mime_type, "application/octet-stream");

        let content = std::fs::read(&cached.local_path).unwrap();
        assert_eq!(content, vec![1, 2, 3, 4]);

        // Cleanup
        let _ = MediaCache::cleanup_session(session_id);
    }

    #[tokio::test]
    async fn test_resolve_no_source_fails() {
        let cache = MediaCache::new();
        let att = empty_attachment();
        let result = cache.resolve(&att, "test-no-source").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), CacheError::NoSource));
    }

    #[tokio::test]
    async fn test_to_base64() {
        let cache = MediaCache::new();
        let mut att = empty_attachment();
        att.data = Some(vec![72, 101, 108, 108, 111]); // "Hello"

        let session_id = "test-base64";
        let cached = cache.resolve(&att, session_id).await.unwrap();
        let b64 = MediaCache::to_base64(&cached).unwrap();
        assert_eq!(b64, "SGVsbG8="); // base64("Hello")

        let _ = MediaCache::cleanup_session(session_id);
    }
}
