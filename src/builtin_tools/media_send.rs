//! `media_send` — LLM-invoked tool for delivering media to the user.
//!
//! This tool does no media processing. It pre-flights each item so a refusal is
//! visible to the model, then passes the input items through as `_media`
//! output, which the `ReplyEmitter` picks up for download and channel delivery.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{AlephError, Result};
use crate::gateway::media::{is_data_url, is_local_media_path, is_remote_fetch_url, MediaItem};
use crate::media::cache::MediaCache;
use crate::security::ssrf::{validate_url_async, SsrfPolicy};
use crate::tools::AlephTool;

/// Input arguments for `media_send` tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MediaSendArgs {
    /// List of media items to send to the user.
    /// Each item needs a `url` and `media_type` ("image", "video", "audio", "file").
    pub items: Vec<MediaSendItem>,
}

/// A single media item to send.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MediaSendItem {
    /// Remote URL (http/https), local file path, or `data:` URL of the media.
    pub url: String,
    /// Type: "image", "video", "audio", or "file"
    pub media_type: String,
    /// Optional MIME type (auto-detected if absent)
    #[serde(default)]
    pub mime_type: Option<String>,
    /// Optional filename
    #[serde(default)]
    pub filename: Option<String>,
}

/// Output from `media_send` tool.
#[derive(Debug, Clone, Serialize)]
pub struct MediaSendOutput {
    /// Human-readable display text
    pub _display: String,
    /// Media items for channel delivery (consumed by `ReplyEmitter`)
    pub _media: Vec<MediaItem>,
}

/// The `media_send` tool — passes media URLs through for channel delivery.
#[derive(Clone)]
pub struct MediaSendTool;

impl MediaSendTool {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for MediaSendTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AlephTool for MediaSendTool {
    const NAME: &'static str = "media_send";
    const DESCRIPTION: &'static str =
        "Send media files (images, videos, audio) directly to the user in the chat. \
         Use this when you find or have media URLs that the user wants to see or hear.";
    type Args = MediaSendArgs;
    type Output = MediaSendOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let ssrf_policy = SsrfPolicy::default();
        for item in &args.items {
            preflight(&item.url, &ssrf_policy).await?;
        }

        let count = args.items.len();
        let media: Vec<MediaItem> = args
            .items
            .into_iter()
            .map(|item| MediaItem {
                url: item.url,
                media_type: item.media_type,
                mime_type: item.mime_type,
                filename: item.filename,
            })
            .collect();

        Ok(MediaSendOutput {
            _display: format!(
                "Sending {} media file{}...",
                count,
                if count == 1 { "" } else { "s" }
            ),
            _media: media,
        })
    }
}

/// Refuse an item the delivery path could not turn into deliverable bytes.
///
/// Not the enforcement point — `MediaCache::download_media_item` re-runs all of
/// this, and the remote fetch is authoritatively guarded by `safe_fetch`. What
/// this buys is a *visible* failure. Every one of the three refusals below is
/// answered downstream by `url_only_attachment`: an `Attachment` with no bytes
/// and no local path, whose `url` for a rejected local path is a **filesystem
/// path** that no channel can send. Meanwhile the model is handed
/// `"Sending 1 media file..."` and the refusal exists only as a server-side
/// `warn!`. The user gets nothing, the model believes it succeeded, and the
/// delivery attempt happens at `RunComplete` — after the loop has ended, so
/// there is no later turn in which the model could find out. Pre-flighting is
/// the only point on this path where the model can still self-correct.
///
/// The dispatch mirrors `download_media_item`'s, in the same order, and calls
/// the *same* validators rather than re-deriving them — a mirrored copy would
/// be free to drift, and drift here is silent by construction.
///
/// One degradation is deliberately left un-pre-flighted: a URL that clears SSRF
/// and then 404s, times out, or exceeds the 50 MB cap. Knowing that costs the
/// fetch itself, and the fetch is what we would be pre-flighting.
async fn preflight(url: &str, ssrf_policy: &SsrfPolicy) -> Result<()> {
    if is_data_url(url) {
        MediaCache::decode_data_url(url).map_err(|e| {
            AlephError::tool(format!(
                "media_send: the data: URL does not decode ({e}), so nothing would reach the \
                 user. Re-encode the payload, or pass an http(s) URL instead."
            ))
        })?;
    } else if is_local_media_path(url) {
        if MediaCache::safe_local_media_path(url).await.is_none() {
            return Err(AlephError::tool(format!(
                "media_send: local path '{url}' does not resolve to an existing file inside the \
                 allowed media root (the OS temp directory), so it cannot be attached. Only \
                 files written by the capture tools can be sent by path; send anything else as \
                 a data: URL or an http(s) URL."
            )));
        }
    } else if is_remote_fetch_url(url) {
        // Everything that is neither inline nor local is fetched over the
        // network — the only branch that is an SSRF vector. Running the host
        // check on the other two rejected every one of them (`Url::parse`
        // fails on a bare path; `data:` has no host), which would have made a
        // captured clip impossible to send.
        if let Err(e) = validate_url_async(url, ssrf_policy).await {
            return Err(AlephError::tool(format!(
                "SSRF blocked for URL '{url}': {e}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_media_send_passthrough() {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "example.com".to_string(),
            vec!["1.1.1.1".parse::<std::net::IpAddr>().unwrap()],
        );
        let _scope = crate::security::ssrf::dns::test_hook::ResolverScope::install(map);

        let tool = MediaSendTool::new();
        let args = MediaSendArgs {
            items: vec![
                MediaSendItem {
                    url: "https://example.com/cat.jpg".to_string(),
                    media_type: "image".to_string(),
                    mime_type: Some("image/jpeg".to_string()),
                    filename: Some("cat.jpg".to_string()),
                },
                MediaSendItem {
                    url: "https://example.com/song.mp3".to_string(),
                    media_type: "audio".to_string(),
                    mime_type: None,
                    filename: None,
                },
            ],
        };

        let output = tool.call(args).await.unwrap();
        assert_eq!(output._media.len(), 2);
        assert_eq!(output._media[0].url, "https://example.com/cat.jpg");
        assert_eq!(output._media[0].media_type, "image");
        assert_eq!(output._media[1].url, "https://example.com/song.mp3");
        assert!(output._display.contains("2"));
    }

    #[tokio::test]
    async fn test_media_send_single() {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "example.com".to_string(),
            vec!["1.1.1.1".parse::<std::net::IpAddr>().unwrap()],
        );
        let _scope = crate::security::ssrf::dns::test_hook::ResolverScope::install(map);

        let tool = MediaSendTool::new();
        let args = MediaSendArgs {
            items: vec![MediaSendItem {
                url: "https://example.com/photo.png".to_string(),
                media_type: "image".to_string(),
                mime_type: None,
                filename: None,
            }],
        };

        let output = tool.call(args).await.unwrap();
        assert_eq!(output._media.len(), 1);
        assert!(output._display.contains("1"));
        assert!(!output._display.contains("files"));
    }

    #[tokio::test]
    async fn test_media_send_accepts_local_path_and_data_url() {
        // A camera_clip file path and a data: URL must pass — the SSRF host
        // check applies only to remotely-fetched http(s) URLs. The path has to
        // be a real file under the temp root, because that is exactly what the
        // delivery path will demand of it.
        let dir = std::env::temp_dir().join("aleph-media-send-accept");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let clip = dir.join("clip.mp4");
        tokio::fs::write(&clip, b"fake mp4").await.unwrap();

        let tool = MediaSendTool::new();
        let args = MediaSendArgs {
            items: vec![
                MediaSendItem {
                    url: clip.to_string_lossy().into_owned(),
                    media_type: "video".to_string(),
                    mime_type: None,
                    filename: None,
                },
                MediaSendItem {
                    url: "data:image/png;base64,iVBORw0KGgo=".to_string(),
                    media_type: "image".to_string(),
                    mime_type: None,
                    filename: None,
                },
            ],
        };
        let output = tool.call(args).await.unwrap();
        assert_eq!(output._media.len(), 2);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_media_send_rejects_local_path_outside_media_root() {
        // The delivery path refuses to read a model-supplied path outside the
        // temp root and degrades to a url-only attachment whose `url` is a
        // filesystem path — which no channel can send. Without this pre-flight
        // the model is told "Sending 1 media file..." and the user gets
        // nothing, with the refusal visible only in a server-side warn!.
        let tool = MediaSendTool::new();
        let args = MediaSendArgs {
            items: vec![MediaSendItem {
                url: "/etc/passwd".to_string(),
                media_type: "file".to_string(),
                mime_type: None,
                filename: None,
            }],
        };
        let err = tool
            .call(args)
            .await
            .expect_err("a path outside the media root must be refused");
        assert!(
            err.to_string().contains("/etc/passwd"),
            "the refusal must name the offending path; got: {err}"
        );
    }

    #[tokio::test]
    async fn test_media_send_rejects_undecodable_data_url() {
        // Same silent degradation, other branch: a data: URL whose payload does
        // not decode never becomes bytes, so the user receives nothing.
        let tool = MediaSendTool::new();
        let args = MediaSendArgs {
            items: vec![MediaSendItem {
                url: "data:image/png;base64,!!!not base64!!!".to_string(),
                media_type: "image".to_string(),
                mime_type: None,
                filename: None,
            }],
        };
        let err = tool
            .call(args)
            .await
            .expect_err("an undecodable data: URL must be refused");
        assert!(
            err.to_string().to_lowercase().contains("data:"),
            "the refusal must say which branch rejected it; got: {err}"
        );
    }

    #[tokio::test]
    async fn test_media_send_still_blocks_ssrf_on_remote_url() {
        // Loopback/internal hosts over http must still be rejected.
        let tool = MediaSendTool::new();
        let args = MediaSendArgs {
            items: vec![MediaSendItem {
                url: "http://169.254.169.254/latest/meta-data/".to_string(),
                media_type: "file".to_string(),
                mime_type: None,
                filename: None,
            }],
        };
        let err = tool
            .call(args)
            .await
            .expect_err("SSRF must block metadata IP");
        assert!(err.to_string().contains("SSRF blocked"), "got: {err}");
    }
}
