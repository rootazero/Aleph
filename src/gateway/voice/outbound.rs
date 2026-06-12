//! Outbound voice helpers — TTS generation for voice mode replies.

use crate::config::types::generation::GenerationConfig;
use crate::gateway::channel::Attachment;
use crate::generation::{
    GenerationData, GenerationParams, GenerationProviderRegistry, GenerationRequest, GenerationType,
};
use tracing::{debug, warn};

use super::state::VoiceState;

/// Calculate a reasonable TTS timeout based on text length.
///
/// Base 10 s, +5 s per 100 characters, capped at 30 s.
#[must_use]
pub fn tts_timeout_ms(text: &str) -> u64 {
    let char_count = text.chars().count() as u64;
    let extra = (char_count / 100) * 5000;
    (10_000 + extra).min(30_000)
}

/// Generate TTS audio for the given text, returning an `Attachment` on success.
///
/// Provider resolution order:
/// 1. `voice_state.provider` (per-channel override)
/// 2. `generation_config.default_speech_provider` (global default)
///
/// Returns `None` if no provider is configured or generation fails.
pub async fn generate_tts(
    text: &str,
    voice_state: &VoiceState,
    generation_registry: &GenerationProviderRegistry,
    generation_config: &GenerationConfig,
) -> Option<Attachment> {
    // Resolve provider ID:
    // 1. Per-channel override from VoiceState
    // 2. Global default_speech_provider from config
    // 3. Fallback: first provider that supports Speech
    let provider_id_owned: Option<String>;
    let provider_id = if let Some(ref p) = voice_state.provider {
        debug!(provider = %p, "TTS: using per-channel provider override");
        p.as_str()
    } else if let Some(ref p) = generation_config.default_speech_provider {
        debug!(provider = %p, "TTS: using default_speech_provider from config");
        p.as_str()
    } else {
        // Auto-detect: find first provider that supports Speech
        provider_id_owned = generation_registry
            .first_for_type(GenerationType::Speech)
            .map(|(name, _)| name);
        match &provider_id_owned {
            Some(p) => {
                debug!(provider = %p, "TTS: auto-detected speech provider (no default configured)");
                p.as_str()
            }
            None => {
                warn!(
                    "TTS: no speech provider available — no override, no default, no auto-detect"
                );
                return None;
            }
        }
    };

    // Look up provider in registry
    let provider = match generation_registry.get(provider_id) {
        Some(p) => p,
        None => {
            warn!(provider = %provider_id, "TTS: provider not found in registry");
            return None;
        }
    };

    // Defensively strip speech-hostile markdown/code/URLs and clamp to the
    // provider's character ceiling. The VoiceModeLayer *asks* the model to
    // avoid these, but R7/P7 forbid trusting compliance — a stray `**` or a
    // bare URL would otherwise be read aloud verbatim, and an over-long reply
    // would hard-error at the provider.
    let spoken = super::sanitize::sanitize_for_tts(text);
    if spoken.trim().is_empty() {
        debug!("TTS: reply has no speakable content after sanitization, skipping");
        return None;
    }

    // Build request with optional voice param
    let mut params = GenerationParams::default();
    if let Some(ref voice) = voice_state.voice {
        params.voice = Some(voice.clone());
    }

    let request = GenerationRequest::new(GenerationType::Speech, &spoken).with_params(params);

    // Execute TTS under a length-aware deadline so a wedged provider can't hang
    // the reply path indefinitely (the provider's own timeout may be minutes).
    let timeout = std::time::Duration::from_millis(tts_timeout_ms(&spoken));
    let output = match tokio::time::timeout(timeout, provider.generate(request)).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            warn!(provider = %provider_id, error = %e, "TTS generation failed");
            return None;
        }
        Err(_) => {
            warn!(provider = %provider_id, timeout_ms = timeout.as_millis(), "TTS generation timed out");
            return None;
        }
    };

    // Convert GenerationData → Attachment
    let id = uuid::Uuid::new_v4().to_string();
    let attachment = match output.data {
        GenerationData::Bytes(bytes) => Attachment {
            id,
            mime_type: output
                .metadata
                .content_type
                .unwrap_or_else(|| "audio/mpeg".to_string()),
            filename: Some("tts_output.mp3".to_string()),
            size: Some(bytes.len() as u64),
            url: None,
            path: None,
            data: Some(bytes),
        },
        GenerationData::Url(url) => Attachment {
            id,
            mime_type: output
                .metadata
                .content_type
                .unwrap_or_else(|| "audio/mpeg".to_string()),
            filename: Some("tts_output.mp3".to_string()),
            size: None,
            url: Some(url),
            path: None,
            data: None,
        },
        GenerationData::LocalPath(path) => Attachment {
            id,
            mime_type: output
                .metadata
                .content_type
                .unwrap_or_else(|| "audio/mpeg".to_string()),
            filename: Some("tts_output.mp3".to_string()),
            size: None,
            url: None,
            path: Some(path),
            data: None,
        },
    };

    Some(attachment)
}

// ---------------------------------------------------------------------------
// Outcome layer — typed result that lets the reply emitter skip the failure
// counter when the local sidecar is still downloading its models.
// ---------------------------------------------------------------------------

/// TTS attempt outcome — lets the reply emitter distinguish "model still
/// downloading" (not a failure, don't count) from real failures (count,
/// 3-strike auto-disable preserved).
pub enum TtsOutcome {
    Generated(Attachment),
    /// Local sidecar still fetching models; carry progress for the user hint.
    Downloading { percent: Option<u8> },
    Failed,
}

/// Pure decision: map a remote model state probe to a preflight outcome.
/// `None` means "proceed with generation".
pub fn preflight_outcome(
    state: Option<&crate::gateway::voice::sidecar::RemoteModelState>,
) -> Option<TtsOutcome> {
    match state {
        Some(crate::gateway::voice::sidecar::RemoteModelState::Downloading { percent }) => {
            Some(TtsOutcome::Downloading { percent: Some(*percent) })
        }
        _ => None,
    }
}

/// Like [`generate_tts`] but with a typed outcome. Preflights the local
/// sidecar's model state when the resolved provider is "local".
pub async fn generate_tts_outcome(
    text: &str,
    voice_state: &VoiceState,
    generation_registry: &GenerationProviderRegistry,
    generation_config: &GenerationConfig,
) -> TtsOutcome {
    let resolved_local = voice_state.provider.as_deref() == Some("local")
        || (voice_state.provider.is_none()
            && generation_config.default_speech_provider.as_deref() == Some("local"));
    if resolved_local {
        if let Some(sup) = crate::gateway::voice::sidecar::global() {
            match sup.tts_model_state().await {
                Ok(state) => {
                    if let Some(outcome) = preflight_outcome(Some(&state)) {
                        return outcome;
                    }
                }
                Err(e) => {
                    warn!(error = %e, "local TTS preflight failed");
                    return TtsOutcome::Failed;
                }
            }
        }
    }
    match generate_tts(text, voice_state, generation_registry, generation_config).await {
        Some(attachment) => TtsOutcome::Generated(attachment),
        None => TtsOutcome::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tts_timeout_short_text() {
        // < 100 chars → base 10s only
        let ms = tts_timeout_ms("Hello world");
        assert_eq!(ms, 10_000);
    }

    #[test]
    fn tts_timeout_medium_text() {
        // 250 chars → 10s + 2*5s = 20s
        let text: String = "x".repeat(250);
        let ms = tts_timeout_ms(&text);
        assert_eq!(ms, 20_000);
    }

    #[test]
    fn tts_timeout_capped() {
        // 10_000 chars → would be 10s + 500s but capped at 30s
        let text: String = "x".repeat(10_000);
        let ms = tts_timeout_ms(&text);
        assert_eq!(ms, 30_000);
    }

    #[test]
    fn tts_timeout_empty() {
        let ms = tts_timeout_ms("");
        assert_eq!(ms, 10_000);
    }

    #[test]
    fn tts_timeout_exactly_100_chars() {
        let text: String = "x".repeat(100);
        let ms = tts_timeout_ms(&text);
        // 100/100 = 1 → 10s + 5s = 15s
        assert_eq!(ms, 15_000);
    }

    #[test]
    fn preflight_maps_downloading_only() {
        use crate::gateway::voice::sidecar::RemoteModelState as S;
        assert!(matches!(
            preflight_outcome(Some(&S::Downloading { percent: 42 })),
            Some(TtsOutcome::Downloading { percent: Some(42) })
        ));
        assert!(preflight_outcome(Some(&S::Ready)).is_none());
        assert!(preflight_outcome(Some(&S::Other("error".into()))).is_none());
        assert!(preflight_outcome(None).is_none());
    }
}
