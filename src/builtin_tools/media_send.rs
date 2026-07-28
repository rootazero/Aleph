//! `media_send` — LLM-invoked tool for delivering media to the user.
//!
//! This tool does NO processing. It simply passes the input items as `_media`
//! output, which the `ReplyEmitter` picks up for download and channel delivery.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::gateway::media::{is_remote_fetch_url, MediaItem};
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
        // SSRF protection applies only to URLs the pipeline actually fetches
        // over the network. Local file paths (what `camera_clip`/`record_audio`
        // return) and `data:` URLs are delivered by reading/decoding locally —
        // running the SSRF host check on them rejected every one (`Url::parse`
        // fails on a bare path; `file:`/`data:` have no host), so a captured
        // clip could never be sent. Guard exactly the remote-fetch branch.
        let ssrf_policy = SsrfPolicy::default();
        for item in &args.items {
            if is_remote_fetch_url(&item.url) {
                if let Err(e) = validate_url_async(&item.url, &ssrf_policy).await {
                    return Err(crate::error::AlephError::tool(format!(
                        "SSRF blocked for URL '{}': {}",
                        item.url, e
                    )));
                }
            }
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
        // check applies only to remotely-fetched http(s) URLs.
        let tool = MediaSendTool::new();
        let args = MediaSendArgs {
            items: vec![
                MediaSendItem {
                    url: "/var/folders/tmp/clip.mp4".to_string(),
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
