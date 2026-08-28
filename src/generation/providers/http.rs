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
    const BACKOFFS: [Duration; (MAX_ATTEMPTS - 1) as usize] = [
        Duration::from_millis(250),
        Duration::from_millis(750),
    ];
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
