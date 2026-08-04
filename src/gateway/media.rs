//! Media attachment types for the `_media` tool output convention.

use crate::sync_primitives::Arc;

use serde::{Deserialize, Serialize};

/// Media item declared by tool output via `_media` convention.
/// Any tool can include `_media: Vec<MediaItem>` in its output JSON
/// to request media delivery to the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaItem {
    /// Remote URL, local file path, or data URL of the media
    pub url: String,
    /// Media type: "image", "video", "audio", "file"
    pub media_type: String,
    /// MIME type (auto-detected from URL/Content-Type if absent)
    #[serde(default)]
    pub mime_type: Option<String>,
    /// Optional filename
    #[serde(default)]
    pub filename: Option<String>,
}

/// True when `url` is fetched over the network by the media pipeline — i.e. it
/// is neither an inline `data:` URL nor a local file path.
///
/// Single source of truth for "is this a remote fetch": mirrors the branching
/// in [`crate::media::cache::MediaCache::download_media_item`] (`data:` → decode
/// inline, `/`·`./`·`~/` → local path, else → HTTP fetch). Only the remote
/// branch is an SSRF vector, so `media_send` runs `validate_url_async` on
/// exactly these — a bare local path (what `camera_clip`/`record_audio` return)
/// or a `data:` URL must not be rejected by the SSRF host check.
#[must_use]
pub fn is_remote_fetch_url(url: &str) -> bool {
    !(url.to_ascii_lowercase().starts_with("data:")
        || url.starts_with('/')
        || url.starts_with("./")
        || url.starts_with("~/"))
}

/// Shared media buffer between `StreamCallback` and `ReplyEmitter`.
pub type PendingMedia = Arc<tokio::sync::Mutex<Vec<MediaItem>>>;

/// Maximum number of media items allowed per run.
pub const MAX_MEDIA_PER_RUN: usize = 10;

tokio::task_local! {
    /// The channel-delivery buffer of the run currently executing — the same
    /// `Arc` this run's `ReplyEmitter` drains in
    /// [`drain_and_send_media`](crate::gateway::reply_emitter::ReplyEmitter).
    ///
    /// Scoped once at the run-loop boundary (`run_agent_loop`) next to
    /// `FsScope` / agent-id / `TURN_ORIGINATOR`, because it is one value for
    /// the whole run tree rather than something per tool call. The tool
    /// chokepoint (`ScopedToolService::apply_layer_two`) is many frames below
    /// that boundary but on the same task, so it can read this without the
    /// buffer having to be threaded through the `ToolService` construction.
    ///
    /// `None` for runs whose surface has no channel to deliver to (Panel, cron,
    /// heartbeat, A2A) and for a subagent whose runtime re-spawns the future —
    /// media then still settles into the artifact store, it just has no channel
    /// leg. Publishing `None` explicitly keeps a nested run from pushing media
    /// into an outer run's message.
    pub static RUN_PENDING_MEDIA: Option<PendingMedia>;
}

/// Run `fut` with `buffer` visible to [`current_pending_media`].
pub async fn with_pending_media<F, T>(buffer: Option<PendingMedia>, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    RUN_PENDING_MEDIA.scope(buffer, fut).await
}

/// The current run's channel-delivery buffer, or `None` outside a scope / for
/// a run with no channel leg.
#[must_use]
pub fn current_pending_media() -> Option<PendingMedia> {
    RUN_PENDING_MEDIA.try_with(Clone::clone).ok().flatten()
}

/// Detect MIME type from URL extension, with a fallback default based on `media_type`.
#[must_use]
pub fn detect_mime(url: &str, media_type: &str) -> String {
    // Strip query string and fragment before checking extension
    let path = url.split('?').next().unwrap_or(url);
    let path = path.split('#').next().unwrap_or(path);
    // Try URL extension first
    if let Some(ext) = path.rsplit('.').next() {
        match ext.to_lowercase().as_str() {
            "png" => return "image/png".to_string(),
            "jpg" | "jpeg" => return "image/jpeg".to_string(),
            "gif" => return "image/gif".to_string(),
            "webp" => return "image/webp".to_string(),
            "mp4" => return "video/mp4".to_string(),
            "webm" => return "video/webm".to_string(),
            "mp3" => return "audio/mpeg".to_string(),
            "wav" => return "audio/wav".to_string(),
            "ogg" => return "audio/ogg".to_string(),
            "flac" => return "audio/flac".to_string(),
            _ => {}
        }
    }
    // Fallback by media_type
    match media_type {
        "image" => "image/png".to_string(),
        "video" => "video/mp4".to_string(),
        "audio" => "audio/mpeg".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_mime_from_extension() {
        assert_eq!(
            detect_mime("https://example.com/photo.png", "image"),
            "image/png"
        );
        assert_eq!(
            detect_mime("https://example.com/video.mp4", "video"),
            "video/mp4"
        );
        assert_eq!(
            detect_mime("https://example.com/song.mp3", "audio"),
            "audio/mpeg"
        );
    }

    #[test]
    fn test_detect_mime_with_query_params() {
        assert_eq!(
            detect_mime("https://example.com/img.png?token=abc", "image"),
            "image/png"
        );
        assert_eq!(
            detect_mime("https://example.com/video.mp4?v=1.0", "video"),
            "video/mp4"
        );
        assert_eq!(
            detect_mime("https://cdn.example.com/file.jpg#frag", "image"),
            "image/jpeg"
        );
    }

    #[test]
    fn test_detect_mime_fallback() {
        assert_eq!(
            detect_mime("https://example.com/file?token=abc", "image"),
            "image/png"
        );
        assert_eq!(
            detect_mime("https://example.com/file?token=abc", "video"),
            "video/mp4"
        );
        assert_eq!(
            detect_mime("https://example.com/file?token=abc", "audio"),
            "audio/mpeg"
        );
        assert_eq!(
            detect_mime("https://example.com/file", "file"),
            "application/octet-stream"
        );
    }

    #[test]
    fn test_media_item_deserialization() {
        let json = r#"{"url":"https://example.com/img.png","media_type":"image"}"#;
        let item: MediaItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.url, "https://example.com/img.png");
        assert_eq!(item.media_type, "image");
        assert!(item.mime_type.is_none());
        assert!(item.filename.is_none());
    }

    #[tokio::test]
    async fn the_run_buffer_is_visible_below_the_scope_and_nowhere_else() {
        assert!(
            current_pending_media().is_none(),
            "outside a run there is no channel to deliver to"
        );

        let buffer: PendingMedia = PendingMedia::default();
        with_pending_media(Some(buffer.clone()), async {
            // Many frames below the run loop — this is what the tool
            // chokepoint's harvest sees.
            let seen = current_pending_media().expect("scoped");
            seen.lock().await.push(MediaItem {
                url: "data:image/png;base64,SGVsbG8=".into(),
                media_type: "image".into(),
                mime_type: None,
                filename: None,
            });
        })
        .await;

        assert_eq!(
            buffer.lock().await.len(),
            1,
            "the scoped handle must be the same Arc the emitter drains"
        );

        // A nested run that publishes `None` must not push into the outer run's
        // message.
        with_pending_media(Some(buffer.clone()), async {
            with_pending_media(None, async {
                assert!(current_pending_media().is_none());
            })
            .await;
        })
        .await;
    }

    #[test]
    fn test_media_item_with_all_fields() {
        let json = r#"{"url":"https://example.com/img.png","media_type":"image","mime_type":"image/png","filename":"photo.png"}"#;
        let item: MediaItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.mime_type.unwrap(), "image/png");
        assert_eq!(item.filename.unwrap(), "photo.png");
    }
}
