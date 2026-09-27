//! Media handling for outbound WhatsApp messages.
//!
//! Conservative R1 implementation: size validation + MIME routing only.
//! Real re-encoding (JPEG quality, dimension resize) is deferred to R2 so the
//! core stays free of the `image` crate (CLAUDE.md R3: Core minimalism).
//!
//! The `MediaConfig` fields `auto_optimize`, `jpeg_quality`, and `max_dimension`
//! are reserved for R2 wiring and intentionally unread here. When
//! `auto_optimize = true` is configured but the encoder is not yet wired up,
//! the constructor logs a warning so operators are not misled into believing
//! images are being optimized (CLAUDE.md §11: no-op surfaces itself).

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::gateway::channel::Attachment;

/// Configuration for outbound media handling.
///
/// Fields `auto_optimize`, `jpeg_quality`, and `max_dimension` are reserved
/// for the future R2 image pipeline and are not consulted by the current
/// implementation; they remain on the struct for config backward compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaConfig {
    pub max_inbound_mb: u64,
    pub max_outbound_mb: u64,
    pub auto_optimize: bool,
    pub jpeg_quality: u8,
    pub max_dimension: u32,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            max_inbound_mb: 50,
            max_outbound_mb: 50,
            auto_optimize: true,
            jpeg_quality: 85,
            max_dimension: 1920,
        }
    }
}

/// Result of preparing an attachment for outbound delivery.
#[derive(Debug, Clone)]
pub struct OutboundMedia {
    /// Best-effort copy of the attachment payload when the caller supplied
    /// inline `data`. Empty when the caller will load from `path`.
    pub data: Vec<u8>,
    /// Final MIME type after routing (e.g. `audio/ogg; codecs=opus`).
    pub mime_type: String,
    /// True for OGG/Opus payloads that should be sent as a voice note.
    pub is_voice_note: bool,
}

/// Validates and routes outbound attachments.
///
/// R1 is conservative: it refuses oversize attachments (fail-closed per
/// CLAUDE.md §8) and rewrites well-known MIME types, but it does not
/// transcode images or videos. The re-encoding work lands in R2.
pub struct MediaProcessor {
    config: MediaConfig,
}

impl MediaProcessor {
    /// Build a processor with the supplied configuration.
    ///
    /// Logs a single warning when `auto_optimize` is set but the encoder is
    /// not yet wired up, so operators see that the flag is currently inert.
    #[must_use]
    pub fn new(config: MediaConfig) -> Self {
        if config.auto_optimize {
            tracing::warn!(
                "whatsapp media.auto_optimize=true is configured but image re-encoding \
                 is not yet wired up; payloads will be sent unmodified until the R2 \
                 encoder lands."
            );
        }
        Self { config }
    }

    /// Validate and route an outbound attachment.
    ///
    /// Steps:
    /// 1. Size check against `max_outbound_mb` (fail-closed).
    /// 2. MIME routing: images → `image/jpeg`, OGG audio → voice note,
    ///    everything else passes through.
    pub async fn prepare_outbound(&self, attachment: &Attachment) -> Result<OutboundMedia> {
        let max_bytes = self.config.max_outbound_mb * 1024 * 1024;
        if let Some(size) = attachment.size {
            if size > max_bytes {
                return Err(anyhow!(
                    "media too large: {size} bytes (max {} MB)",
                    self.config.max_outbound_mb
                ));
            }
        }

        let mime = attachment.mime_type.as_str();
        let data = attachment.data.clone().unwrap_or_default();

        if mime == "audio/ogg" {
            return Ok(OutboundMedia {
                data,
                mime_type: "audio/ogg; codecs=opus".to_string(),
                is_voice_note: true,
            });
        }

        if mime.starts_with("image/") {
            return Ok(OutboundMedia {
                data,
                mime_type: "image/jpeg".to_string(),
                is_voice_note: false,
            });
        }

        Ok(OutboundMedia {
            data,
            mime_type: attachment.mime_type.clone(),
            is_voice_note: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(mime: &str, size: Option<u64>, data: Option<Vec<u8>>) -> Attachment {
        Attachment {
            id: format!("att-{mime}"),
            mime_type: mime.to_string(),
            filename: None,
            size,
            url: None,
            path: None,
            data,
        }
    }

    #[tokio::test]
    async fn prepare_outbound_rejects_oversized() {
        let cfg = MediaConfig {
            max_inbound_mb: 50,
            max_outbound_mb: 1,
            auto_optimize: false,
            jpeg_quality: 85,
            max_dimension: 1920,
        };
        let processor = MediaProcessor::new(cfg);
        let att = attachment("image/png", Some(2 * 1024 * 1024), Some(vec![0u8; 16]));
        let err = processor
            .prepare_outbound(&att)
            .await
            .expect_err("oversized attachment must be rejected (fail-closed)");
        let msg = format!("{err}");
        assert!(
            msg.contains("too large") || msg.contains("MB"),
            "error should mention size: {msg}"
        );
    }

    #[tokio::test]
    async fn prepare_audio_ogg_marks_as_voice_note() {
        let processor = MediaProcessor::new(MediaConfig::default());
        let att = attachment("audio/ogg", Some(8), Some(vec![0u8; 8]));
        let out = processor
            .prepare_outbound(&att)
            .await
            .expect("voice note prep should succeed");
        assert_eq!(out.mime_type, "audio/ogg; codecs=opus");
        assert!(out.is_voice_note);
    }

    #[tokio::test]
    async fn prepare_image_passthrough_routes_to_jpeg() {
        let processor = MediaProcessor::new(MediaConfig::default());
        let att = attachment("image/png", Some(32), Some(vec![1u8; 32]));
        let out = processor
            .prepare_outbound(&att)
            .await
            .expect("image prep should succeed");
        assert_eq!(out.mime_type, "image/jpeg");
        assert!(!out.is_voice_note);
        assert_eq!(out.data, vec![1u8; 32]);
    }

    #[tokio::test]
    async fn prepare_unknown_mime_passes_through() {
        let processor = MediaProcessor::new(MediaConfig::default());
        let att = attachment("application/pdf", Some(64), Some(vec![2u8; 64]));
        let out = processor.prepare_outbound(&att).await.unwrap();
        assert_eq!(out.mime_type, "application/pdf");
        assert!(!out.is_voice_note);
    }
}