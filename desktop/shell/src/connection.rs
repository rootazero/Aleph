//! Connection target: local daemon vs remote Gateway.
//!
//! The shell connects to exactly one Gateway at a time — either the
//! same-machine `aleph-server` it launches and supervises (Local, the
//! default and today's behaviour), or a remote Gateway by URL (Remote, which
//! never touches the local daemon). The choice persists in
//! `~/.aleph/.desktop-shell-target`; a missing file means Local (zero
//! regression on first run).

use url::Url;

/// Default Gateway port when the user omits one.
const DEFAULT_PORT: u16 = 18790;

/// Where the chosen target persists. Mirrors the sibling
/// `.desktop-shell-autostart` / `.desktop-shell-daemon-version` markers.
fn target_marker() -> Option<std::path::PathBuf> {
    dirs::home_dir().map(|h| h.join(".aleph/.desktop-shell-target"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionTarget {
    /// Launch + supervise the local daemon; webview → 127.0.0.1:18790.
    Local,
    /// Connect to a remote Gateway by origin; never touch the local daemon.
    Remote(Url),
}

impl ConnectionTarget {
    pub fn is_local(&self) -> bool {
        matches!(self, ConnectionTarget::Local)
    }

    /// Parse a persisted/user-entered target string. `"local"` (any case) or
    /// empty → Local. Otherwise normalise to a `Remote(Url)`:
    /// accept `host`, `host:port`, `http://host`, `https://host:port`;
    /// default scheme `http`, default port 18790.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let t = raw.trim();
        if t.is_empty() || t.eq_ignore_ascii_case("local") {
            return Ok(ConnectionTarget::Local);
        }
        let with_scheme = if t.contains("://") {
            t.to_string()
        } else {
            format!("http://{t}")
        };
        let mut url = Url::parse(&with_scheme).map_err(|e| format!("invalid target URL: {e}"))?;
        match url.scheme() {
            "http" | "https" => {}
            other => return Err(format!("unsupported scheme: {other}")),
        }
        if url.host().is_none() {
            return Err("target URL has no host".to_string());
        }
        // Apply the default port only when the user did not supply one explicitly.
        // `url::Url::port()` returns None both for "no port written" and for "port
        // equals the scheme default" (e.g. https:443, http:80) — the two cases are
        // indistinguishable after parsing.  We therefore inspect the pre-parse
        // string: if it already contains ":<digits>" after the host, the user made
        // an explicit choice and we honour it; otherwise we inject DEFAULT_PORT.
        let has_explicit_port = has_explicit_port_in_input(t);
        if !has_explicit_port {
            // set_port only errors when the URL cannot have a port (it can here)
            let _ = url.set_port(Some(DEFAULT_PORT));
        }
        Ok(ConnectionTarget::Remote(url))
    }

    /// Serialise for persistence. Local → `"local"`; Remote → the URL origin
    /// with an explicit port (so that `load_target` round-trips correctly even
    /// when the port equals the scheme default and would otherwise be elided by
    /// the `url` crate's normalisation).
    pub fn to_persisted(&self) -> String {
        match self {
            ConnectionTarget::Local => "local".to_string(),
            ConnectionTarget::Remote(url) => {
                let scheme = url.scheme();
                let host = url.host_str().unwrap_or("127.0.0.1");
                // `url::Url::port()` returns None for scheme-default ports (443 for
                // https, 80 for http); use `port_or_known_default()` to recover them.
                let port = url.port_or_known_default().unwrap_or(DEFAULT_PORT);
                format!("{scheme}://{host}:{port}")
            }
        }
    }
}

/// Detect whether the raw user input already contains an explicit port number.
/// Handles forms: `host:port`, `http://host:port`, `https://host:port`,
/// including IPv6 (`[::1]:port`).  Returns false when only a scheme default
/// would apply (e.g. `https://host` with no written port).
fn has_explicit_port_in_input(raw: &str) -> bool {
    // Strip scheme if present so we work on `[host]/path` or `host:port/path`.
    let after_scheme = if let Some(pos) = raw.find("://") {
        &raw[pos + 3..]
    } else {
        raw
    };
    // For IPv6 addresses the host is wrapped in brackets: `[::1]:port`.
    let host_end = if after_scheme.starts_with('[') {
        after_scheme.find(']').map(|i| i + 1)
    } else {
        // Plain hostname or IPv4 — find the first `:`.
        after_scheme.find(':')
    };
    match host_end {
        // IPv6: `idx` is already one past `]` (i.e. pointing at `:` when a
        // port is present), so check for `:` directly at `idx`.
        Some(idx) if after_scheme.starts_with('[') => {
            after_scheme[idx..].starts_with(':')
                && after_scheme[idx + 1..]
                    .split('/')
                    .next()
                    .is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        }
        // Plain host:port — digits after the `:` (before any `/`).
        Some(colon) => after_scheme[colon + 1..]
            .split('/')
            .next()
            .is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())),
        None => false,
    }
}

/// Load the persisted target; missing/unreadable/unparsable → Local
/// (fail-safe: a corrupt marker must never strand the user on a broken
/// remote — it falls back to the always-available local daemon).
pub fn load_target() -> ConnectionTarget {
    let Some(marker) = target_marker() else {
        return ConnectionTarget::Local;
    };
    match std::fs::read_to_string(&marker) {
        Ok(s) => ConnectionTarget::parse(&s).unwrap_or(ConnectionTarget::Local),
        Err(_) => ConnectionTarget::Local,
    }
}

/// Persist a target string (already validated by `parse`). Writes the
/// normalised form.
pub fn save_target(target: &ConnectionTarget) -> Result<(), String> {
    let Some(marker) = target_marker() else {
        return Err("home directory not found".to_string());
    };
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create .aleph dir: {e}"))?;
    }
    std::fs::write(&marker, target.to_persisted()).map_err(|e| format!("write target: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_local_parse_to_local() {
        assert_eq!(
            ConnectionTarget::parse("").unwrap(),
            ConnectionTarget::Local
        );
        assert_eq!(
            ConnectionTarget::parse("  ").unwrap(),
            ConnectionTarget::Local
        );
        assert_eq!(
            ConnectionTarget::parse("local").unwrap(),
            ConnectionTarget::Local
        );
        assert_eq!(
            ConnectionTarget::parse("LOCAL").unwrap(),
            ConnectionTarget::Local
        );
    }

    #[test]
    fn bare_host_gets_http_and_default_port() {
        let t = ConnectionTarget::parse("192.168.1.5").unwrap();
        assert_eq!(t.to_persisted(), "http://192.168.1.5:18790");
    }

    #[test]
    fn host_port_gets_http() {
        let t = ConnectionTarget::parse("box.lan:9000").unwrap();
        assert_eq!(t.to_persisted(), "http://box.lan:9000");
    }

    #[test]
    fn explicit_scheme_preserved() {
        let t = ConnectionTarget::parse("https://gw.example.com").unwrap();
        assert_eq!(t.to_persisted(), "https://gw.example.com:18790");
        let t2 = ConnectionTarget::parse("https://gw.example.com:443").unwrap();
        assert_eq!(t2.to_persisted(), "https://gw.example.com:443");
    }

    #[test]
    fn unsupported_scheme_rejected() {
        assert!(ConnectionTarget::parse("ftp://host").is_err());
        assert!(ConnectionTarget::parse("ws://host").is_err());
    }

    #[test]
    fn is_local_flag() {
        assert!(ConnectionTarget::Local.is_local());
        assert!(!ConnectionTarget::parse("10.0.0.1").unwrap().is_local());
    }

    #[test]
    fn ipv6_with_port_keeps_user_port() {
        let t = ConnectionTarget::parse("http://[::1]:9000").unwrap();
        assert_eq!(t.to_persisted(), "http://[::1]:9000");
    }

    #[test]
    fn ipv6_without_port_gets_default_port() {
        let t = ConnectionTarget::parse("http://[::1]").unwrap();
        assert_eq!(t.to_persisted(), "http://[::1]:18790");
    }
}
