//! `MediaProcessor` — unified entry point that converts `Vec<Attachment>` into
//! `Vec<ContentBlock>` for LLM injection.
//!
//! Processing rules per MIME prefix:
//!
//! | MIME prefix | `supports_vision=true` | `supports_vision=false` |
//! |-------------|----------------------|-----------------------|
//! | `image/*`   | base64 Image block   | `VisionPipeline` fallback → text description |
//! | `audio/*`   | `TranscriptionService` → text | Same |
//! | Other       | `[Attachment: name (mime)]` | Same |
//!
//! Each attachment is processed independently — a single failure produces a
//! fallback text block, never an abort.

use crate::gateway::channel::Attachment;
use crate::media::placeholder::{MediaPlaceholder, MediaPlaceholderType};
use crate::providers::message::ContentBlock;
use crate::sync_primitives::Arc;
use crate::vision::types::{ImageFormat, ImageInput};
use crate::vision::VisionPipeline;

use tracing::{debug, warn};

use super::cache::{CachedMedia, MediaCache};
use super::transcription::TranscriptionService;

/// Unified media processor that converts channel attachments into LLM-ready
/// content blocks.
pub struct MediaProcessor {
    cache: MediaCache,
    transcription: Option<Box<dyn TranscriptionService>>,
    vision: Option<Arc<VisionPipeline>>,
}

impl MediaProcessor {
    /// Create a new processor with optional transcription and vision backends.
    #[must_use]
    pub fn new(
        transcription: Option<Box<dyn TranscriptionService>>,
        vision: Option<Arc<VisionPipeline>>,
    ) -> Self {
        Self {
            cache: MediaCache::new(),
            transcription,
            vision,
        }
    }

    /// Process a slice of attachments into content blocks suitable for LLM
    /// injection.
    ///
    /// - `supports_vision`: whether the target LLM accepts inline image blocks.
    /// - `session_id`: used for cache directory scoping.
    ///
    /// Each attachment is processed independently. Failures produce a fallback
    /// text block rather than aborting the entire batch.
    pub async fn process(
        &self,
        attachments: &[Attachment],
        supports_vision: bool,
        session_id: &str,
        run_id: &str,
    ) -> Vec<ContentBlock> {
        let mut blocks = Vec::with_capacity(attachments.len());

        for attachment in attachments {
            let block = self
                .process_one(attachment, supports_vision, session_id, run_id)
                .await;
            blocks.push(block);
        }

        blocks
    }

    /// Remove all cached files for a session.
    pub fn cleanup(&self, session_id: &str) {
        if let Err(e) = MediaCache::cleanup_session(session_id) {
            warn!(session_id, error = %e, "failed to cleanup media cache for session");
        }
    }

    /// Remove stale session directories (call at startup).
    pub fn cleanup_stale(&self) {
        MediaCache::cleanup_stale();
    }

    // -------------------------------------------------------------------------
    // Internal
    // -------------------------------------------------------------------------

    /// Process a single attachment, returning a fallback text block on error.
    async fn process_one(
        &self,
        attachment: &Attachment,
        supports_vision: bool,
        session_id: &str,
        run_id: &str,
    ) -> ContentBlock {
        let mime = attachment.mime_type.to_ascii_lowercase();

        if mime.starts_with("image/") {
            self.process_image(attachment, supports_vision, session_id, run_id)
                .await
        } else if mime.starts_with("audio/") {
            self.process_audio(attachment, session_id, run_id).await
        } else {
            tracing::info!(
                target: "multimodal",
                probe = "P4_process",
                run_id = %run_id,
                attachment_id = %attachment.id,
                media_type = "other",
                action = "placeholder",
                "Attachment processed"
            );
            ContentBlock::Text {
                text: MediaPlaceholder::new(MediaPlaceholderType::File, &attachment.id).to_text(),
                cache_control: None,
            }
        }
    }

    /// Process an image attachment.
    ///
    /// - `supports_vision=true`: resolve → base64 → `ContentBlock::Image`
    /// - `supports_vision=false`: resolve → base64 → `VisionPipeline` description → text
    async fn process_image(
        &self,
        attachment: &Attachment,
        supports_vision: bool,
        session_id: &str,
        run_id: &str,
    ) -> ContentBlock {
        // Resolve to local file
        let cached = match self.cache.resolve(attachment, session_id).await {
            Ok(c) => c,
            Err(e) => {
                warn!(attachment_id = %attachment.id, error = %e, "failed to resolve image attachment");
                tracing::info!(
                    target: "multimodal",
                    probe = "P4_process",
                    run_id = %run_id,
                    attachment_id = %attachment.id,
                    media_type = "image",
                    action = "error_fallback",
                    "Attachment processed"
                );
                return fallback_text(attachment, &e.to_string());
            }
        };

        tracing::info!(
            target: "multimodal",
            probe = "P3_download",
            run_id = %run_id,
            attachment_id = %attachment.id,
            mime_type = %cached.mime_type,
            size_bytes = cached.size,
            source = if attachment.data.is_some() { "data" } else if attachment.path.is_some() { "path" } else { "url" },
            "Media attachment resolved"
        );

        // Encode to base64
        let b64 = match MediaCache::to_base64(&cached).await {
            Ok(b) => b,
            Err(e) => {
                warn!(attachment_id = %attachment.id, error = %e, "failed to base64-encode image");
                tracing::info!(
                    target: "multimodal",
                    probe = "P4_process",
                    run_id = %run_id,
                    attachment_id = %attachment.id,
                    media_type = "image",
                    action = "error_fallback",
                    "Attachment processed"
                );
                return fallback_text(attachment, &e.to_string());
            }
        };

        if supports_vision {
            // LLM supports inline images — send the raw data
            debug!(attachment_id = %attachment.id, "injecting image as base64 block");
            tracing::info!(
                target: "multimodal",
                probe = "P4_process",
                run_id = %run_id,
                attachment_id = %attachment.id,
                media_type = "image",
                action = "native",
                "Attachment processed"
            );
            ContentBlock::Image {
                data: b64,
                mime_type: attachment.mime_type.clone(),
            }
        } else {
            // LLM does not support vision — use VisionPipeline for description
            tracing::info!(
                target: "multimodal",
                probe = "P4_process",
                run_id = %run_id,
                attachment_id = %attachment.id,
                media_type = "image",
                action = "vision_fallback",
                "Attachment processed"
            );
            self.describe_image_fallback(&b64, &cached, attachment)
                .await
        }
    }

    /// Use the `VisionPipeline` to produce a text description of an image.
    async fn describe_image_fallback(
        &self,
        b64: &str,
        cached: &CachedMedia,
        attachment: &Attachment,
    ) -> ContentBlock {
        let Some(ref vision) = self.vision else {
            debug!(attachment_id = %attachment.id, "no vision pipeline, returning placeholder");
            return ContentBlock::Text {
                text: MediaPlaceholder::new(MediaPlaceholderType::Image, &attachment.id).to_text(),
                cache_control: None,
            };
        };

        let Some(format) = image_format_from_mime(&cached.mime_type) else {
            warn!(
                attachment_id = %attachment.id,
                mime_type = %cached.mime_type,
                "unsupported image format for vision fallback"
            );
            return ContentBlock::Text {
                text: MediaPlaceholder::new(MediaPlaceholderType::Image, &attachment.id).to_text(),
                cache_control: None,
            };
        };
        let input = ImageInput::Base64 {
            data: b64.to_string(),
            format,
        };

        match vision
            .understand_image(&input, "Describe this image concisely.")
            .await
        {
            Ok(result) => {
                debug!(attachment_id = %attachment.id, "vision described image");
                ContentBlock::Text {
                    text: format!("[Image: {}]", result.description),
                    cache_control: None,
                }
            }
            Err(e) => {
                warn!(attachment_id = %attachment.id, error = %e, "vision pipeline failed");
                ContentBlock::Text {
                    text: "[Image: description unavailable]".to_string(),
                    cache_control: None,
                }
            }
        }
    }

    /// Process an audio attachment via the transcription service.
    async fn process_audio(
        &self,
        attachment: &Attachment,
        session_id: &str,
        run_id: &str,
    ) -> ContentBlock {
        let Some(ref transcription) = self.transcription else {
            tracing::info!(
                target: "multimodal",
                probe = "P4_process",
                run_id = %run_id,
                attachment_id = %attachment.id,
                media_type = "audio",
                action = "error_fallback",
                "Attachment processed"
            );
            return ContentBlock::Text {
                text: MediaPlaceholder::new(MediaPlaceholderType::Audio, &attachment.id).to_text(),
                cache_control: None,
            };
        };

        // Resolve to local file
        let cached = match self.cache.resolve(attachment, session_id).await {
            Ok(c) => c,
            Err(e) => {
                warn!(attachment_id = %attachment.id, error = %e, "failed to resolve audio attachment");
                tracing::info!(
                    target: "multimodal",
                    probe = "P4_process",
                    run_id = %run_id,
                    attachment_id = %attachment.id,
                    media_type = "audio",
                    action = "error_fallback",
                    "Attachment processed"
                );
                return fallback_text(attachment, &e.to_string());
            }
        };

        tracing::info!(
            target: "multimodal",
            probe = "P3_download",
            run_id = %run_id,
            attachment_id = %attachment.id,
            mime_type = %cached.mime_type,
            size_bytes = cached.size,
            source = if attachment.data.is_some() { "data" } else if attachment.path.is_some() { "path" } else { "url" },
            "Media attachment resolved"
        );

        match transcription.transcribe(&cached, None).await {
            Ok(result) => {
                debug!(
                    attachment_id = %attachment.id,
                    lang = ?result.language,
                    "transcribed audio"
                );
                tracing::info!(
                    target: "multimodal",
                    probe = "P4_process",
                    run_id = %run_id,
                    attachment_id = %attachment.id,
                    media_type = "audio",
                    action = "transcribe",
                    "Attachment processed"
                );
                ContentBlock::Text {
                    text: format!("[Voice message transcript]:\n\"{}\"", result.text),
                    cache_control: None,
                }
            }
            Err(e) => {
                warn!(attachment_id = %attachment.id, error = %e, "transcription failed");
                tracing::info!(
                    target: "multimodal",
                    probe = "P4_process",
                    run_id = %run_id,
                    attachment_id = %attachment.id,
                    media_type = "audio",
                    action = "error_fallback",
                    "Attachment processed"
                );
                ContentBlock::Text {
                    text: MediaPlaceholder::new(MediaPlaceholderType::Audio, &attachment.id)
                        .to_text(),
                    cache_control: None,
                }
            }
        }
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Build a generic fallback text block for a failed attachment.
fn fallback_text(attachment: &Attachment, error: &str) -> ContentBlock {
    let mime_lower = attachment.mime_type.to_ascii_lowercase();
    let ty = if mime_lower.starts_with("image/") {
        MediaPlaceholderType::Image
    } else if mime_lower.starts_with("audio/") {
        MediaPlaceholderType::Audio
    } else {
        MediaPlaceholderType::File
    };
    ContentBlock::Text {
        text: format!(
            "{} [{}]",
            MediaPlaceholder::new(ty, &attachment.id).to_text(),
            error
        ),
        cache_control: None,
    }
}

/// Best-effort MIME-to-ImageFormat mapping.
///
/// Returns `None` for formats that cannot be reliably converted to a
/// vision-api-compatible format (e.g. SVG, HEIC).
fn image_format_from_mime(mime: &str) -> Option<ImageFormat> {
    let mime_lower = mime.to_ascii_lowercase();
    match mime_lower.as_str() {
        "image/png" => Some(ImageFormat::Png),
        "image/webp" => Some(ImageFormat::WebP),
        // JPEG variants
        "image/jpeg" | "image/jpg" => Some(ImageFormat::Jpeg),
        // SVG and HEIC are not raster formats — vision APIs typically cannot
        // process them directly. Return None so callers can fail gracefully.
        "image/svg+xml" | "image/heic" | "image/heif" => None,
        _ => {
            // Unknown format — don't guess, let caller decide fallback
            None
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_format_from_mime() {
        assert!(matches!(
            image_format_from_mime("image/png"),
            Some(ImageFormat::Png)
        ));
        assert!(matches!(
            image_format_from_mime("image/jpeg"),
            Some(ImageFormat::Jpeg)
        ));
        assert!(matches!(
            image_format_from_mime("image/jpg"),
            Some(ImageFormat::Jpeg)
        ));
        assert!(matches!(
            image_format_from_mime("image/webp"),
            Some(ImageFormat::WebP)
        ));
        // SVG and HEIC are not supported
        assert!(image_format_from_mime("image/svg+xml").is_none());
        assert!(image_format_from_mime("image/heic").is_none());
        // Unknown formats return None
        assert!(image_format_from_mime("image/bmp").is_none());
    }

    #[test]
    fn test_fallback_text_image() {
        let att = Attachment {
            id: "att-1".into(),
            mime_type: "image/png".into(),
            filename: Some("photo.png".into()),
            size: None,
            url: None,
            path: None,
            data: None,
        };
        let block = fallback_text(&att, "network timeout");
        if let ContentBlock::Text { text, .. } = block {
            assert!(text.starts_with("{{media:image:att-1}}"));
            assert!(text.contains("network timeout"));
        } else {
            panic!("expected Text block");
        }
    }

    #[test]
    fn test_fallback_text_audio() {
        let att = Attachment {
            id: "att-2".into(),
            mime_type: "audio/mp3".into(),
            filename: None,
            size: None,
            url: None,
            path: None,
            data: None,
        };
        let block = fallback_text(&att, "download failed");
        if let ContentBlock::Text { text, .. } = block {
            assert!(text.starts_with("{{media:audio:att-2}}"));
            assert!(text.contains("download failed"));
        } else {
            panic!("expected Text block");
        }
    }

    #[tokio::test]
    async fn test_process_unsupported_mime() {
        let processor = MediaProcessor::new(None, None);
        let att = Attachment {
            id: "att-3".into(),
            mime_type: "application/pdf".into(),
            filename: Some("report.pdf".into()),
            size: None,
            url: None,
            path: None,
            data: None,
        };
        let blocks = processor
            .process(&[att], true, "test-session", "test-run")
            .await;
        assert_eq!(blocks.len(), 1);
        if let ContentBlock::Text { text, .. } = &blocks[0] {
            assert_eq!(text, "{{media:file:att-3}}");
        } else {
            panic!("expected Text block");
        }
    }

    #[tokio::test]
    async fn test_process_image_inline_vision_supported() {
        let processor = MediaProcessor::new(None, None);
        let att = Attachment {
            id: "img-1".into(),
            mime_type: "image/png".into(),
            filename: Some("photo.png".into()),
            size: None,
            url: None,
            path: None,
            data: Some(vec![0x89, 0x50, 0x4E, 0x47]), // PNG magic bytes
        };
        let session_id = "test-image-vision";
        let blocks = processor
            .process(&[att], true, session_id, "test-run")
            .await;
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ContentBlock::Image { mime_type, data } => {
                assert_eq!(mime_type, "image/png");
                assert!(!data.is_empty());
            }
            other => panic!("expected Image block, got {:?}", other),
        }
        // Cleanup
        processor.cleanup(session_id);
    }

    #[tokio::test]
    async fn test_process_image_no_vision_no_pipeline() {
        let processor = MediaProcessor::new(None, None);
        let att = Attachment {
            id: "img-2".into(),
            mime_type: "image/jpeg".into(),
            filename: Some("selfie.jpg".into()),
            size: None,
            url: None,
            path: None,
            data: Some(vec![0xFF, 0xD8, 0xFF]),
        };
        let session_id = "test-no-vision";
        let blocks = processor
            .process(&[att], false, session_id, "test-run")
            .await;
        assert_eq!(blocks.len(), 1);
        if let ContentBlock::Text { text, .. } = &blocks[0] {
            assert_eq!(text, "{{media:image:img-2}}");
        } else {
            panic!("expected Text block");
        }
        processor.cleanup(session_id);
    }

    #[tokio::test]
    async fn test_process_audio_no_transcription() {
        let processor = MediaProcessor::new(None, None);
        let att = Attachment {
            id: "aud-1".into(),
            mime_type: "audio/mp3".into(),
            filename: Some("voice.mp3".into()),
            size: None,
            url: None,
            path: None,
            data: Some(vec![0xFF, 0xFB]),
        };
        let blocks = processor
            .process(&[att], true, "test-audio", "test-run")
            .await;
        assert_eq!(blocks.len(), 1);
        if let ContentBlock::Text { text, .. } = &blocks[0] {
            assert_eq!(text, "{{media:audio:aud-1}}");
        } else {
            panic!("expected Text block");
        }
    }

    #[tokio::test]
    async fn test_process_multiple_attachments_independent() {
        let processor = MediaProcessor::new(None, None);
        let attachments = vec![
            Attachment {
                id: "img".into(),
                mime_type: "image/png".into(),
                filename: Some("pic.png".into()),
                size: None,
                url: None,
                path: None,
                data: Some(vec![1, 2, 3]),
            },
            Attachment {
                id: "doc".into(),
                mime_type: "application/pdf".into(),
                filename: Some("doc.pdf".into()),
                size: None,
                url: None,
                path: None,
                data: None,
            },
            // This one has no source — should still produce a block
            Attachment {
                id: "bad".into(),
                mime_type: "audio/wav".into(),
                filename: None,
                size: None,
                url: None,
                path: None,
                data: None,
            },
        ];
        let session_id = "test-multi";
        let blocks = processor
            .process(&attachments, true, session_id, "test-run")
            .await;
        // All three should produce blocks (no aborts)
        assert_eq!(blocks.len(), 3);
        processor.cleanup(session_id);
    }
}
