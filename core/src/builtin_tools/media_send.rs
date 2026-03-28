//! media_send — LLM-invoked tool for delivering media to the user.
//!
//! This tool does NO processing. It simply passes the input items as `_media`
//! output, which the ReplyEmitter picks up for download and channel delivery.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::gateway::media::MediaItem;
use crate::security::ssrf::{validate_url, SsrfPolicy};
use crate::tools::AlephTool;

/// Input arguments for media_send tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MediaSendArgs {
    /// List of media items to send to the user.
    /// Each item needs a `url` and `media_type` ("image", "video", "audio", "file").
    pub items: Vec<MediaSendItem>,
}

/// A single media item to send.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MediaSendItem {
    /// URL of the media file
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

/// Output from media_send tool.
#[derive(Debug, Clone, Serialize)]
pub struct MediaSendOutput {
    /// Human-readable display text
    pub _display: String,
    /// Media items for channel delivery (consumed by ReplyEmitter)
    pub _media: Vec<MediaItem>,
}

/// The media_send tool — passes media URLs through for channel delivery.
#[derive(Clone)]
pub struct MediaSendTool;

impl MediaSendTool {
    pub fn new() -> Self {
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
        // SSRF protection: validate all URLs before processing
        let ssrf_policy = SsrfPolicy::default();
        for item in &args.items {
            if let Err(e) = validate_url(&item.url, &ssrf_policy) {
                return Err(crate::error::AlephError::tool(format!(
                    "SSRF blocked for URL '{}': {}",
                    item.url, e
                )));
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
            _display: format!("Sending {} media file{}...", count, if count == 1 { "" } else { "s" }),
            _media: media,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_media_send_passthrough() {
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
}
