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

/// The response's `Retry-After` as a decimal seconds string, or `None` when the
/// header is absent or unparseable.
///
/// Every protocol adapter reads this header the same way and interpolates it
/// into a `"Rate limited. Retry after {v} seconds."` suggestion that the
/// failover layer parses back into a real delay. Going through one normaliser
/// ([`retry_after_header_secs`](crate::providers::llm_retry::retry_after_header_secs))
/// is what makes that round-trip safe: an HTTP-date value would otherwise be
/// spliced in verbatim and read back as its day-of-month.
pub(crate) fn retry_after_secs(headers: &reqwest::header::HeaderMap) -> Option<String> {
    let raw = headers.get("retry-after")?.to_str().ok()?;
    crate::providers::llm_retry::retry_after_header_secs(raw).map(|s| s.to_string())
}

/// Read a non-2xx response body with a hard cap.
///
/// `build_provider_http_client` deliberately sets no overall request timeout
/// (streaming success responses are long-lived), and `stream_idle` only guards
/// the *success* byte stream — the error path (`response.text()` on a non-OK
/// status) is read unbounded. A provider or proxy that returns non-OK headers
/// then stalls the body would otherwise hang the turn until the 300s per-turn
/// watchdog fires — too late to fail over cleanly. Bounding the error-body read
/// keeps failover prompt. A2/P7: the model still sees a typed error and
/// self-heals; the harness picks no recovery strategy.
pub(crate) async fn read_error_body(response: reqwest::Response) -> String {
    // Long enough for a legitimate multi-KB error envelope over a slow link,
    // short enough that a dead socket fails over well before the turn watchdog.
    const ERROR_BODY_READ_TIMEOUT: Duration = Duration::from_secs(15);
    bounded_body_read(response.text(), ERROR_BODY_READ_TIMEOUT).await
}

/// Validate that a configured `base_url` parses as an `http(s)` URL before
/// the protocol adapters splice it into a final endpoint.
///
/// `base_url` is operator config (`[providers.<name>].base_url`) or a preset
/// string baked into the binary. Defending in depth here means rejecting
/// non-HTTP schemes (`file://`, `javascript:`, `gopher://`, etc.) which reqwest
/// cannot service but which a typo or a tampered preset could otherwise smuggle
/// into the URL parser. Host-level filtering (loopback, RFC1918, cloud
/// metadata IPs) is left to the operator's network policy: requiring it here
/// would break legitimate localhost proxies (`Ollama` on
/// `http://localhost:11434`) and internal HTTPS-terminating relays with no
/// benefit beyond what an egress firewall already provides.
pub(crate) fn validate_provider_base_url(raw: &str) -> Result<reqwest::Url, InvalidBaseUrl> {
    let parsed = reqwest::Url::parse(raw).map_err(|e| InvalidBaseUrl {
        url: raw.to_string(),
        reason: format!("not a parseable URL: {e}"),
    })?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed),
        other => Err(InvalidBaseUrl {
            url: raw.to_string(),
            reason: format!("unsupported scheme '{other}'; expected http or https"),
        }),
    }
}

/// Error returned by [`validate_provider_base_url`]. Surfaced at provider
/// construction so a misconfigured preset or `aleph.toml` entry fails fast
/// with a usable message instead of silently routing to a non-HTTP URL.
#[derive(Debug)]
pub(crate) struct InvalidBaseUrl {
    pub url: String,
    pub reason: String,
}

impl std::fmt::Display for InvalidBaseUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid provider base_url '{}': {}", self.url, self.reason)
    }
}

impl std::error::Error for InvalidBaseUrl {}

/// Transport-agnostic timeout wrapper, split out so the bound is unit-testable
/// without a live socket. Mirrors the previous `unwrap_or_default()` on a read
/// error (empty body); a timeout yields a marker so the surfaced error says the
/// body stalled instead of silently reading empty.
async fn bounded_body_read<F>(read: F, cap: Duration) -> String
where
    F: std::future::Future<Output = reqwest::Result<String>>,
{
    match tokio::time::timeout(cap, read).await {
        Ok(Ok(body)) => body,
        Ok(Err(_)) => String::new(),
        Err(_) => "<error response body read timed out>".to_string(),
    }
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

    #[tokio::test]
    async fn bounded_body_read_returns_body_on_success() {
        let out =
            bounded_body_read(async { Ok("payload".to_string()) }, Duration::from_secs(5)).await;
        assert_eq!(out, "payload");
    }

    #[tokio::test]
    async fn bounded_body_read_times_out_on_stalled_read() {
        // A body read that never completes must not hang past the cap.
        let out = bounded_body_read(
            std::future::pending::<reqwest::Result<String>>(),
            Duration::from_millis(1),
        )
        .await;
        assert_eq!(out, "<error response body read timed out>");
    }

    fn headers_with_retry_after(value: &str) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", value.parse().expect("valid header value"));
        headers
    }

    /// The whole reason this normaliser exists, asserted end to end: header →
    /// the sentence every adapter builds → the delay the failover walk parses
    /// back out. Splicing an HTTP-date in verbatim used to make
    /// `extract_retry_after_str` return the *day of month*, so a server asking
    /// for hours was re-dialed every ~21 seconds.
    #[test]
    fn http_date_retry_after_survives_the_suggestion_round_trip() {
        const THREE_HOURS: u64 = 3 * 3600;
        let at = std::time::SystemTime::now() + Duration::from_secs(THREE_HOURS);
        let normalised = retry_after_secs(&headers_with_retry_after(&httpdate::fmt_http_date(at)))
            .expect("an HTTP-date is a valid Retry-After");

        let suggestion = format!("Rate limited. Retry after {normalised} seconds.");
        let delay = crate::providers::llm_retry::extract_retry_after_str(&suggestion)
            .expect("the adapters' suggestion must parse back into a delay");

        assert!(
            delay >= Duration::from_secs(THREE_HOURS - 5),
            "a multi-hour Retry-After must come back as hours, got {delay:?}"
        );
    }

    #[test]
    fn delay_seconds_retry_after_passes_through() {
        assert_eq!(
            retry_after_secs(&headers_with_retry_after("42")).as_deref(),
            Some("42")
        );
    }

    #[test]
    fn absent_or_unreadable_retry_after_yields_no_hint() {
        assert_eq!(retry_after_secs(&reqwest::header::HeaderMap::new()), None);
        assert_eq!(retry_after_secs(&headers_with_retry_after("soon")), None);
    }
}
