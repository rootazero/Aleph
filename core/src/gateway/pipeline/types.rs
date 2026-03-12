//! Core data types for the message pipeline.
//!
//! These types flow through the pipeline stages:
//! InboundContext → MergedMessage → EnrichedMessage → (ready for execution)

use std::path::PathBuf;

use crate::gateway::channel::{Attachment, MessageId};
use crate::gateway::inbound_context::InboundContext;

// ---------------------------------------------------------------------------
// MediaCategory
// ---------------------------------------------------------------------------

/// Classifies an attachment by its MIME type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaCategory {
    Image,
    Document,
    Link,
    Audio,
    Video,
    Unknown,
}

impl MediaCategory {
    /// Derive category from a MIME type string.
    pub fn from_mime(mime: &str) -> Self {
        let mime_lower = mime.to_ascii_lowercase();
        if mime_lower.starts_with("image/") {
            Self::Image
        } else if mime_lower.starts_with("audio/") {
            Self::Audio
        } else if mime_lower.starts_with("video/") {
            Self::Video
        } else if mime_lower.starts_with("text/html") {
            // HTML pages are treated as links
            Self::Link
        } else if mime_lower.starts_with("application/pdf")
            || mime_lower.starts_with("text/")
            || mime_lower.starts_with("application/msword")
            || mime_lower.starts_with("application/vnd.openxmlformats")
        {
            Self::Document
        } else {
            Self::Unknown
        }
    }
}

// ---------------------------------------------------------------------------
// LocalMedia
// ---------------------------------------------------------------------------

/// An attachment that has been downloaded to a local path.
#[derive(Debug, Clone)]
pub struct LocalMedia {
    /// The original attachment metadata.
    pub original: Attachment,
    /// Local filesystem path where the file was saved.
    pub local_path: PathBuf,
    /// Derived media category.
    pub media_category: MediaCategory,
}

// ---------------------------------------------------------------------------
// UnderstandingType / MediaUnderstanding
// ---------------------------------------------------------------------------

/// What kind of understanding was produced for a media item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnderstandingType {
    ImageDescription,
    LinkSummary,
    DocumentSummary,
    /// The media was intentionally skipped, with a reason.
    Skipped(String),
}

/// Result of running an understanding pass over a single media item.
#[derive(Debug, Clone)]
pub struct MediaUnderstanding {
    /// The local media that was analysed.
    pub media: LocalMedia,
    /// Human-readable description / summary produced by the model.
    pub description: String,
    /// The kind of understanding that was performed.
    pub understanding_type: UnderstandingType,
}

// ---------------------------------------------------------------------------
// MergedMessage
// ---------------------------------------------------------------------------

/// One or more rapid-fire inbound messages merged into a single logical message.
#[derive(Debug, Clone)]
pub struct MergedMessage {
    /// Combined text content (newline-joined when merged).
    pub text: String,
    /// All attachments from every merged message.
    pub attachments: Vec<Attachment>,
    /// The first (or only) inbound context — used for routing / reply.
    pub primary_context: InboundContext,
    /// IDs of all messages that were merged.
    pub merged_message_ids: Vec<MessageId>,
    /// How many messages were merged.
    pub merge_count: usize,
}

impl MergedMessage {
    /// Wrap a single inbound context as-is.
    pub fn from_single(ctx: InboundContext) -> Self {
        let text = ctx.message.text.clone();
        let attachments = ctx.message.attachments.clone();
        let id = ctx.message.id.clone();
        Self {
            text,
            attachments,
            primary_context: ctx,
            merged_message_ids: vec![id],
            merge_count: 1,
        }
    }

    /// Merge a batch of inbound contexts (must be non-empty).
    ///
    /// Text is joined by newlines; attachments are aggregated in order.
    /// The first context becomes the primary (used for routing).
    pub fn from_batch(contexts: Vec<InboundContext>) -> Self {
        assert!(!contexts.is_empty(), "from_batch requires at least one context");

        let mut iter = contexts.into_iter();
        let first = iter.next().unwrap();

        let mut texts = vec![first.message.text.clone()];
        let mut attachments = first.message.attachments.clone();
        let mut ids = vec![first.message.id.clone()];

        for ctx in iter {
            texts.push(ctx.message.text.clone());
            attachments.extend(ctx.message.attachments.clone());
            ids.push(ctx.message.id.clone());
        }

        let merge_count = ids.len();
        Self {
            text: texts.join("\n"),
            attachments,
            primary_context: first,
            merged_message_ids: ids,
            merge_count,
        }
    }
}

// ---------------------------------------------------------------------------
// EnrichedMessage
// ---------------------------------------------------------------------------

/// A merged message that has been enriched with media understanding.
#[derive(Debug, Clone)]
pub struct EnrichedMessage {
    /// The original merged message.
    pub merged: MergedMessage,
    /// Text enriched with media understanding sections appended.
    pub enriched_text: String,
    /// Downloaded local media files.
    pub local_media: Vec<LocalMedia>,
    /// Total tokens consumed by understanding passes.
    pub understanding_tokens: u64,
}

impl EnrichedMessage {
    /// Build an enriched message from pipeline stage outputs.
    pub fn build(
        merged: MergedMessage,
        local_media: Vec<LocalMedia>,
        understandings: Vec<MediaUnderstanding>,
        tokens: u64,
    ) -> Self {
        let enriched_text = Self::build_enriched_text(&merged.text, &understandings);
        Self {
            merged,
            enriched_text,
            local_media,
            understanding_tokens: tokens,
        }
    }

    /// Append an `[Attachment Understanding]` section to the original text.
    ///
    /// Skipped entries are omitted from the output.
    pub fn build_enriched_text(
        original: &str,
        understandings: &[MediaUnderstanding],
    ) -> String {
        let descriptions: Vec<String> = understandings
            .iter()
            .filter(|u| !matches!(u.understanding_type, UnderstandingType::Skipped(_)))
            .map(|u| {
                let filename = u
                    .media
                    .original
                    .filename
                    .as_deref()
                    .unwrap_or("attachment");
                format!("- {}: {}", filename, u.description)
            })
            .collect();

        if descriptions.is_empty() {
            return original.to_string();
        }

        format!(
            "{}\n\n[Attachment Understanding]\n{}",
            original,
            descriptions.join("\n")
        )
    }
}

// ---------------------------------------------------------------------------
// PipelineError
// ---------------------------------------------------------------------------

/// Errors that can occur during message pipeline processing.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("Download failed: {0}")]
    DownloadFailed(String),

    #[error("Understanding failed: {0}")]
    UnderstandingFailed(String),

    #[error("Pipeline cancelled")]
    Cancelled,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    use crate::gateway::channel::{ChannelId, ConversationId, UserId};
    use crate::gateway::inbound_context::ReplyRoute;
    use crate::gateway::router::SessionKey;

    fn make_attachment(id: &str, mime: &str) -> Attachment {
        Attachment {
            id: id.to_string(),
            mime_type: mime.to_string(),
            filename: Some(format!("{id}.bin")),
            size: None,
            url: None,
            path: None,
            data: None,
        }
    }

    fn make_context(text: &str, msg_id: &str) -> InboundContext {
        use crate::gateway::channel::InboundMessage;
        let msg = InboundMessage {
            id: MessageId::new(msg_id),
            channel_id: ChannelId::new("test-ch"),
            conversation_id: ConversationId::new("conv-1"),
            sender_id: UserId::new("user-1"),
            sender_name: None,
            text: text.to_string(),
            attachments: vec![],
            timestamp: Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
        };
        let route = ReplyRoute::new(ChannelId::new("test-ch"), ConversationId::new("conv-1"));
        let session_key = SessionKey::main("main");
        InboundContext::new(msg, route, session_key)
    }

    fn make_context_with_attachments(
        text: &str,
        msg_id: &str,
        attachments: Vec<Attachment>,
    ) -> InboundContext {
        use crate::gateway::channel::InboundMessage;
        let msg = InboundMessage {
            id: MessageId::new(msg_id),
            channel_id: ChannelId::new("test-ch"),
            conversation_id: ConversationId::new("conv-1"),
            sender_id: UserId::new("user-1"),
            sender_name: None,
            text: text.to_string(),
            attachments,
            timestamp: Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
        };
        let route = ReplyRoute::new(ChannelId::new("test-ch"), ConversationId::new("conv-1"));
        let session_key = SessionKey::main("main");
        InboundContext::new(msg, route, session_key)
    }

    fn make_understanding(
        filename: &str,
        desc: &str,
        utype: UnderstandingType,
    ) -> MediaUnderstanding {
        MediaUnderstanding {
            media: LocalMedia {
                original: Attachment {
                    id: "a1".to_string(),
                    mime_type: "image/png".to_string(),
                    filename: Some(filename.to_string()),
                    size: None,
                    url: None,
                    path: None,
                    data: None,
                },
                local_path: PathBuf::from("/tmp/test.png"),
                media_category: MediaCategory::Image,
            },
            description: desc.to_string(),
            understanding_type: utype,
        }
    }

    // --- MediaCategory ---

    #[test]
    fn test_media_category_from_mime() {
        assert_eq!(MediaCategory::from_mime("image/png"), MediaCategory::Image);
        assert_eq!(MediaCategory::from_mime("image/jpeg"), MediaCategory::Image);
        assert_eq!(MediaCategory::from_mime("IMAGE/GIF"), MediaCategory::Image);

        assert_eq!(MediaCategory::from_mime("audio/mp3"), MediaCategory::Audio);
        assert_eq!(MediaCategory::from_mime("audio/wav"), MediaCategory::Audio);

        assert_eq!(MediaCategory::from_mime("video/mp4"), MediaCategory::Video);
        assert_eq!(MediaCategory::from_mime("video/webm"), MediaCategory::Video);

        assert_eq!(
            MediaCategory::from_mime("application/pdf"),
            MediaCategory::Document
        );
        assert_eq!(
            MediaCategory::from_mime("text/plain"),
            MediaCategory::Document
        );

        assert_eq!(
            MediaCategory::from_mime("text/html"),
            MediaCategory::Link
        );

        assert_eq!(
            MediaCategory::from_mime("application/octet-stream"),
            MediaCategory::Unknown
        );
    }

    // --- MergedMessage ---

    #[test]
    fn test_merged_message_single() {
        let ctx = make_context("Hello world", "msg-1");
        let merged = MergedMessage::from_single(ctx);

        assert_eq!(merged.text, "Hello world");
        assert_eq!(merged.merge_count, 1);
        assert_eq!(merged.merged_message_ids.len(), 1);
        assert_eq!(merged.merged_message_ids[0].as_str(), "msg-1");
        assert!(merged.attachments.is_empty());
    }

    #[test]
    fn test_merged_message_batch() {
        let c1 = make_context_with_attachments(
            "First",
            "msg-1",
            vec![make_attachment("a1", "image/png")],
        );
        let c2 = make_context("Second", "msg-2");
        let c3 = make_context_with_attachments(
            "Third",
            "msg-3",
            vec![make_attachment("a2", "application/pdf")],
        );

        let merged = MergedMessage::from_batch(vec![c1, c2, c3]);

        assert_eq!(merged.text, "First\nSecond\nThird");
        assert_eq!(merged.merge_count, 3);
        assert_eq!(merged.merged_message_ids.len(), 3);
        assert_eq!(merged.attachments.len(), 2);
        assert_eq!(merged.primary_context.message.id.as_str(), "msg-1");
    }

    // --- EnrichedMessage ---

    #[test]
    fn test_enriched_text_no_understandings() {
        let text = EnrichedMessage::build_enriched_text("Hello", &[]);
        assert_eq!(text, "Hello");
    }

    #[test]
    fn test_enriched_text_with_understanding() {
        let understandings = vec![make_understanding(
            "photo.png",
            "A cat sitting on a desk",
            UnderstandingType::ImageDescription,
        )];

        let text = EnrichedMessage::build_enriched_text("Check this out", &understandings);

        assert!(text.contains("[Attachment Understanding]"));
        assert!(text.contains("photo.png: A cat sitting on a desk"));
        assert!(text.starts_with("Check this out"));
    }

    #[test]
    fn test_enriched_text_skips_skipped() {
        let understandings = vec![
            make_understanding(
                "photo.png",
                "A landscape",
                UnderstandingType::ImageDescription,
            ),
            make_understanding(
                "video.mp4",
                "too large",
                UnderstandingType::Skipped("File too large".to_string()),
            ),
        ];

        let text = EnrichedMessage::build_enriched_text("Look", &understandings);

        assert!(text.contains("photo.png: A landscape"));
        assert!(!text.contains("video.mp4"));
        assert!(!text.contains("too large"));
    }
}
