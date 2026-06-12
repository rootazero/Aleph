//! Inbound voice middleware — STT transcription for incoming audio attachments.
//!
//! When a user sends a voice message via any Channel (Telegram, Discord, etc.),
//! the audio attachment arrives in the `InboundMessage`. This middleware transcribes
//! it to text before the Agent Loop sees it.
//!
//! Uses Whisper-compatible API directly (no `MediaPipeline` dependency).

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

/// STT unavailability, typed so callers can react without string-matching.
#[derive(Debug)]
pub enum SttUnavailable {
    /// Local sidecar still downloading models.
    Downloading { percent: u8 },
    Error(String),
}

/// Late-bound STT source. Local resolves (port, token) per message because the
/// sidecar port changes per spawn; Static is the existing cloud behavior.
#[derive(Clone)]
pub enum SttSource {
    Static(SttConfig),
    /// Local sidecar, with an optional pre-resolved cloud fallback (spec §4.6:
    /// local failure / model download falls back to cloud automatically; a
    /// cloud default never falls back to local).
    Local { fallback: Option<Box<SttConfig>> },
}

impl SttSource {
    /// Materialize a concrete `SttConfig`, consulting the sidecar when local.
    pub async fn materialize(&self) -> Result<SttConfig, SttUnavailable> {
        match self {
            Self::Static(cfg) => Ok(cfg.clone()),
            Self::Local { fallback } => match Self::local_config().await {
                Ok(cfg) => Ok(cfg),
                Err(unavailable) => match fallback {
                    Some(cloud) => {
                        warn!(
                            reason = ?unavailable,
                            "local STT unavailable — falling back to cloud provider"
                        );
                        Ok((**cloud).clone())
                    }
                    None => Err(unavailable),
                },
            },
        }
    }

    async fn local_config() -> Result<SttConfig, SttUnavailable> {
        let sup = super::sidecar::global()
            .ok_or_else(|| SttUnavailable::Error("local voice not initialized".into()))?;
        let ep = sup
            .ensure_endpoint()
            .await
            .map_err(|e| SttUnavailable::Error(format!("{e:#}")))?;
        Ok(SttConfig {
            api_key: ep.token,
            base_url: ep.base_url,
            model: sup.config().stt_model.clone(),
        })
    }
}

/// Resolve the active STT source from generation config + vault.
///
/// Local (`provider_type == "local"`) wins per the normalized defaults; a cloud
/// candidate (if any) rides along as the fallback. Pure cloud setups produce
/// `Static` exactly as before. Returns `None` when no usable provider exists.
///
/// Shared by the boot path (inbound router wiring) and the `voice.transcribe`
/// panel RPC so both resolve the provider identically.
pub fn resolve_stt_source(
    gen_cfg: &crate::config::types::generation::GenerationConfig,
    vault: &crate::gateway::security::SharedTokenManager,
) -> Option<SttSource> {
    let chosen = choose_transcription_provider(gen_cfg, vault, false)?;
    if chosen.1.provider_type == crate::config::types::voice_local::LOCAL_PROVIDER_TYPE {
        let fallback = choose_transcription_provider(gen_cfg, vault, true)
            .map(|(key, pcfg)| Box::new(static_stt_config(key, pcfg)));
        Some(SttSource::Local { fallback })
    } else {
        let (key, pcfg) = chosen;
        Some(SttSource::Static(static_stt_config(key, pcfg)))
    }
}

/// Selection walk shared by primary + fallback resolution.
/// `skip_local = true` excludes `provider_type == "local"` entries.
///
/// Picks the `default_transcription_provider` when set and enabled, otherwise
/// the first enabled transcription provider with a resolvable API key (config
/// inline or vault `gen:<name>`).
fn choose_transcription_provider<'a>(
    gen_cfg: &'a crate::config::types::generation::GenerationConfig,
    vault: &crate::gateway::security::SharedTokenManager,
    skip_local: bool,
) -> Option<(String, &'a crate::GenerationProviderConfig)> {
    let resolve_key = |name: &str, pcfg: &crate::GenerationProviderConfig| -> Option<String> {
        if let Some(ref key) = pcfg.api_key {
            if !key.is_empty() {
                return Some(key.clone());
            }
        }
        if let Ok(Some(secret)) = vault.get_secret(&format!("gen:{name}")) {
            let val = secret.expose().to_string();
            if !val.is_empty() {
                return Some(val);
            }
        }
        None
    };
    let eligible = |pcfg: &crate::GenerationProviderConfig| {
        pcfg.enabled
            && !(skip_local
                && pcfg.provider_type == crate::config::types::voice_local::LOCAL_PROVIDER_TYPE)
    };

    gen_cfg
        .default_transcription_provider
        .as_ref()
        .and_then(|default_name| {
            gen_cfg
                .transcription_providers
                .get_key_value(default_name)
                .filter(|(_, pcfg)| eligible(pcfg))
                .and_then(|(name, pcfg)| resolve_key(name, pcfg).map(|key| (key, pcfg)))
        })
        .or_else(|| {
            gen_cfg
                .transcription_providers
                .iter()
                .find_map(|(name, pcfg)| {
                    if eligible(pcfg) {
                        resolve_key(name, pcfg).map(|key| (key, pcfg))
                    } else {
                        None
                    }
                })
        })
}

/// The original static `SttConfig` construction (url normalize + model pick).
fn static_stt_config(key: String, pcfg: &crate::GenerationProviderConfig) -> SttConfig {
    let base = pcfg.base_url.as_deref().unwrap_or("https://api.openai.com");
    let resolved = crate::generation::providers::url_normalize::resolve_base_url(base);
    let stt_endpoint = resolved.primary_endpoint(crate::generation::GenerationType::Transcription);
    let stt_base = stt_endpoint
        .trim_end_matches("/audio/transcriptions")
        .to_string();
    let stt_model = pcfg
        .models
        .first()
        .cloned()
        .unwrap_or_else(|| "whisper-1".to_string());

    SttConfig {
        api_key: key,
        base_url: stt_base,
        model: stt_model,
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

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
    let (audio_attachments, other_attachments): (Vec<Attachment>, Vec<Attachment>) = msg
        .attachments
        .drain(..)
        .partition(|a| a.mime_type.starts_with("audio/"));

    // Transcribe the first audio attachment.
    let first_audio = &audio_attachments[0];
    match transcribe_attachment(first_audio, stt_config).await {
        Ok(transcription) if transcription.trim().is_empty() => {
            // Whisper returned nothing usable — empty audio or a hallucination
            // that the filter nulled. Don't claim a transcription (no
            // voice-reply hint, no spurious agent turn on phantom text).
            debug!("Voice transcription empty after hallucination filter");
            msg.attachments = other_attachments;
            if msg.text.is_empty() {
                msg.text = "[No speech detected in audio]".to_string();
            }
            VoiceProcessResult {
                message: msg,
                transcribed: false,
            }
        }
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

/// Download audio bytes from an attachment and transcribe them.
async fn transcribe_attachment(
    attachment: &Attachment,
    config: &SttConfig,
) -> Result<String, String> {
    let (bytes, filename) = get_audio_bytes(attachment).await?;
    transcribe_bytes(bytes, &filename, &attachment.mime_type, None, config).await
}

/// Send raw audio bytes to a Whisper-compatible API and return the transcript.
///
/// Backend-agnostic core shared by the channel inbound middleware
/// ([`transcribe_attachment`]) and the `voice.transcribe` panel RPC. `language`
/// is an optional ISO 639-1 hint passed natively to the API.
pub async fn transcribe_bytes(
    bytes: Vec<u8>,
    filename: &str,
    mime: &str,
    language: Option<&str>,
    config: &SttConfig,
) -> Result<String, String> {
    let file_part = reqwest::multipart::Part::bytes(bytes)
        .file_name(filename.to_string())
        .mime_str(mime)
        .map_err(|e| format!("Invalid MIME type: {e}"))?;

    let mut form = reqwest::multipart::Form::new()
        .part("file", file_part)
        .text("model", config.model.clone())
        .text("response_format", "json");
    if let Some(lang) = language.filter(|l| !l.is_empty()) {
        form = form.text("language", lang.to_string());
    }

    let url = format!(
        "{}/audio/transcriptions",
        config.base_url.trim_end_matches('/')
    );
    debug!(url = %url, model = %config.model, "Sending Whisper transcription request");

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .bearer_auth(&config.api_key)
        .multipart(form)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| format!("Whisper API request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Whisper API error {status}: {body}"));
    }

    #[derive(serde::Deserialize)]
    struct WhisperResponse {
        text: String,
    }

    let result: WhisperResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse Whisper response: {e}"))?;

    // Drop Whisper boilerplate hallucinations (silence/noise decode to "Thanks
    // for watching!" etc.) before the transcript becomes agent context.
    // Conservative: only nulls a transcript that is *entirely* boilerplate or a
    // degenerate repetition loop, so genuine speech is never eaten.
    Ok(super::hallucination::filter_transcript(&result.text))
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
            .map_err(|e| format!("Failed to read audio file {path}: {e}"))?;
        return Ok((bytes, filename));
    }

    if let Some(ref url) = attachment.url {
        debug!(url = %url, "Downloading audio for transcription");
        let resp = reqwest::get(url)
            .await
            .map_err(|e| format!("Failed to download audio from {url}: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("Audio download failed: HTTP {}", resp.status()));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("Failed to read audio bytes: {e}"))?;
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

    fn make_vault() -> (tempfile::TempDir, crate::gateway::security::SharedTokenManager) {
        use crate::gateway::security::{SecurityStore, SharedTokenManager};
        let dir = tempfile::TempDir::new().unwrap();
        let store = std::sync::Arc::new(SecurityStore::in_memory().unwrap());
        let vault = SharedTokenManager::new(store, dir.path().join("test.vault"));
        (dir, vault)
    }

    #[test]
    fn resolve_prefers_local_with_cloud_fallback() {
        use crate::config::types::generation::GenerationConfig;
        let (_dir, vault) = make_vault();
        let mut gen = GenerationConfig::default();
        // local (normalized shape) + one cloud provider with inline key
        let mut local = crate::GenerationProviderConfig::new("local");
        local.api_key = Some("local-sidecar".into());
        gen.transcription_providers.insert("local".into(), local);
        let mut cloud = crate::GenerationProviderConfig::new("openai_whisper");
        cloud.api_key = Some("sk-cloud".into());
        gen.transcription_providers
            .insert("openai_whisper".into(), cloud);
        gen.default_transcription_provider = Some("local".into());

        match resolve_stt_source(&gen, &vault) {
            Some(SttSource::Local { fallback: Some(f) }) => assert_eq!(f.api_key, "sk-cloud"),
            other => panic!("expected Local with fallback, got {:?}", other.is_some()),
        }
    }

    #[test]
    fn resolve_pure_cloud_stays_static() {
        use crate::config::types::generation::GenerationConfig;
        let (_dir, vault) = make_vault();
        let mut gen = GenerationConfig::default();
        let mut cloud = crate::GenerationProviderConfig::new("openai_whisper");
        cloud.api_key = Some("sk-cloud".into());
        gen.transcription_providers
            .insert("openai_whisper".into(), cloud);
        match resolve_stt_source(&gen, &vault) {
            Some(SttSource::Static(cfg)) => assert_eq!(cfg.api_key, "sk-cloud"),
            _ => panic!("expected Static"),
        }
    }
}
