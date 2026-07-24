//! Media cache — downloads/resolves channel attachments to local temp files.
//!
//! Resolution priority:
//! 1. `attachment.data` (inline bytes) → write to temp file
//! 2. `attachment.path` (local path) → use directly (no copy)
//! 3. `attachment.url` (remote) → HTTP GET download (60s timeout)

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use tracing::{debug, warn};

use crate::gateway::channel::Attachment;
use crate::gateway::media::{detect_mime, MediaItem};
use crate::security::ssrf::{safe_fetch, SafeFetchRequest, SsrfPolicy};

/// Maximum file size allowed (50 MB — for video files).
const MAX_FILE_SIZE: u64 = 50 * 1024 * 1024;

/// HTTP download timeout (60s — large media files).
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);

/// Stale session threshold (1 hour).
const STALE_THRESHOLD: Duration = Duration::from_secs(3600);

const FALLBACK_FILENAME_PREFIX_LEN: usize = 8;

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

    #[error("File exceeds 50 MB limit: {size} bytes")]
    TooLarge { size: u64 },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP download failed: {0}")]
    Download(String),
}

/// Downloads/resolves channel attachments to local temp files and provides
/// base64 encoding for LLM injection.
pub struct MediaCache;

impl MediaCache {
    /// Create a new cache.
    #[must_use]
    pub fn new() -> Self {
        Self
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
        if let Some(ref data) = attachment.data {
            return Self::resolve_inline(
                data,
                &attachment.id,
                attachment.filename.as_deref(),
                &attachment.mime_type,
                session_id,
            )
            .await;
        }
        if let Some(ref p) = attachment.path {
            return Self::resolve_local_path(p, &attachment.mime_type).await;
        }
        if let Some(ref url) = attachment.url {
            return self
                .resolve_url(
                    url,
                    &attachment.id,
                    attachment.filename.as_deref(),
                    &attachment.mime_type,
                    session_id,
                )
                .await;
        }
        Err(CacheError::NoSource)
    }

    async fn resolve_inline(
        data: &[u8],
        id: &str,
        filename: Option<&str>,
        mime_type: &str,
        session_id: &str,
    ) -> Result<CachedMedia, CacheError> {
        let size = data.len() as u64;
        if size > MAX_FILE_SIZE {
            return Err(CacheError::TooLarge { size });
        }
        let dir = ensure_session_dir(session_id).await?;
        let filename = unique_filename(id, filename);
        let path = dir.join(filename);
        tokio::fs::write(&path, data).await?;
        debug!(path = %path.display(), "cached inline attachment");
        Ok(CachedMedia {
            local_path: path,
            mime_type: mime_type.to_string(),
            size,
        })
    }

    async fn resolve_local_path(path: &str, mime_type: &str) -> Result<CachedMedia, CacheError> {
        let path = expand_tilde(path);
        let meta = tokio::fs::metadata(&path).await?;
        let size = meta.len();
        if size > MAX_FILE_SIZE {
            return Err(CacheError::TooLarge { size });
        }
        Ok(CachedMedia {
            local_path: path,
            mime_type: mime_type.to_string(),
            size,
        })
    }

    async fn resolve_url(
        &self,
        url: &str,
        id: &str,
        filename: Option<&str>,
        mime_type: &str,
        session_id: &str,
    ) -> Result<CachedMedia, CacheError> {
        let dir = ensure_session_dir(session_id).await?;

        let response = safe_fetch(
            url,
            &SsrfPolicy::default(),
            SafeFetchRequest::get(DOWNLOAD_TIMEOUT).with_max_body_bytes(MAX_FILE_SIZE as usize),
        )
        .await
        .map_err(|e| CacheError::Download(e.to_string()))?;

        if !response.status.is_success() {
            return Err(CacheError::Download(format!("HTTP {}", response.status)));
        }

        if let Some(content_length) = response.headers.get(reqwest::header::CONTENT_LENGTH) {
            if let Some(content_length) = content_length.to_str().ok().and_then(|s| s.parse().ok()) {
                if content_length > MAX_FILE_SIZE {
                    return Err(CacheError::TooLarge {
                        size: content_length,
                    });
                }
            }
        }

        let filename = unique_filename(id, filename);
        let path = dir.join(&filename);
        let total_size = response.body.len() as u64;
        tokio::fs::write(&path, response.body).await?;

        debug!(path = %path.display(), size = total_size, "downloaded attachment from URL");
        Ok(CachedMedia {
            local_path: path,
            mime_type: mime_type.to_string(),
            size: total_size,
        })
    }

    /// Read a cached file and return its base64-encoded content.
    pub async fn to_base64(cached: &CachedMedia) -> Result<String, CacheError> {
        let bytes = tokio::fs::read(&cached.local_path).await?;
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
    ///
    /// Uses the `.created_at` marker file written at session creation time
    /// to determine age, falling back to directory `modified` time.
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
            // Prefer .created_at marker over directory mtime (mtime updates on every write)
            let created = entry.path().join(".created_at");
            let timestamp = if created.exists() {
                std::fs::metadata(&created)
                    .ok()
                    .and_then(|m| m.modified().ok())
            } else {
                meta.modified().ok()
            };
            let age = timestamp
                .and_then(|t| now.duration_since(t).ok())
                .unwrap_or(Duration::ZERO);
            // If timestamp is in the future (clock skew), treat as fresh
            if age > STALE_THRESHOLD && age != Duration::ZERO {
                if let Err(e) = std::fs::remove_dir_all(entry.path()) {
                    warn!(path = %entry.path().display(), error = %e, "failed to remove stale media dir");
                } else {
                    debug!(path = %entry.path().display(), "removed stale media dir");
                }
            }
        }
    }

    /// Convert a [`MediaItem`] (from tool `_media` output) to a channel [`Attachment`].
    ///
    /// Downloads to temp file on success, falls back to URL-only on failure.
    pub async fn download_media_item(&self, item: &MediaItem, session_id: &str) -> Attachment {
        let id = uuid::Uuid::new_v4().to_string();
        let mime = item
            .mime_type
            .as_deref()
            .map(|s| s.to_string())
            .unwrap_or_else(|| detect_mime(&item.url, &item.media_type));

        debug!(
            url = %item.url,
            media_type = %item.media_type,
            mime = %mime,
            session_id = %session_id,
            "download_media_item: starting download"
        );

        // Build a temporary Attachment to pass through resolve()
        let temp_attachment = if item.url.to_ascii_lowercase().starts_with("data:") {
            // Parse data URL: data:[<mediatype>][;base64],<data>
            match Self::decode_data_url(&item.url) {
                Ok((decoded_mime, bytes)) => Attachment {
                    // rust-doctor-disable-next-line excessive-clone
                    id: id.clone(),
                    // rust-doctor-disable-next-line excessive-clone
                    mime_type: decoded_mime.unwrap_or_else(|| mime.clone()),
                    filename: item
                        .filename
                        .as_deref()
                        .map(|s| s.to_string())
.or_else(|| Some(format!("{}.bin", id.get(..FALLBACK_FILENAME_PREFIX_LEN).unwrap_or(&id)))),
                    size: Some(bytes.len() as u64),
                    url: None,
                    path: None,
                    data: Some(bytes),
                },
                Err(e) => {
                    let url_prefix: String = item.url.chars().take(30).collect();
                    warn!(url_prefix = %url_prefix, error = %e, "Failed to decode data URL, falling back to URL-only");
                    return Self::url_only_attachment(&id, &item.url, &mime, &item.filename);
                }
            }
        } else if item.url.starts_with('/')
            || item.url.starts_with("./")
            || item.url.starts_with("~/")
        {
            // Local file path. A `media_send` path is model-supplied and
            // untrusted: only accept one that resolves inside the OS temp dir —
            // the sole root where legitimate producers write (native
            // camera_clip/record_audio via NSTemporaryDirectory, and this cache
            // itself under `<temp_dir>/aleph/media`). Without this a crafted path
            // like "~/.ssh/id_rsa" or "/etc/passwd" would be read and delivered
            // outbound (arbitrary-file exfiltration).
            match Self::safe_local_media_path(&item.url).await {
                Some(safe) => Attachment {
                    // rust-doctor-disable-next-line excessive-clone
                    id: id.clone(),
                    // rust-doctor-disable-next-line excessive-clone
                    mime_type: mime.clone(),
                    // rust-doctor-disable-next-line excessive-clone
                    filename: item.filename.clone(),
                    size: None,
                    url: None,
                    path: Some(safe),
                    data: None,
                },
                None => {
                    warn!(
                        path = %item.url,
                        "media_send local path escapes the allowed media root; refusing to attach file"
                    );
                    return Self::url_only_attachment(&id, &item.url, &mime, &item.filename);
                }
            }
        } else {
            // HTTP/HTTPS URL
            Attachment {
                // rust-doctor-disable-next-line excessive-clone
                id: id.clone(),
                // rust-doctor-disable-next-line excessive-clone
                mime_type: mime.clone(),
                filename: item
                    .filename
                    .as_deref()
                    .map(|s| s.to_string())
                    .or_else(|| Some(format!("{}.bin", id.get(..FALLBACK_FILENAME_PREFIX_LEN).unwrap_or(&id)))),
                size: None,
                // rust-doctor-disable-next-line excessive-clone
                url: Some(item.url.clone()),
                path: None,
                data: None,
            }
        };

        match self.resolve(&temp_attachment, session_id).await {
            Ok(cached) => Attachment {
                id,
                mime_type: cached.mime_type,
                // rust-doctor-disable-next-line excessive-clone
                filename: item.filename.clone(),
                size: Some(cached.size),
                // rust-doctor-disable-next-line excessive-clone
                url: Some(item.url.clone()),
                path: Some(cached.local_path.to_string_lossy().to_string()),
                data: None,
            },
            Err(e) => {
                warn!(url = %item.url, error = %e, "Media download failed, falling back to URL-only");
                Self::url_only_attachment(&id, &item.url, &mime, &item.filename)
            }
        }
    }

    /// Parse a data URL and decode its content.
    fn decode_data_url(url: &str) -> Result<(Option<String>, Vec<u8>), CacheError> {
        // Format: data:[<mediatype>][;base64],<data>
        let rest = url
            .strip_prefix("data:")
            .ok_or_else(|| CacheError::Download("Not a data URL".to_string()))?;
        let (header, data) = rest.split_once(',').ok_or_else(|| {
            CacheError::Download("Invalid data URL: no comma separator".to_string())
        })?;
        let mime = if header.contains(';') {
            let mime_part = header.split(';').next().unwrap_or("");
            if mime_part.is_empty() {
                None
            } else {
                Some(mime_part.to_string())
            }
        } else if !header.is_empty() && !header.contains("base64") {
            Some(header.to_string())
        } else {
            None
        };

        let bytes = if header.contains("base64") {
            BASE64
                .decode(data)
                .map_err(|e| CacheError::Download(format!("Base64 decode failed: {e}")))?
        } else {
            // Non-base64 data URLs carry percent-encoded text per RFC 2397.
            // Decode the `%XX` escapes rather than storing them as literal bytes.
            percent_encoding::percent_decode_str(data).collect()
        };

        Ok((mime, bytes))
    }

    /// Validate a model-supplied local media path for outbound delivery.
    ///
    /// Returns the canonicalized path (as a String) iff it resolves inside the
    /// OS temp dir — the only root where legitimate producers write (native
    /// `camera_clip`/`record_audio` via `NSTemporaryDirectory`, and this cache's
    /// own `<temp_dir>/aleph/media`). Canonicalization collapses `..` and
    /// symlinks first, and `PathBuf::starts_with` matches whole components, so an
    /// escape cannot slip past the prefix check. Returns `None` for any path
    /// outside that root (or one that cannot be resolved), which the caller
    /// treats as "do not attach this file".
    async fn safe_local_media_path(raw: &str) -> Option<String> {
        let expanded = expand_tilde(raw);
        let canonical = tokio::fs::canonicalize(&expanded).await.ok()?;
        let root = tokio::fs::canonicalize(std::env::temp_dir()).await.ok()?;
        canonical
            .starts_with(&root)
            .then(|| canonical.to_string_lossy().into_owned())
    }

    /// Create a URL-only fallback Attachment (no local file).
    fn url_only_attachment(
        id: &str,
        url: &str,
        mime: &str,
        filename: &Option<String>,
    ) -> Attachment {
        Attachment {
            id: id.to_string(),
            mime_type: mime.to_string(),
            // rust-doctor-disable-next-line excessive-clone
            filename: filename.clone(),
            size: None,
            url: Some(url.to_string()),
            path: None,
            data: None,
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

/// Ensure session directory exists and write a `.created_at` marker
/// (only on first creation) so `cleanup_stale` can use a stable timestamp.
async fn ensure_session_dir(session_id: &str) -> Result<PathBuf, std::io::Error> {
    let dir = session_dir(session_id);
    let marker = dir.join(".created_at");
    // create_dir_all is idempotent — avoids TOCTOU between exists() check and creation.
    tokio::fs::create_dir_all(&dir).await?;
    if !tokio::fs::try_exists(&marker).await.unwrap_or(false) {
        // Best-effort marker — if it fails, cleanup_stale falls back to mtime
        let _ = tokio::fs::write(&marker, "").await;
    }
    Ok(dir)
}

/// Build a collision-free temp filename by prefixing the (sanitized) attachment
/// id onto the (sanitized) display name.
///
/// `download_media_item` resolves media items in parallel (`join_all`) into one
/// shared per-session dir. Two items carrying the same `filename` would otherwise
/// map to the same temp path and write over each other concurrently, corrupting
/// both. The unique per-item id prefix keeps their paths distinct. Both halves are
/// sanitized (no path separators), so the joined result stays traversal-safe.
fn unique_filename(id: &str, name: Option<&str>) -> String {
    let base = sanitize_filename(name.unwrap_or(id));
    let prefix = sanitize_filename(id);
    format!("{prefix}-{base}")
}

/// Strip directory components from a filename to prevent path traversal.
///
/// `foo/bar.txt` → `bar.txt`; `../../../etc/passwd` → `passwd`; `..` → `unnamed`.
fn sanitize_filename(name: &str) -> String {
    Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unnamed")
        .to_string()
}

/// Expand a leading `~/` (or bare `~`) into the user's home directory.
fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if Path::new(rest)
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return PathBuf::from(path);
        }
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    } else if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    PathBuf::from(path)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn download_media_item_rejects_local_path_outside_temp_root() {
        // A model-supplied path outside the OS temp dir must not be attached as a
        // local file (arbitrary-file exfiltration guard): it falls back to
        // URL-only, which carries no readable `path` for the channel to upload.
        let cache = MediaCache::new();
        let item = MediaItem {
            url: "/etc/hosts".to_string(),
            media_type: "file".to_string(),
            mime_type: None,
            filename: None,
        };
        let att = cache.download_media_item(&item, "sess-guard").await;
        assert!(
            att.path.is_none(),
            "file outside the temp root must not be attached, got {:?}",
            att.path
        );
    }

    #[tokio::test]
    async fn download_media_item_allows_local_path_inside_temp_root() {
        // A file a legitimate producer wrote under the OS temp dir (where
        // camera_clip/record_audio and this cache write) must still attach.
        let dir = std::env::temp_dir().join("aleph-media-guard-test");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("clip.bin");
        tokio::fs::write(&path, b"hello").await.unwrap();
        let cache = MediaCache::new();
        let item = MediaItem {
            url: path.to_string_lossy().into_owned(),
            media_type: "file".to_string(),
            mime_type: Some("application/octet-stream".to_string()),
            filename: Some("clip.bin".to_string()),
        };
        let att = cache.download_media_item(&item, "sess-guard").await;
        assert!(
            att.path.is_some(),
            "file inside the temp root must be attached"
        );
        let _ = tokio::fs::remove_file(&path).await;
    }

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

        let content = tokio::fs::read(&cached.local_path).await.unwrap();
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
        let b64 = MediaCache::to_base64(&cached).await.unwrap();
        assert_eq!(b64, "SGVsbG8="); // base64("Hello")

        let _ = MediaCache::cleanup_session(session_id);
    }

    #[tokio::test]
    #[cfg(not(windows))] // TODO(windows): local-file-path vs URL detection (C:\ resembles a scheme); needs repro
    async fn test_download_media_item_local_path() {
        use crate::gateway::media::MediaItem;
        let cache = MediaCache::new();

        // Create a temp file to use as "local path"
        let dir = session_dir("test-media-item-local");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let local_file = dir.join("test.png");
        tokio::fs::write(&local_file, b"fake png data")
            .await
            .unwrap();

        let item = MediaItem {
            url: local_file.to_string_lossy().to_string(),
            media_type: "image".to_string(),
            mime_type: Some("image/png".to_string()),
            filename: None,
        };

        let att = cache
            .download_media_item(&item, "test-media-item-local")
            .await;
        assert_eq!(att.mime_type, "image/png");
        assert!(att.path.is_some());
        assert!(att.url.is_some());

        let _ = MediaCache::cleanup_session("test-media-item-local");
    }

    #[tokio::test]
    async fn test_download_media_item_data_url() {
        use crate::gateway::media::MediaItem;
        let cache = MediaCache::new();

        // "Hello" in base64 = SGVsbG8=
        let item = MediaItem {
            url: "data:text/plain;base64,SGVsbG8=".to_string(),
            media_type: "file".to_string(),
            mime_type: None,
            filename: Some("hello.txt".to_string()),
        };

        let att = cache
            .download_media_item(&item, "test-media-item-data")
            .await;
        assert_eq!(att.mime_type, "text/plain");
        assert!(att.path.is_some(), "data URL should be decoded to file");

        // Verify content
        let content = tokio::fs::read_to_string(att.path.as_ref().unwrap())
            .await
            .unwrap();
        assert_eq!(content, "Hello");

        let _ = MediaCache::cleanup_session("test-media-item-data");
    }

    #[test]
    fn test_decode_data_url_base64() {
        // "Hello" base64-encoded
        let (mime, bytes) = MediaCache::decode_data_url("data:text/plain;base64,SGVsbG8=").unwrap();
        assert_eq!(mime.as_deref(), Some("text/plain"));
        assert_eq!(bytes, b"Hello");
    }

    #[test]
    fn test_decode_data_url_percent_encoded() {
        // Non-base64 data URL with percent-encoded text must be decoded, not
        // stored as literal "%20" / "%21" byte sequences.
        let (mime, bytes) =
            MediaCache::decode_data_url("data:text/plain,Hello%20World%21").unwrap();
        assert_eq!(mime.as_deref(), Some("text/plain"));
        assert_eq!(bytes, b"Hello World!");
    }

    #[tokio::test]
    async fn test_download_media_item_invalid_url_fallback() {
        use crate::gateway::media::MediaItem;
        let cache = MediaCache::new();

        let item = MediaItem {
            url: "https://invalid.example.com/does-not-exist.png".to_string(),
            media_type: "image".to_string(),
            mime_type: Some("image/png".to_string()),
            filename: None,
        };

        let att = cache
            .download_media_item(&item, "test-media-item-fallback")
            .await;
        // Should fallback to URL-only
        assert!(att.url.is_some());
        assert!(att.path.is_none());
        assert_eq!(att.mime_type, "image/png");

        let _ = MediaCache::cleanup_session("test-media-item-fallback");
    }
}
