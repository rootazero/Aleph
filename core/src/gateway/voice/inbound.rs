//! Inbound voice middleware — STT transcription for incoming audio attachments.
//!
//! When a user sends a voice message via any Channel (Telegram, Discord, etc.),
//! the audio attachment arrives in the InboundMessage. This middleware transcribes
//! it to text before the Agent Loop sees it.
//!
//! Uses Whisper-compatible API directly (no MediaPipeline dependency).

use crate::gateway::channel::{Attachment, InboundMessage};
use tracing::{debug, warn};

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

/// Configuration for inbound voice STT.
#[derive(Clone)]
pub struct SttConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
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
/// - Has audio → downloads audio bytes, sends to Whisper API for transcription.
///   - On success: sets text to transcription, removes audio attachments.
///   - On failure: keeps attachments, appends error hint.
pub async fn process_inbound_voice(
    mut msg: InboundMessage,
    stt_config: &SttConfig,
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
    match transcribe_attachment(first_audio, stt_config).await {
        Ok(transcription) => {
            debug!(chars = transcription.len(), "Voice transcription succeeded");
            let new_text = if msg.text.is_empty() {
                transcription
            } else {
                format!("{}\n{}", transcription, msg.text)
            };
            msg.text = new_text;
            msg.attachments = other_attachments;
            VoiceProcessResult {
                message: msg,
                transcribed: true,
            }
        }
        Err(e) => {
            warn!(error = %e, "Voice transcription failed");
            let mut all_attachments = audio_attachments;
            all_attachments.extend(other_attachments);
            msg.attachments = all_attachments;
            if msg.text.is_empty() {
                msg.text = "[Voice transcription failed, please resend or use text]".to_string();
            } else {
                msg.text.push_str("\n[Voice transcription failed]");
            }
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

/// Download audio bytes and send to Whisper-compatible API for transcription.
async fn transcribe_attachment(
    attachment: &Attachment,
    config: &SttConfig,
) -> Result<String, String> {
    // Get audio bytes: from inline data, local file, or download from URL
    let (bytes, filename) = get_audio_bytes(attachment).await?;

    // Send to Whisper API
    let mime = &attachment.mime_type;
    let file_part = reqwest::multipart::Part::bytes(bytes)
        .file_name(filename)
        .mime_str(mime)
        .map_err(|e| format!("Invalid MIME type: {}", e))?;

    let form = reqwest::multipart::Form::new()
        .part("file", file_part)
        .text("model", config.model.clone())
        .text("response_format", "json");

    let url = format!("{}/audio/transcriptions", config.base_url.trim_end_matches('/'));
    debug!(url = %url, model = %config.model, "Sending Whisper transcription request");

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .bearer_auth(&config.api_key)
        .multipart(form)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| format!("Whisper API request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Whisper API error {}: {}", status, body));
    }

    #[derive(serde::Deserialize)]
    struct WhisperResponse {
        text: String,
    }

    let result: WhisperResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse Whisper response: {}", e))?;

    Ok(result.text)
}

/// Get audio bytes from attachment (inline data, local file, or URL download).
async fn get_audio_bytes(attachment: &Attachment) -> Result<(Vec<u8>, String), String> {
    let filename = attachment
        .filename
        .clone()
        .unwrap_or_else(|| "voice.ogg".to_string());

    if let Some(ref data) = attachment.data {
        return Ok((data.clone(), filename));
    }

    if let Some(ref path) = attachment.path {
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|e| format!("Failed to read audio file {}: {}", path, e))?;
        return Ok((bytes, filename));
    }

    if let Some(ref url) = attachment.url {
        debug!(url = %url, "Downloading audio for transcription");
        let resp = reqwest::get(url)
            .await
            .map_err(|e| format!("Failed to download audio from {}: {}", url, e))?;
        if !resp.status().is_success() {
            return Err(format!("Audio download failed: HTTP {}", resp.status()));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("Failed to read audio bytes: {}", e))?;
        return Ok((bytes.to_vec(), filename));
    }

    Err("Audio attachment has no data, path, or URL".to_string())
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
