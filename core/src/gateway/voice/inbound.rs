//! Inbound voice middleware — STT transcription for incoming audio attachments.
//!
//! When a user sends a voice message via any Channel (Telegram, Discord, etc.),
//! the audio attachment arrives in the InboundMessage. This middleware transcribes
//! it to text before the Agent Loop sees it.

use std::sync::Arc;

use crate::gateway::channel::{Attachment, InboundMessage};
use crate::media::pipeline::MediaPipeline;
use crate::media::types::{AudioFormat, MediaInput, MediaOutput, MediaType};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Result of inbound voice processing.
pub struct VoiceProcessResult {
    /// The (possibly modified) message with transcription applied.
    pub message: InboundMessage,
    /// Whether at least one audio attachment was successfully transcribed.
    pub transcribed: bool,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Returns true if the message contains at least one audio attachment.
pub fn has_audio_attachment(msg: &InboundMessage) -> bool {
    msg.attachments
        .iter()
        .any(|a| a.mime_type.starts_with("audio/"))
}

/// Process inbound voice: transcribe audio attachments to text.
///
/// - No audio attachments → returns message unchanged, `transcribed = false`.
/// - Has audio attachments → transcribes the first audio via MediaPipeline STT.
///   - On success: sets text to transcription (prepends "[Voice] {transcription}\n"
///     if original text was non-empty), removes audio attachments, `transcribed = true`.
///   - On failure: restores all attachments, appends error hint to text,
///     `transcribed = false`.
pub async fn process_inbound_voice(
    mut msg: InboundMessage,
    media_pipeline: &Arc<MediaPipeline>,
) -> VoiceProcessResult {
    if !has_audio_attachment(&msg) {
        return VoiceProcessResult {
            message: msg,
            transcribed: false,
        };
    }

    // Partition attachments into audio and non-audio.
    let (audio_attachments, other_attachments): (Vec<Attachment>, Vec<Attachment>) =
        msg.attachments.drain(..).partition(|a| a.mime_type.starts_with("audio/"));

    // Transcribe the first audio attachment.
    let first_audio = &audio_attachments[0];
    match transcribe_attachment(first_audio, media_pipeline).await {
        Ok(transcription) => {
            // Prepend "[Voice]" marker if original text was non-empty.
            let new_text = if msg.text.is_empty() {
                transcription
            } else {
                format!("[Voice] {}\n{}", transcription, msg.text)
            };
            msg.text = new_text;
            // Keep only non-audio attachments.
            msg.attachments = other_attachments;
            VoiceProcessResult {
                message: msg,
                transcribed: true,
            }
        }
        Err(_) => {
            // Restore all attachments and append failure hint.
            let mut all_attachments = audio_attachments;
            all_attachments.extend(other_attachments);
            msg.attachments = all_attachments;
            msg.text.push_str(
                "\n[Voice transcription failed, please resend or use text]",
            );
            VoiceProcessResult {
                message: msg,
                transcribed: false,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Map a MIME type string to an `AudioFormat`.
fn audio_format_from_mime(mime: &str) -> AudioFormat {
    match mime {
        "audio/mp3" | "audio/mpeg" => AudioFormat::Mp3,
        "audio/ogg" | "audio/opus" => AudioFormat::Ogg,
        "audio/wav" | "audio/wave" => AudioFormat::Wav,
        "audio/flac" => AudioFormat::Flac,
        "audio/m4a" | "audio/mp4" | "audio/x-m4a" => AudioFormat::M4a,
        _ => AudioFormat::Mp3,
    }
}

/// Build a `MediaInput` and invoke `MediaPipeline::process()` for STT.
async fn transcribe_attachment(
    attachment: &Attachment,
    pipeline: &Arc<MediaPipeline>,
) -> Result<String, crate::media::error::MediaError> {
    let format = audio_format_from_mime(&attachment.mime_type);
    let media_type = MediaType::Audio {
        format,
        duration_secs: None,
    };

    // Prefer local path → inline data → remote URL.
    let input = if let Some(path) = &attachment.path {
        MediaInput::FilePath {
            path: path.into(),
        }
    } else if let Some(data) = &attachment.data {
        MediaInput::Base64 {
            data: base64_encode(data),
            media_type: media_type.clone(),
        }
    } else if let Some(url) = &attachment.url {
        MediaInput::Url { url: url.clone() }
    } else {
        // No usable source — return an error without hitting the pipeline.
        return Err(crate::media::error::MediaError::UnsupportedFormat(
            "audio attachment has no path, data, or url".into(),
        ));
    };

    let output = pipeline.process(&input, &media_type, None).await?;

    // Extract text from the output variants.
    let text = match output {
        MediaOutput::Text { text } => text,
        MediaOutput::Description { text, .. } => text,
        MediaOutput::Chunks { chunks } => chunks
            .into_iter()
            .map(|c| c.content)
            .collect::<Vec<_>>()
            .join(" "),
        MediaOutput::Structured { data } => data.to_string(),
    };

    Ok(text)
}

/// Encode raw bytes as base64 (standard alphabet, no padding issues).
fn base64_encode(data: &[u8]) -> String {
    use std::fmt::Write;
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((n >> 6) & 63) as usize] as char);
        } else {
            let _ = write!(out, "=");
        }
        if chunk.len() > 2 {
            out.push(TABLE[(n & 63) as usize] as char);
        } else {
            let _ = write!(out, "=");
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use crate::gateway::channel::{ChannelId, ConversationId, MessageId, UserId};

    fn make_attachment(mime: &str) -> Attachment {
        Attachment {
            id: "att-1".to_string(),
            mime_type: mime.to_string(),
            filename: None,
            size: None,
            url: None,
            path: None,
            data: None,
        }
    }

    fn make_message(attachments: Vec<Attachment>) -> InboundMessage {
        InboundMessage {
            id: MessageId::new("msg-1"),
            channel_id: ChannelId::new("chan-1"),
            conversation_id: ConversationId::new("conv-1"),
            sender_id: UserId::new("user-1"),
            sender_name: None,
            text: String::new(),
            attachments,
            timestamp: Utc::now(),
            reply_to: None,
            is_group: false,
            raw: None,
        }
    }

    #[test]
    fn has_audio_attachment_detects_audio() {
        let msg = make_message(vec![
            make_attachment("image/png"),
            make_attachment("audio/ogg"),
        ]);
        assert!(has_audio_attachment(&msg));
    }

    #[test]
    fn has_audio_attachment_ignores_non_audio() {
        let msg = make_message(vec![
            make_attachment("image/jpeg"),
            make_attachment("application/pdf"),
        ]);
        assert!(!has_audio_attachment(&msg));
    }

    #[test]
    fn has_audio_attachment_empty() {
        let msg = make_message(vec![]);
        assert!(!has_audio_attachment(&msg));
    }
}
