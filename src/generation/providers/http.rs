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
