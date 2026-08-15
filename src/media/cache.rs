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
use crate::gateway::media::{detect_mime, is_data_url, is_local_media_path, MediaItem};
use crate::security::ssrf::{safe_fetch, SafeFetchRequest, SsrfPolicy};
use crate::utils::filename::sanitize_filename;

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

    /// The source was well-formed but policy refused to read it.
    ///
    /// Distinct from [`Self::Io`] on purpose: "this path is outside the media
    /// root" is a decision, not a failure to read, and the caller phrases it to
    /// the model differently — a refusal is actionable, an I/O error is not.
    #[error("refused: {0}")]
    Refused(String),
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
        write_private(&path, data).await?;
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
            if let Some(content_length) = content_length.to_str().ok().and_then(|s| s.parse().ok())
            {
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
        write_private(&path, &response.body).await?;

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
        let dir = session_dir(session_id)?;
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
        // A refused root is worth saying out loud: the sweep is fire-and-forget,
        // so silence here is indistinguishable from "nothing to clean".
        let base = match base_dir() {
            Ok(base) => base,
            Err(e) => {
                warn!(error = %e, "media cache root unusable — skipping stale sweep");
                return;
            }
        };
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

    /// Convert a [`MediaItem`] (from tool `_media` output) to a channel
    /// [`Attachment`] backed by a readable local file under `session_id`'s
    /// directory.
    ///
    /// # Why this returns the error instead of degrading
    ///
    /// It used to answer every failure with [`Self::url_only_attachment`] and
    /// leave the reason in a `warn!`. That made three different problems —
    /// an undecodable `data:` URL, a path outside the media root, a 404 —
    /// indistinguishable to every caller, and the *reason* is precisely what
    /// lets the model fix its next attempt. Degrading is still a legitimate
    /// choice for a remote URL a channel might fetch on its own, so the
    /// fallback constructor stayed — but it is now the **caller's** decision,
    /// made per branch, rather than a policy buried in the cache.
    pub async fn download_media_item(
        &self,
        item: &MediaItem,
        session_id: &str,
    ) -> Result<Attachment, CacheError> {
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
        let temp_attachment =
            if is_data_url(&item.url) {
                // Parse data URL: data:[<mediatype>][;base64],<data>
                let (decoded_mime, bytes) = Self::decode_data_url(&item.url).map_err(|e| {
                    let url_prefix: String = item.url.chars().take(30).collect();
                    warn!(url_prefix = %url_prefix, error = %e, "Failed to decode data URL");
                    e
                })?;
                Attachment {
                    // rust-doctor-disable-next-line excessive-clone
                    id: id.clone(),
                    // rust-doctor-disable-next-line excessive-clone
                    mime_type: decoded_mime.unwrap_or_else(|| mime.clone()),
                    filename: item.filename.as_deref().map(|s| s.to_string()).or_else(|| {
                        Some(format!(
                            "{}.bin",
                            id.get(..FALLBACK_FILENAME_PREFIX_LEN).unwrap_or(&id)
                        ))
                    }),
                    size: Some(bytes.len() as u64),
                    url: None,
                    path: None,
                    data: Some(bytes),
                }
            } else if is_local_media_path(&item.url) {
                // Local file path. A `media_send` path is model-supplied and
                // untrusted: only accept one that resolves inside the OS temp dir —
                // the sole root where legitimate producers write (native
                // camera_clip/record_audio via NSTemporaryDirectory, and this cache
                // itself under `<temp_dir>/aleph/media`). Without this a crafted path
                // like "~/.ssh/id_rsa" or "/etc/passwd" would be read and delivered
                // outbound (arbitrary-file exfiltration).
                let safe = Self::safe_local_media_path(&item.url).await.ok_or_else(|| {
                warn!(
                    path = %item.url,
                    "media local path escapes the allowed media root; refusing to attach file"
                );
                CacheError::Refused(format!(
                    "local path does not resolve to an existing file inside the allowed media \
                     root ({})",
                    std::env::temp_dir().display()
                ))
            })?;
                Attachment {
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
                }
            } else {
                // HTTP/HTTPS URL
                Attachment {
                    // rust-doctor-disable-next-line excessive-clone
                    id: id.clone(),
                    // rust-doctor-disable-next-line excessive-clone
                    mime_type: mime.clone(),
                    filename: item.filename.as_deref().map(|s| s.to_string()).or_else(|| {
                        Some(format!(
                            "{}.bin",
                            id.get(..FALLBACK_FILENAME_PREFIX_LEN).unwrap_or(&id)
                        ))
                    }),
                    size: None,
                    // rust-doctor-disable-next-line excessive-clone
                    url: Some(item.url.clone()),
                    path: None,
                    data: None,
                }
            };

        let cached = self
            .resolve(&temp_attachment, session_id)
            .await
            .inspect_err(|e| warn!(url = %item.url, error = %e, "Media download failed"))?;
        Ok(Attachment {
            id,
            mime_type: cached.mime_type,
            // rust-doctor-disable-next-line excessive-clone
            filename: item.filename.clone(),
            size: Some(cached.size),
            // rust-doctor-disable-next-line excessive-clone
            url: Some(item.url.clone()),
            path: Some(cached.local_path.to_string_lossy().to_string()),
            data: None,
        })
    }

    /// Parse a data URL and decode its content.
    ///
    /// `pub(crate)` so `media_send`'s pre-flight can ask the *same* decoder
    /// whether an item will produce bytes. A second parser written to mirror
    /// this one would be free to drift, and the drift would be silent: the
    /// delivery path answers a decode failure with a url-only attachment, not
    /// an error.
    pub(crate) fn decode_data_url(url: &str) -> Result<(Option<String>, Vec<u8>), CacheError> {
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
    ///
    /// `pub(crate)` so `media_send`'s pre-flight can ask this exact predicate
    /// rather than a copy of it — see [`Self::decode_data_url`] for why a copy
    /// would be the dangerous option.
    pub(crate) async fn safe_local_media_path(raw: &str) -> Option<String> {
        let expanded = expand_tilde(raw);
        let canonical = tokio::fs::canonicalize(&expanded).await.ok()?;
        let root = tokio::fs::canonicalize(std::env::temp_dir()).await.ok()?;
        canonical
            .starts_with(&root)
            .then(|| canonical.to_string_lossy().into_owned())
    }

    /// An [`Attachment`] that carries only the original URL — no bytes, no
    /// local file.
    ///
    /// Worth something for exactly one kind of failure: a remote `http(s)` URL
    /// we could not fetch but a channel might (Telegram uploads from a URL
    /// itself, and a file over our own 50 MB cap is still a working link).
    /// Worth nothing for the other two — a `data:` URL that did not decode has
    /// no URL to hand on, and a refused local path yields an attachment whose
    /// `url` is a **filesystem path** no channel can send. So the choice
    /// belongs to the caller, which knows which branch it is in; see
    /// [`Self::download_media_item`].
    #[must_use]
    pub(crate) fn url_only_attachment(item: &MediaItem) -> Attachment {
        Attachment {
            id: uuid::Uuid::new_v4().to_string(),
            mime_type: item
                .mime_type
                .clone()
                .unwrap_or_else(|| detect_mime(&item.url, &item.media_type)),
            // rust-doctor-disable-next-line excessive-clone
            filename: item.filename.clone(),
            size: None,
            // rust-doctor-disable-next-line excessive-clone
            url: Some(item.url.clone()),
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

/// Base directory: `<private_temp_root>/media`
///
/// Inbound attachment bytes are exactly the content a shared host must not
/// leak, so the tree hangs off [`crate::utils::paths::private_temp_root`] (an
/// owner-only `<temp_dir>/aleph-<uid>`) rather than a fixed, world-listable
/// `<temp_dir>/aleph`. It stays *under* `temp_dir()`, which is what keeps
/// [`MediaCache::safe_local_media_path`] accepting these files on the way back
/// out.
///
/// A tree an older build left under `<temp_dir>/aleph/media` is not migrated
/// and no longer swept — nothing writes there any more, and it is exactly the
/// path another account may already have claimed. It ages out with the host's
/// own temp cleaning.
///
/// # Errors
///
/// Propagates the root's refusal when another account owns or can reach it —
/// the one case where writing an attachment here would hand it over.
fn base_dir() -> Result<PathBuf, std::io::Error> {
    Ok(crate::utils::paths::private_temp_root()?.join("media"))
}

/// Per-session directory: `<private_temp_root>/media/<encoded_session_id>`
///
/// `session_id` is a raw session key such as `agent:main:main`. Joining it
/// verbatim made `create_dir_all` fail on Windows, where `:` is illegal in a
/// path component. It goes through the same encoder the artifact store uses so
/// both agree on how a session key becomes one directory name.
///
/// Directories created under the old raw naming are still swept by
/// [`MediaCache::cleanup_stale`], which walks the *current* base dir by age and
/// never interprets the names — see [`base_dir`] for the pre-move tree, which
/// has no sweeper at all.
fn session_dir(session_id: &str) -> Result<PathBuf, std::io::Error> {
    Ok(base_dir()?.join(crate::artifacts::encode_session_key(session_id)))
}

/// Ensure session directory exists and write a `.created_at` marker
/// (only on first creation) so `cleanup_stale` can use a stable timestamp.
async fn ensure_session_dir(session_id: &str) -> Result<PathBuf, std::io::Error> {
    let dir = session_dir(session_id)?;
    let marker = dir.join(".created_at");
    // create_dir_all is idempotent — avoids TOCTOU between exists() check and creation.
    tokio::fs::create_dir_all(&dir).await?;
    if !tokio::fs::try_exists(&marker).await.unwrap_or(false) {
        // Best-effort marker — if it fails, cleanup_stale falls back to mtime.
        // Owner-only like every other file here, so "what mode is this?" has one
        // answer for the whole tree.
        let _ = write_private(&marker, b"").await;
    }
    Ok(dir)
}

/// Write attachment bytes to a fresh owner-only file.
///
/// The 0700 root already keeps other accounts out of the tree; this is the
/// per-file half of the same discipline — the mode every file the repo treats
/// as sensitive carries (`secrets::vault`, `config::save`, the node identity)
/// — so a later relaxation of the root's mode does not silently expose
/// everything under it. Created *at* that mode rather than chmod-ed after the
/// write, which is the 0644 window the others still have.
///
/// Deliberately create-and-truncate rather than an exclusive create: resolving
/// the same attachment id twice must keep overwriting, as `tokio::fs::write`
/// did. Nothing is won by exclusivity inside a directory no other account can
/// open.
async fn write_private(path: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    use tokio::io::AsyncWriteExt as _;

    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        // Inherent on tokio's OpenOptions — no `OpenOptionsExt` import needed.
        options.mode(0o600);
    }
    let mut file = options.open(path).await?;
    file.write_all(bytes).await?;
    // tokio's File buffers; a dropped handle can lose the tail.
    file.flush().await
}

/// Build a collision-free temp filename by prefixing the (sanitized) attachment
/// id onto the (sanitized) display name.
///
/// `download_media_item` resolves media items in parallel (`join_all`) into one
/// shared per-session dir. Two items carrying the same `filename` would otherwise
/// map to the same temp path and write over each other concurrently, corrupting
/// both. The unique per-item id prefix keeps their paths distinct.
///
/// Both halves go through the shared [`sanitize_filename`], so each is a single
/// path component — no separator, no control byte, no character illegal on
/// Windows, bounded length — and the joined result stays traversal-safe.
fn unique_filename(id: &str, name: Option<&str>) -> String {
    let base = sanitize_filename(name.unwrap_or(id));
    let prefix = sanitize_filename(id);
    format!("{prefix}-{base}")
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
    use crate::utils::filename::{FALLBACK_FILENAME, MAX_FILENAME_CHARS};

    /// The pre-move tree at `<temp_dir>/aleph/media` is deliberately NOT
    /// migrated and deliberately NOT swept — see [`base_dir`], which gives the
    /// reason: that fixed, world-listable name "is exactly the path another
    /// account may already have claimed". A sweeper pointed at it would be
    /// this process deleting under a name it does not own.
    ///
    /// That is a ruling, and until now it lived only in prose. Prose does not
    /// stop the next reader from helpfully adding the migration back — the
    /// orphaned tree looks like an oversight, which is precisely why the
    /// decision needs a test that goes red rather than a paragraph that does
    /// not.
    ///
    /// Both halves, because the ruled-out side alone would stay green if
    /// `base_dir` stopped resolving anywhere sensible at all: first that the
    /// tree really does hang off the owner-only root, then that it is not the
    /// shared name.
    #[test]
    fn the_media_tree_hangs_off_the_private_root_and_never_the_shared_name() {
        let base = base_dir().expect("the private scratch root must resolve");
        let private =
            crate::utils::paths::private_temp_root().expect("private root resolves twice alike");

        assert_eq!(
            base,
            private.join("media"),
            "the media tree must be the private root's own child"
        );

        // The ruled-out location. Compared component-wise (`Path::starts_with`),
        // never as a string: `aleph-501` string-starts-with `aleph`, and a
        // string compare here would report a violation on every Unix machine.
        let shared = std::env::temp_dir().join("aleph");
        assert!(
            !base.starts_with(&shared),
            "{} sits under the shared name {} that base_dir's own doc rules out",
            base.display(),
            shared.display()
        );
    }

    /// Before the two `sanitize_filename` copies converged, this half stripped
    /// directory components and *nothing else*: a control byte, a character
    /// illegal on Windows, and an unbounded length all reached the temp path
    /// verbatim. Everything asserted below except the separator check would
    /// have gone RED against that copy.
    #[test]
    fn unique_filename_hardens_the_display_name_not_only_its_directory() {
        let id = "11111111-2222-3333-4444-555555555555";
        let raw = format!("../re\u{7}port:{}.txt", "x".repeat(400));
        let out = unique_filename(id, Some(&raw));

        assert!(
            out.starts_with(&format!("{id}-")),
            "the per-item collision prefix must survive: {out:?}"
        );
        assert!(!out.contains('\u{7}'), "control byte survived: {out:?}");
        assert!(
            !out.contains(':'),
            "a character illegal on Windows survived: {out:?}"
        );
        assert!(
            !out.contains('/') && !out.contains('\\'),
            "a path separator survived: {out:?}"
        );
        assert_eq!(
            Path::new(&out).components().count(),
            1,
            "the temp filename must stay one path component: {out:?}"
        );
        assert!(
            out.chars().count() <= 2 * MAX_FILENAME_CHARS + 1,
            "each half is length-bounded: {} chars",
            out.chars().count()
        );
    }

    /// The fallback string is load-bearing here: it becomes a real path
    /// component under the per-session media dir, so it must stay non-empty
    /// and must never be the parent link.
    #[test]
    fn unique_filename_falls_back_when_the_display_name_sanitizes_away() {
        let id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        assert_eq!(
            unique_filename(id, Some("..")),
            format!("{id}-{FALLBACK_FILENAME}")
        );
        // No display name at all: the id stands in for both halves.
        assert_eq!(unique_filename(id, None), format!("{id}-{id}"));
    }

    #[test]
    fn session_dir_encodes_characters_illegal_on_windows() {
        // Session keys look like `agent:main:main`. A raw join put a `:` into a
        // path component, which `create_dir_all` rejects on Windows.
        let dir = session_dir("agent:main:main").expect("session dir resolves");
        let component = dir
            .file_name()
            .and_then(|n| n.to_str())
            .expect("session dir has a final component");
        assert!(
            !component.contains(':'),
            "no colon may survive into the path, got {component}"
        );
        assert_eq!(component, "agent%3Amain%3Amain");
    }

    #[tokio::test]
    async fn download_media_item_rejects_local_path_outside_temp_root() {
        // A model-supplied path outside the OS temp dir must not be attached as
        // a local file (arbitrary-file exfiltration guard), and the refusal
        // must be legible to the caller rather than buried in a warn! — the
        // caller is what decides whether anything is queued for the user.
        let cache = MediaCache::new();
        let item = MediaItem {
            url: "/etc/hosts".to_string(),
            media_type: "file".to_string(),
            mime_type: None,
            filename: None,
        };
        let err = cache
            .download_media_item(&item, "sess-guard")
            .await
            .expect_err("a file outside the temp root must not be attached");
        assert!(
            matches!(err, CacheError::Refused(_)),
            "a policy refusal must not masquerade as an I/O failure, got {err:?}"
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
        let att = cache
            .download_media_item(&item, "sess-guard")
            .await
            .expect("file inside the temp root must be attached");
        assert!(att.path.is_some());
        let _ = tokio::fs::remove_file(&path).await;
    }

    /// Inbound attachment bytes land owner-only, and the tree they land in is
    /// the private root — not the world-listable `<temp_dir>/aleph` this cache
    /// used to write to on a shared Linux host.
    #[cfg(unix)]
    #[tokio::test]
    async fn cached_attachment_bytes_are_owner_only_inside_the_private_root() {
        use std::os::unix::fs::PermissionsExt;

        let cache = MediaCache::new();
        let mut att = empty_attachment();
        att.data = Some(vec![7, 7, 7]);

        let session_id = "test-inline-permissions";
        let cached = cache.resolve(&att, session_id).await.unwrap();

        let root = crate::utils::paths::private_temp_root().expect("private root resolves");
        assert!(
            cached.local_path.starts_with(&root),
            "{} escaped the private root {}",
            cached.local_path.display(),
            root.display()
        );
        let meta = tokio::fs::metadata(&cached.local_path).await.unwrap();
        let mode = meta.permissions().mode() & 0o7777;
        assert_eq!(mode, 0o600, "attachment bytes must be owner-only");

        let _ = MediaCache::cleanup_session(session_id);
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
        let dir = session_dir("test-media-item-local").expect("session dir resolves");
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
            .await
            .expect("a local path under the temp root resolves");
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
            .await
            .expect("a well-formed data: URL decodes");
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
    async fn test_download_media_item_unreachable_url_reports_the_reason() {
        use crate::gateway::media::MediaItem;
        let cache = MediaCache::new();

        let item = MediaItem {
            url: "https://invalid.example.com/does-not-exist.png".to_string(),
            media_type: "image".to_string(),
            mime_type: Some("image/png".to_string()),
            filename: None,
        };

        let err = cache
            .download_media_item(&item, "test-media-item-fallback")
            .await
            .expect_err("an unreachable URL must surface as an error");
        // The caller — not the cache — decides whether a remote URL it could
        // not fetch is still worth handing to the channel.
        let fallback = MediaCache::url_only_attachment(&item);
        assert_eq!(fallback.url.as_deref(), Some(item.url.as_str()));
        assert!(fallback.path.is_none());
        assert_eq!(fallback.mime_type, "image/png");
        assert!(!err.to_string().is_empty(), "the reason must be legible");

        let _ = MediaCache::cleanup_session("test-media-item-fallback");
    }
}
