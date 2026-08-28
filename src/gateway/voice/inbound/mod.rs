//! Inbound voice — STT for incoming audio, split by responsibility:
//!
//! - [`stt`] — the Whisper-dialect HTTP core + [`SttSource`] local→cloud
//!   degradation. Knows endpoints, not providers or channels.
//! - [`provider`] — resolves generation config + vault into an [`SttSource`]
//!   (BYO local preferred, cloud fallback riding along).
//! - this module — the **channel middleware**: when a user sends a voice
//!   message via any Channel (Telegram, Discord, ...), the audio attachment
//!   arrives in the `InboundMessage` and is transcribed to text before the
//!   Agent Loop sees it.
//!
//! The `voice.transcribe` panel RPC reuses [`stt`] + [`provider`] directly —
//! one transcription implementation for every path.

pub mod provider;
pub mod stt;

pub use provider::resolve_stt_source;
pub use stt::{transcribe_bytes, transcribe_with_source, SttConfig, SttSource};

use crate::gateway::channel::{Attachment, InboundMessage};
use tracing::{debug, warn};

/// Result of inbound voice processing.
pub struct VoiceProcessResult {
    /// The (possibly modified) message with transcription applied.
    pub message: InboundMessage,
    /// Whether at least one audio attachment was successfully transcribed.
    pub transcribed: bool,
}

/// Returns true if the message contains at least one audio attachment.
#[must_use]
pub fn has_audio_attachment(msg: &InboundMessage) -> bool {
    msg.attachments
        .iter()
        .any(|a| a.mime_type.starts_with("audio/"))
}

/// Process inbound voice: transcribe audio attachments to text.
///
/// - No audio attachments → returns message unchanged, `transcribed = false`.
/// - Has audio → downloads audio bytes, sends to Whisper API for transcription.
///   **Every** audio attachment is transcribed (a Telegram forward can carry
///   several voice notes in one message); transcripts join in order.
///   - ≥ 1 success: sets text to the joined transcription, removes audio
///     attachments (failed ones noted inline rather than silently destroyed).
///   - All failed: keeps attachments, appends error hint.
///   - All empty (silence / hallucination-nulled): no transcription claimed.
pub async fn process_inbound_voice(
    mut msg: InboundMessage,
    stt_source: &SttSource,
) -> VoiceProcessResult {
    if !has_audio_attachment(&msg) {
        return VoiceProcessResult {
            message: msg,
            transcribed: false,
        };
    }

    // Partition attachments into audio and non-audio.
    let (audio_attachments, other_attachments): (Vec<Attachment>, Vec<Attachment>) = msg
        .attachments
        .drain(..)
        .partition(|a| a.mime_type.starts_with("audio/"));

    let mut transcripts: Vec<String> = Vec::new();
    let mut failures = 0usize;
    for (i, audio) in audio_attachments.iter().enumerate() {
        match transcribe_attachment(audio, stt_source).await {
            Ok(t) if t.trim().is_empty() => {
                // Empty audio or a hallucination the filter nulled — not a
                // failure, just nothing said in this clip.
                debug!(
                    index = i,
                    "Voice transcription empty after hallucination filter"
                );
            }
            Ok(t) => {
                debug!(index = i, chars = t.len(), "Voice transcription succeeded");
                transcripts.push(t.trim().to_string());
            }
            Err(e) => {
                warn!(index = i, error = %e, "Voice transcription failed");
                failures += 1;
            }
        }
    }

    if !transcripts.is_empty() {
        let mut transcription = transcripts.join("\n");
        if failures > 0 {
            // Partial degradation: say so instead of silently dropping clips.
            transcription.push_str(&format!(
                "\n[{failures} voice clip(s) failed to transcribe]"
            ));
        }
        let new_text = if msg.text.is_empty() {
            transcription
        } else {
            format!("{}\n{}", transcription, msg.text)
        };
        msg.text = new_text;
        msg.attachments = other_attachments;
        return VoiceProcessResult {
            message: msg,
            transcribed: true,
        };
    }

    if failures == 0 {
        // Every clip was silence/hallucination — don't claim a transcription
        // (no voice-reply hint, no spurious agent turn on phantom text).
        msg.attachments = other_attachments;
        if msg.text.is_empty() {
            msg.text = "[No speech detected in audio]".to_string();
        }
        return VoiceProcessResult {
            message: msg,
            transcribed: false,
        };
    }

    // Nothing usable and at least one hard failure: keep the attachments so
    // the user (or a retry) still has the audio.
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

/// Download audio bytes from an attachment and transcribe them.
async fn transcribe_attachment(
    attachment: &Attachment,
    source: &SttSource,
) -> Result<String, String> {
    let (bytes, filename) = get_audio_bytes(attachment).await?;
    transcribe_with_source(
        bytes::Bytes::from(bytes),
        &filename,
        &attachment.mime_type,
        None,
        source,
    )
    .await
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
        // NOTE on the threat model: `path` is set by the CHANNEL ADAPTER
        // (e.g. a channel that downloaded the media to a local temp dir), so
        // it is trusted input under the R7 brain-limb contract — the adapter
        // is the limb that owns the inbound bytes. Channels that let a
        // remote peer supply the path verbatim would be an LFI vector; that
        // is the adapter's responsibility to prevent, and is flagged in the
        // audit as a follow-up for the webhook channel's payload parsing.
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|e| format!("Failed to read audio file {path}: {e}"))?;
        return Ok((bytes, filename));
    }

    if let Some(ref url) = attachment.url {
        debug!(url = %url, "Downloading audio for transcription");
        // **SSRF guard**: the previous `reqwest::get(url)` followed redirects
        // by default and reached arbitrary external hosts — including cloud
        // metadata endpoints (169.254.169.254) and anything a crafted
        // attachment URL pointed at. `safe_fetch` validates the URL AND every
        // redirect hop against the SSRF policy (scheme allowlist, hostname
        // blocklist, private/loopback/metadata IP rejection), pins DNS to
        // close the rebinding TOCTOU window, strips auth headers on
        // cross-origin redirects, and caps the body so a hostile upstream
        // cannot OOM the process before the transcription path sees it.
        //
        // Channel-CDN media URLs (discord/telegram/etc.) are public https
        // hosts and pass the default policy unchanged.
        const MAX_AUDIO_BYTES: usize = 100 * 1024 * 1024; // 100 MiB
        let policy = crate::security::ssrf::SsrfPolicy::default();
        let mut request =
            crate::security::ssrf::SafeFetchRequest::get(std::time::Duration::from_secs(60));
        request.max_body_bytes = Some(MAX_AUDIO_BYTES);
        let resp = crate::security::ssrf::safe_fetch(url, &policy, request)
            .await
            .map_err(|e| format!("Failed to download audio from {url}: {e}"))?;
        if !resp.status.is_success() {
            return Err(format!("Audio download failed: HTTP {}", resp.status));
        }
        return Ok((resp.body, filename));
    }

    Err("Audio attachment has no data, path, or URL".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::{ChannelId, ConversationId, MessageId, UserId};
    use chrono::Utc;

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
            metadata: vec![],
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
