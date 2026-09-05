//! Shared HTTP client policy for speech/transcription providers.
//!
//! One constructor so every voice-path provider carries the same defenses:
//!
//! - **`pool_max_idle_per_host(0)`** — no keep-alive reuse. The
//!   OpenAI-compatible endpoints we target in production (e.g. api.302.ai) sit
//!   behind load balancers that silently drop idle sockets; reqwest's pool then
//!   hands out a dead connection and the next request writes into the void and
//!   hangs the full `timeout` before failing (the voice-mode "stuck at Thinking"
//!   / leading-sentence-eaten bug). Speech traffic is low-QPS and bursty, and
//!   TLS session resumption keeps a fresh dial cheap.
//! - **`connect_timeout`** — bounds that fresh dial so an unreachable endpoint
//!   fails in seconds, not the OS TCP timeout.
//! - **`timeout`** — per-request total cap, taken from the provider's
//!   `timeout_seconds` config knob.

use std::time::Duration;

use crate::generation::error::{GenerationError, GenerationResult};

/// Fresh-dial bound. Generous enough for a cold TLS handshake through a slow
/// proxy, small enough that a black-holed endpoint fails fast.
pub(crate) const CONNECT_TIMEOUT_SECS: u64 = 8;

/// Build the hardened client shared by speech/transcription providers.
pub(crate) fn voice_http_client(timeout: Duration) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
        .pool_max_idle_per_host(0)
        .build()
}

/// Build the plain client the image / video / music providers share.
///
/// The same policy each of their `new()` constructors used to write inline
/// (`Client::builder().timeout(t).build()`), in ONE place so the per-request
/// cap has one derivation. Deliberately NOT the hardened
/// [`voice_http_client`]: disabling keep-alive is a defense the speech
/// endpoints need and a cost the others have never paid, and quietly changing
/// fifteen providers' connection policy is not what "wire up a timeout" means.
pub(crate) fn generation_http_client(timeout: Duration) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder().timeout(timeout).build()
}

/// Apply a provider's configured `timeout_seconds` to the client it makes its
/// requests with.
///
/// Each provider supplies the field; the clamp, the build and the error
/// mapping live here, so wiring the knob into a sixteenth provider is three
/// lines that cannot get the policy subtly wrong.
///
/// ⚠️ Two providers deliberately do NOT use this. [`OpenAiTtsProvider`] and
/// [`AzureSpeechProvider`] carry their own `with_timeout`: they hold a
/// `timeout: Duration` field that has to stay in step with the client, and
/// they rebuild through [`voice_http_client`] rather than this one.
///
/// ⚠️ This is a PER-REQUEST cap, which is what `timeout_seconds` is
/// documented as ("Request timeout in seconds"). A provider that polls a queue
/// bounds the whole JOB separately -- see `fal`'s deadline and
/// `google_veo`'s `MAX_POLL_ATTEMPTS`. Do not point this at those.
pub(crate) trait WithRequestTimeout: Sized {
    /// The client this provider makes its requests with.
    fn request_client_mut(&mut self) -> &mut reqwest::Client;

    /// Rebuild that client with `secs` as the per-request cap.
    ///
    /// `max(1)` because reqwest reads a zero as "no timeout at all", which is
    /// the opposite of what `timeout_seconds = 0` means. Config validation
    /// already rejects 0, so this is the second gate rather than the first
    /// (判据 §8: a rejected value must not be read as permission).
    fn with_timeout(mut self, secs: u64) -> GenerationResult<Self> {
        *self.request_client_mut() = generation_http_client(Duration::from_secs(secs.max(1)))
            .map_err(|e| GenerationError::network(format!("Failed to build HTTP client: {e}")))?;
        Ok(self)
    }
}


/// Retry a fallible generation operation on transient errors with backoff.
///
/// `is_retryable()` decides whether an attempt is retried (rate limits,
/// timeouts, network errors, 5xx, 429). The wait between attempts honors the
/// provider's `Retry-After` when one is supplied, else the fixed ladder
/// 250ms / 750ms / 2s (max 3 attempts). Non-retryable errors (auth, content
/// filter, invalid params) return immediately.
///
/// Pass a closure that performs ONE attempt: the HTTP call plus its response
/// classification, so a 429/5xx surfaced during response handling is retried
/// the same way a transport error is. The closure must be re-runnable (build
/// the request inside it; do not consume per-attempt state outside).
pub(crate) async fn retry_transient<F, Fut, T>(op_name: &str, mut op: F) -> GenerationResult<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = GenerationResult<T>>,
{
    const MAX_ATTEMPTS: u32 = 3;
    const BACKOFFS: [Duration; (MAX_ATTEMPTS - 1) as usize] =
        [Duration::from_millis(250), Duration::from_millis(750)];
    let mut attempt = 0u32;
    loop {
        match op().await {
            Ok(v) => return Ok(v),
            Err(e) if e.is_retryable() && (attempt as usize) < BACKOFFS.len() => {
                let wait = retry_after_of(&e).unwrap_or(BACKOFFS[attempt as usize]);
                tracing::warn!(
                    op = op_name,
                    attempt = attempt + 1,
                    max_attempts = MAX_ATTEMPTS,
                    wait_ms = wait.as_millis() as u64,
                    error = %e,
                    "transient generation error; retrying after backoff"
                );
                tokio::time::sleep(wait).await;
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Extract the provider-supplied `Retry-After` hint, if the error carries one.
fn retry_after_of(e: &GenerationError) -> Option<Duration> {
    match e {
        GenerationError::RateLimitError { retry_after, .. } => *retry_after,
        _ => None,
    }
}
