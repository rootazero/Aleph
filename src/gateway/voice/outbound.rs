//! Outbound voice helpers — TTS generation for voice mode replies.

use crate::config::types::generation::GenerationConfig;
use crate::gateway::channel::Attachment;
use crate::generation::{
    GenerationData, GenerationOutput, GenerationParams, GenerationProviderRegistry,
    GenerationRequest, GenerationResult, GenerationType,
};
use std::time::Duration;
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

/// Maximum TTS attempts per sentence. The *primary* defence against the
/// stale-keep-alive stall now lives in the provider's HTTP client (connection
/// pooling is disabled, so every request dials a live socket — see
/// `OpenAiTtsProvider::new`). This retry is the residual safety net for a
/// *genuine* transient failure on a fresh connection — the endpoint returns a
/// 5xx/429, or a real network blip drops the request — where a second attempt
/// recovers. Capped at 2 so a genuinely-down endpoint cannot multiply the reply
/// latency without bound — openclaw's lesson (attempt, then fall back) applied
/// to a single provider: retry, never hammer.
const TTS_MAX_ATTEMPTS: u32 = 2;

/// Settle between TTS attempts — long enough to let a stale connection drop and
/// a cold endpoint begin warming, short enough to stay imperceptible against the
/// multi-second synth itself.
const TTS_RETRY_BACKOFF: Duration = Duration::from_millis(300);

/// Run one TTS `attempt` under a per-attempt deadline, retrying *transient*
/// failures up to `max_attempts`.
///
/// A `tokio` timeout (the attempt outran `per_attempt`) is always transient. An
/// inner [`crate::generation::GenerationError`] is retried only when it is
/// [`is_retryable`](crate::generation::GenerationError::is_retryable) — network,
/// timeout, 5xx, 429 or rate limit — so auth, invalid-parameter and format
/// errors fail fast (a retry cannot fix them). Returns the first success, or
/// `None` once attempts are exhausted or a non-retryable error is hit. The same
/// length-aware deadline applies to every attempt.
async fn synth_with_retry<F, Fut>(
    per_attempt: Duration,
    max_attempts: u32,
    backoff: Duration,
    provider_id: &str,
    mut attempt: F,
) -> Option<GenerationOutput>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = GenerationResult<GenerationOutput>>,
{
    for n in 1..=max_attempts {
        match tokio::time::timeout(per_attempt, attempt()).await {
            Ok(Ok(output)) => return Some(output),
            Ok(Err(e)) => {
                warn!(provider = %provider_id, attempt = n, error = %e, "TTS generation failed");
                // Auth / invalid-param / format errors are deterministic — a
                // retry would only burn another round-trip on the same failure.
                if !e.is_retryable() {
                    return None;
                }
            }
            Err(_) => {
                warn!(
                    provider = %provider_id,
                    attempt = n,
                    timeout_ms = per_attempt.as_millis(),
                    "TTS generation timed out"
                );
            }
        }
        if n < max_attempts {
            tokio::time::sleep(backoff).await;
        }
    }
    None
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

    // Build request params once. `with_params` consumes them, so each attempt
    // rebuilds the request from a cheap clone (the prompt stays borrowed).
    let mut params = GenerationParams::default();
    if let Some(ref voice) = voice_state.voice {
        params.voice = Some(voice.clone());
    }

    // Execute TTS under a length-aware per-attempt deadline so a wedged provider
    // can't hang the reply path (the provider's own timeout may be minutes), and
    // retry transient cold-start failures so a flaky endpoint doesn't silently
    // eat the leading sentences. `synth_with_retry` logs each failed attempt.
    let timeout = Duration::from_millis(tts_timeout_ms(&spoken));
    let output = synth_with_retry(
        timeout,
        TTS_MAX_ATTEMPTS,
        TTS_RETRY_BACKOFF,
        provider_id,
        || {
            let request =
                GenerationRequest::new(GenerationType::Speech, &spoken).with_params(params.clone());
            provider.generate(request)
        },
    )
    .await;
    let output = output?;

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
// Outcome layer — typed result for the reply emitter's 3-strike counter.
// ---------------------------------------------------------------------------

/// TTS attempt outcome (3-strike auto-disable counts `Failed`).
///
/// Crate-internal: the only consumer is the reply emitter's `send_as_voice`;
/// keeping it `pub(crate)` means any future variant surfaces every match site
/// at compile time instead of leaking into the crate API.
pub(crate) enum TtsOutcome {
    Generated(Attachment),
    Failed,
}

/// Like [`generate_tts`] but with a typed outcome.
pub(crate) async fn generate_tts_outcome(
    text: &str,
    voice_state: &VoiceState,
    generation_registry: &GenerationProviderRegistry,
    generation_config: &GenerationConfig,
) -> TtsOutcome {
    match generate_tts(text, voice_state, generation_registry, generation_config).await {
        Some(attachment) => TtsOutcome::Generated(attachment),
        None => TtsOutcome::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::GenerationError;
    use std::cell::Cell;

    fn ok_output() -> GenerationOutput {
        GenerationOutput::new(GenerationType::Speech, GenerationData::bytes(vec![1, 2, 3]))
    }

    // The cold-start bug: the first synth attempt fails transiently (stale
    // keep-alive connection), the endpoint recovers, the retry succeeds. Before
    // this helper that first failure was silently dropped → the leading sentence
    // had no audio. Now it recovers on the second attempt.
    #[tokio::test]
    async fn retry_recovers_after_transient_failure() {
        let calls = Cell::new(0u32);
        let out = synth_with_retry(
            Duration::from_secs(5),
            TTS_MAX_ATTEMPTS,
            Duration::from_millis(1),
            "test",
            || {
                calls.set(calls.get() + 1);
                let first = calls.get() == 1;
                async move {
                    if first {
                        Err(GenerationError::network("error sending request"))
                    } else {
                        Ok(ok_output())
                    }
                }
            },
        )
        .await;
        assert!(out.is_some(), "should recover on the second attempt");
        assert_eq!(calls.get(), 2, "exactly two attempts");
    }

    // A deterministic error (bad key) must NOT be retried — a second round-trip
    // would only burn latency on the same guaranteed failure.
    #[tokio::test]
    async fn no_retry_on_non_retryable_error() {
        let calls = Cell::new(0u32);
        let out = synth_with_retry(
            Duration::from_secs(5),
            TTS_MAX_ATTEMPTS,
            Duration::from_millis(1),
            "test",
            || {
                calls.set(calls.get() + 1);
                async { Err(GenerationError::authentication("bad key", "test")) }
            },
        )
        .await;
        assert!(out.is_none());
        assert_eq!(calls.get(), 1, "auth error fails fast — single attempt");
    }

    // A persistently-down endpoint stops at the attempt cap (bounded latency),
    // returning None so the caller degrades to caption-only.
    #[tokio::test]
    async fn gives_up_after_max_attempts() {
        let calls = Cell::new(0u32);
        let out = synth_with_retry(
            Duration::from_secs(5),
            TTS_MAX_ATTEMPTS,
            Duration::from_millis(1),
            "test",
            || {
                calls.set(calls.get() + 1);
                async { Err(GenerationError::network("down")) }
            },
        )
        .await;
        assert!(out.is_none());
        assert_eq!(calls.get(), TTS_MAX_ATTEMPTS, "stops at the attempt cap");
    }

    // A timed-out attempt (the future outran the per-attempt deadline) is
    // transient and gets retried — this is the 10 s cold-start timeout case.
    #[tokio::test]
    async fn timeout_is_transient_and_retried() {
        let calls = Cell::new(0u32);
        let out = synth_with_retry(
            Duration::from_millis(30),
            TTS_MAX_ATTEMPTS,
            Duration::from_millis(1),
            "test",
            || {
                calls.set(calls.get() + 1);
                let first = calls.get() == 1;
                async move {
                    if first {
                        tokio::time::sleep(Duration::from_millis(300)).await;
                    }
                    Ok(ok_output())
                }
            },
        )
        .await;
        assert!(out.is_some(), "a timed-out first attempt should be retried");
        assert_eq!(calls.get(), 2);
    }

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
}
