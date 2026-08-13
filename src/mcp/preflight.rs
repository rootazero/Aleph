//! Pre-flight validation for remote MCP server URLs.
//!
//! A mistyped URL that points at an ordinary web page (an HTML dashboard, a
//! login screen, a 404 page) is indistinguishable from a real MCP endpoint
//! until the JSON-RPC handshake times out — historically a wall of up to the
//! full connect timeout (300s by default). This module performs a cheap,
//! short-timeout probe up front: if the URL clearly serves a web page rather
//! than an MCP transport, the connection is rejected in seconds with an
//! actionable error instead of hanging.
//!
//! The probe is deliberately *lenient*. It rejects only on an unambiguous
//! "this is a web page" signal — a 2xx response whose `Content-Type` is HTML.
//! Network errors, redirects, auth challenges, and method-not-allowed responses
//! all pass through to the real connection path, which owns retry and timeout
//! policy. A conformant streamable-HTTP MCP server answers a bare GET with
//! 405/406/400 or an event-stream, never `200 text/html`. The SSRF policy is
//! enforced before any network access, matching the transport layer.

use std::collections::HashMap;
use std::time::Duration;

use crate::error::{AlephError, Result};
use crate::security::ssrf::{validate_url_async, SsrfPolicy};

/// How long the pre-flight probe waits before giving up and deferring to the
/// real connection. Short by design — the probe is an optimization, not a gate.
const PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(5);

/// Probe a remote MCP URL and reject it early if it serves a web page.
///
/// Returns `Ok(())` when the URL looks like — or merely might be — an MCP
/// endpoint, and `Err` only when the server unambiguously returns an HTML page.
/// The SSRF policy is enforced before any network access.
pub async fn preflight_remote_url(url: &str, headers: &HashMap<String, String>) -> Result<()> {
    let (validated_url, pinned) = validate_url_async(url, &SsrfPolicy::default())
        .await
        .map_err(|e| AlephError::IoError(format!("SSRF blocked for '{url}': {e}")))?;

    let host = validated_url
        .host_str()
        .ok_or_else(|| AlephError::IoError(format!("no host in URL: {url}")))?
        .to_string();

    let mut client_builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(PREFLIGHT_TIMEOUT);
    if let Some(addr) = pinned {
        client_builder = client_builder.resolve(&host, addr);
    }
    let client = match client_builder.build() {
        Ok(c) => c,
        Err(_) => return Ok(()),
    };

    let mut req = client.get(validated_url.as_str());
    for (key, value) in headers {
        req = req.header(key, value);
    }

    let resp = match req.send().await {
        Ok(r) => r,
        Err(_) => return Ok(()),
    };

    if !resp.status().is_success() {
        return Ok(());
    }

    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();

    if is_html_content_type(&content_type) {
        return Err(AlephError::IoError(format!(
            "Remote MCP URL '{}' returned an HTML page (Content-Type: {content_type}); it does not \
             appear to be an MCP endpoint — check the URL or transport setting.",
            redact_url_for_error(url),
        )));
    }

    Ok(())
}

/// Render a URL safely for inclusion in a user-visible error message.
///
/// Strips the userinfo (`user:pass@`) and any query string so a misconfigured
/// URL that carries a long-lived bearer (`https://user:token@host/mcp`) does
/// not leak the credential into the agent's reply or the operator's log. The
/// full URL is still available to the structured `tracing` layer via the
/// surrounding `preflight_remote_url` caller's own logging.
fn redact_url_for_error(url: &str) -> String {
    let mut s = url.to_string();
    if let Some(scheme_end) = s.find("://") {
        let after_scheme = scheme_end + 3;
        if let Some(at) = s[after_scheme..].find('@') {
            let userinfo_end = after_scheme + at;
            s.replace_range(after_scheme..=userinfo_end, "");
        }
    }
    if let Some(q) = s.find('?') {
        s.truncate(q);
    }
    s
}

/// Whether a `Content-Type` header denotes an HTML web page.
fn is_html_content_type(content_type: &str) -> bool {
    content_type.starts_with("text/html") || content_type.starts_with("application/xhtml+xml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_html_content_types() {
        assert!(is_html_content_type("text/html"));
        assert!(is_html_content_type("text/html; charset=utf-8"));
        assert!(is_html_content_type("application/xhtml+xml"));
    }

    #[test]
    fn does_not_classify_mcp_content_types() {
        assert!(!is_html_content_type("application/json"));
        assert!(!is_html_content_type("text/event-stream"));
        assert!(!is_html_content_type(""));
        assert!(!is_html_content_type("application/json-rpc"));
    }

    #[test]
    fn redact_url_for_error_strips_userinfo_and_query() {
        // A userinfo carrier must not appear in the user-visible error.
        assert_eq!(
            redact_url_for_error("https://user:token@host.example/mcp"),
            "https://host.example/mcp"
        );
        // Query strings (session ids, tokens) are stripped too.
        assert_eq!(
            redact_url_for_error("https://host.example/mcp?sessionId=secret"),
            "https://host.example/mcp"
        );
        // A URL with neither is returned unchanged.
        assert_eq!(
            redact_url_for_error("https://host.example/mcp"),
            "https://host.example/mcp"
        );
        // A URL with no scheme/path is left intact.
        assert_eq!(redact_url_for_error("host.example"), "host.example");
    }

    #[tokio::test]
    async fn ssrf_blocks_loopback_before_probe() {
        // SSRF policy must reject a private/loopback target up front, regardless
        // of whether anything is listening there.
        let headers = HashMap::new();
        let result = preflight_remote_url("http://127.0.0.1:1/mcp", &headers).await;
        assert!(result.is_err());
    }
}
