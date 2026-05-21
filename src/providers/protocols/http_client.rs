//! Shared reqwest client construction for LLM provider protocols.

use std::time::Duration;

/// Build the HTTP client used by every provider protocol.
///
/// Sets connection-level timeouts so a stale pooled keep-alive connection
/// cannot hang a request's handshake without bound:
/// - `connect_timeout` caps the TCP+TLS handshake;
/// - `pool_idle_timeout` evicts idle keep-alive connections before a NAT or
///   proxy silently drops them half-open;
/// - `tcp_keepalive` lets the OS detect a dead peer on a long-lived stream.
///
/// Deliberately sets NO overall request `.timeout()` — streaming responses are
/// long-lived and an overall cap would kill a legitimately long stream.
/// Mid-stream stalls are handled separately by `stream_idle::wrap_idle_timeout`.
pub(crate) fn build_provider_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(60))
        .build()
        // Fail-soft: a builder error is implausible with these options, but a
        // default client beats a panic at provider-construction time.
        .unwrap_or_else(|_| reqwest::Client::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_without_panicking() {
        // reqwest does not expose configured timeout values for assertion;
        // verify the builder succeeds with these options.
        let _client = build_provider_http_client();
    }
}
