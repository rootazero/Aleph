//! Cross-origin policy for the gateway's browser-facing surface.
//!
//! Browsers attach an `Origin` header to every WebSocket upgrade and every
//! cross-origin `fetch`/XHR, and a web page **cannot forge** this header. A
//! malicious site the user happens to visit can nonetheless *reach*
//! `ws://127.0.0.1:18790/ws` (loopback is not firewalled from the browser),
//! so without an origin check it could open an authenticated control channel
//! to the local daemon — the classic DNS-rebinding / cross-origin-WebSocket
//! confused-deputy. This type centralises the allow/deny decision so the WS
//! upgrade handler and the static-asset CORS layer share one implementation.
//!
//! Decision (`is_allowed`):
//!   * **No `Origin` header** → allow. Native clients (CLI, bots, bridges,
//!     `tokio-tungstenite`) send none; only browsers do. Absence therefore
//!     means a same-host non-browser caller, which loopback/auth already gate.
//!   * **Exact allow-list match** → allow. Operator-configured extra origins
//!     (`[gateway.auth] allowed_origins`) for split panel/API deployments.
//!   * **`tauri:` scheme** → allow. The desktop shell's own webview origin,
//!     unspoofable by a remote page.
//!   * **Loopback host** (`127.0.0.0/8`, `::1`, `localhost`, `*.localhost`) →
//!     allow. Same-machine UI.
//!   * **Same-origin** (the `Origin` authority equals the request `Host`) →
//!     allow. Keeps remote deployments served from their own domain working
//!     without configuration — this is the documented "additional to
//!     same-origin" contract of `allowed_origins`.
//!   * Anything else → deny.

use axum::http::Uri;

/// Browser-origin allow/deny policy. Cheap to clone (one `Vec<String>`), and
/// `Send + Sync` so it can live inside a CORS predicate closure or
/// `GatewaySharedState`.
#[derive(Debug, Clone, Default)]
pub struct OriginPolicy {
    /// Operator-configured extra origins, compared verbatim (case-insensitive)
    /// against the raw `Origin` header. Empty ⇒ only same-origin / loopback /
    /// `tauri:` are accepted.
    allowed: Vec<String>,
}

impl OriginPolicy {
    /// Build from the configured extra-origin allow-list.
    #[must_use]
    pub const fn new(allowed: Vec<String>) -> Self {
        Self { allowed }
    }

    /// Policy with no extra origins — same-origin, loopback and `tauri:` only.
    #[must_use]
    pub const fn loopback_only() -> Self {
        Self {
            allowed: Vec::new(),
        }
    }

    /// Decide whether a request carrying `origin` (the raw `Origin` header) and
    /// `host` (the raw `Host` header) may proceed.
    ///
    /// `host` is `None` for callers that have no notion of same-origin (the
    /// static-asset CORS layer), in which case only loopback / `tauri:` /
    /// allow-list pass.
    #[must_use]
    pub fn is_allowed(&self, origin: Option<&str>, host: Option<&str>) -> bool {
        let Some(origin) = origin else {
            // Non-browser client: no Origin header. Browsers always send one.
            return true;
        };
        let origin = origin.trim();
        if origin.is_empty() {
            // Some agents send a literal empty Origin; treat as non-browser.
            return true;
        }

        // Operator allow-list: exact, case-insensitive match on the full origin.
        if self
            .allowed
            .iter()
            .any(|a| a.trim().eq_ignore_ascii_case(origin))
        {
            return true;
        }

        // Parse to inspect scheme/host robustly. A bare prefix match would
        // accept hostile origins like `http://127.0.0.1.evil.com`.
        let Ok(uri) = origin.parse::<Uri>() else {
            return false;
        };

        match uri.scheme_str() {
            Some("http") | Some("https") => {}
            // The desktop shell webview's own origin. Unspoofable by a remote
            // page and only present for in-process requests.
            Some("tauri") => return true,
            _ => return false,
        }

        let origin_host = uri.host().unwrap_or("");
        if is_loopback_host(origin_host) {
            return true;
        }

        // Same-origin: the Origin's authority (host[:port]) equals the request
        // Host header. This is the DNS-rebinding defence — a page at
        // evil.com hitting a victim gateway carries `Origin: …evil.com` which
        // never matches the gateway's own Host.
        if let Some(host) = host {
            if let Some(authority) = uri.authority() {
                if authority.as_str().eq_ignore_ascii_case(host.trim()) {
                    return true;
                }
            }
        }

        false
    }
}

/// Whether `host` (a bare host, possibly bracketed IPv6) resolves to loopback.
/// `*.localhost` is reserved (RFC 6761) and always loopback, so it is accepted
/// without a DNS lookup.
fn is_loopback_host(host: &str) -> bool {
    let h = host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(host);
    if h.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let lower = h.to_ascii_lowercase();
    if lower.ends_with(".localhost") {
        return true;
    }
    h.parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_origin_is_allowed() {
        let p = OriginPolicy::loopback_only();
        assert!(p.is_allowed(None, Some("aleph.example.com")));
        assert!(p.is_allowed(Some("  "), None));
    }

    #[test]
    fn loopback_origins_allowed() {
        let p = OriginPolicy::loopback_only();
        assert!(p.is_allowed(Some("http://127.0.0.1:18790"), None));
        assert!(p.is_allowed(Some("http://localhost:3000"), None));
        assert!(p.is_allowed(Some("http://[::1]:18790"), None));
        assert!(p.is_allowed(Some("http://panel.localhost"), None));
    }

    #[test]
    fn tauri_scheme_allowed() {
        let p = OriginPolicy::loopback_only();
        assert!(p.is_allowed(Some("tauri://localhost"), None));
    }

    #[test]
    fn same_origin_allowed_without_config() {
        let p = OriginPolicy::loopback_only();
        // Remote deployment served from its own domain: Origin authority
        // equals the request Host → allowed, no config needed.
        assert!(p.is_allowed(Some("https://aleph.example.com"), Some("aleph.example.com")));
        assert!(p.is_allowed(
            Some("https://aleph.example.com:8443"),
            Some("aleph.example.com:8443")
        ));
    }

    #[test]
    fn cross_origin_browser_rejected() {
        let p = OriginPolicy::loopback_only();
        // Malicious page hitting the local daemon.
        assert!(!p.is_allowed(Some("https://evil.com"), Some("127.0.0.1:18790")));
        // DNS-rebinding lookalike must not pass the loopback test.
        assert!(!p.is_allowed(Some("http://127.0.0.1.evil.com"), None));
        // Origin host differs from request Host → not same-origin.
        assert!(!p.is_allowed(Some("https://evil.com"), Some("aleph.example.com")));
    }

    #[test]
    fn explicit_allowlist_match() {
        let p = OriginPolicy::new(vec!["https://panel.example.com".to_string()]);
        assert!(p.is_allowed(Some("https://panel.example.com"), Some("api.example.com")));
        assert!(p.is_allowed(Some("HTTPS://PANEL.EXAMPLE.COM"), None));
        assert!(!p.is_allowed(Some("https://other.example.com"), Some("api.example.com")));
    }

    #[test]
    fn malformed_origin_rejected() {
        let p = OriginPolicy::loopback_only();
        assert!(!p.is_allowed(Some("not a uri"), Some("127.0.0.1")));
        assert!(!p.is_allowed(Some("ftp://127.0.0.1"), None));
    }
}
