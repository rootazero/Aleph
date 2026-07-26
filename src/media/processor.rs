//! `MediaProcessor` — unified entry point that converts `Vec<Attachment>` into
//! `Vec<ContentBlock>` for LLM injection.
//!
//! Processing rules per MIME prefix:
//!
//! | MIME prefix | `supports_vision=true` | `supports_vision=false` |
//! |-------------|----------------------|-----------------------|
//! | `image/*`   | base64 Image block   | `VisionPipeline` fallback → text description |
//! | `audio/*`   | `TranscriptionService` → text | Same |
//! | Other       | `[Attachment: name (mime, size)]` | Same |
//!
//! Each attachment is processed independently — a single failure produces a
//! fallback text block, never an abort.
//!
//! Every non-native path produces *self-describing* text via [`media_summary`].
//! It used to emit a `{{media:<kind>:<id>}}` placeholder instead, which was a
//! dangling handle: the registry that resolved those ids had no production
//! consumer, so the model received an opaque token naming neither the file nor
//! its type and could not even tell the user what had arrived.

use crate::gateway::channel::Attachment;
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
                text: media_summary("Attachment", attachment, None, None),
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
            debug!(attachment_id = %attachment.id, "no vision pipeline, describing image in text");
            return ContentBlock::Text {
                text: media_summary(
                    "Image",
                    attachment,
                    Some(cached.size),
                    Some("not viewable by this model"),
                ),
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
                text: media_summary(
                    "Image",
                    attachment,
                    Some(cached.size),
                    Some("format not supported for description"),
                ),
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
                text: media_summary("Audio", attachment, None, Some("transcription unavailable")),
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
                    text: media_summary(
                        "Audio",
                        attachment,
                        Some(cached.size),
                        Some("transcription failed"),
                    ),
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
    ContentBlock::Text {
        text: media_summary(
            media_kind(&attachment.mime_type),
            attachment,
            None,
            Some(&format!("could not be retrieved: {error}")),
        ),
        cache_control: None,
    }
}

/// Display label for a MIME type.
fn media_kind(mime: &str) -> &'static str {
    let mime_lower = mime.to_ascii_lowercase();
    if mime_lower.starts_with("image/") {
        "Image"
    } else if mime_lower.starts_with("audio/") {
        "Audio"
    } else {
        "Attachment"
    }
}

/// Self-describing text for media the model cannot receive natively, e.g.
/// `[Attachment: report.pdf (application/pdf, 2.3 MB)]`.
///
/// Names the file and its type so the model can at least tell the user what
/// arrived — the whole point of replacing the old opaque `{{media:...}}` token.
/// `size` overrides the attachment's own metadata when the resolved byte count
/// is known; `note` appends a short reason such as `transcription failed`.
fn media_summary(
    kind: &str,
    attachment: &Attachment,
    size: Option<u64>,
    note: Option<&str>,
) -> String {
    let name = attachment
        .filename
        .as_deref()
        .filter(|n| !n.is_empty())
        .unwrap_or(&attachment.id);
    let size = size
        .or(attachment.size)
        .or_else(|| attachment.data.as_ref().map(|d| d.len() as u64));
    let details = match size {
        Some(size) => format!("{}, {}", attachment.mime_type, human_size(size)),
        None => attachment.mime_type.clone(),
    };
    match note {
        Some(note) => format!("[{kind}: {name} ({details}) — {note}]"),
        None => format!("[{kind}: {name} ({details})]"),
    }
}

/// Render a byte count for humans, e.g. `512 B`, `1.5 KB`, `2.3 MB`.
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    #[allow(clippy::cast_precision_loss)] // display only
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
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
            // The model must be able to name the file and its type, which the
            // old `{{media:image:att-1}}` token could not express.
            assert!(text.starts_with("[Image: photo.png (image/png"), "{text}");
            assert!(text.contains("network timeout"));
            assert!(!text.contains("{{media:"), "no dangling handle: {text}");
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
            // No filename on this attachment — fall back to the id, still
            // alongside the MIME type.
            assert!(text.starts_with("[Audio: att-2 (audio/mp3"), "{text}");
            assert!(text.contains("download failed"));
        } else {
            panic!("expected Text block");
        }
    }

    #[test]
    fn test_media_summary_names_file_and_size() {
        let att = Attachment {
            id: "att-9".into(),
            mime_type: "application/pdf".into(),
            filename: Some("report.pdf".into()),
            size: Some(2_411_724),
            url: None,
            path: None,
            data: None,
        };
        assert_eq!(
            media_summary("Attachment", &att, None, None),
            "[Attachment: report.pdf (application/pdf, 2.3 MB)]"
        );
    }

    #[test]
    fn test_human_size_units() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
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
            assert_eq!(text, "[Attachment: report.pdf (application/pdf)]");
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
            assert_eq!(
                text,
                "[Image: selfie.jpg (image/jpeg, 3 B) — not viewable by this model]"
            );
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
            assert_eq!(
                text,
                "[Audio: voice.mp3 (audio/mp3, 2 B) — transcription unavailable]"
            );
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
