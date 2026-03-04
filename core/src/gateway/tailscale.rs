//! Tailscale integration for zero-config mesh network authentication.
//!
//! When Aleph runs behind a Tailscale proxy (e.g. `tailscale serve`),
//! incoming requests carry identity headers injected by the Tailscale daemon.
//! This module extracts those headers and validates that the peer IP
//! belongs to the Tailscale CGNAT range (100.64.0.0/10).

use serde::Serialize;

/// Identity information extracted from Tailscale proxy headers.
#[derive(Debug, Clone, Serialize)]
pub struct TailscaleIdentity {
    /// Tailscale user login (typically an email address).
    pub login: String,
    /// Human-readable display name of the Tailscale user.
    pub display_name: String,
    /// Peer IP address from X-Forwarded-For.
    pub peer_ip: String,
}

impl TailscaleIdentity {
    /// Extract Tailscale identity from HTTP headers.
    ///
    /// Returns `None` if any of the required headers are missing or empty:
    /// - `Tailscale-User-Login`
    /// - `Tailscale-User-Name`
    /// - `X-Forwarded-For`
    pub fn from_headers(headers: &axum::http::HeaderMap) -> Option<Self> {
        let login = header_non_empty(headers, "Tailscale-User-Login")?;
        let display_name = header_non_empty(headers, "Tailscale-User-Name")?;
        let peer_ip = header_non_empty(headers, "X-Forwarded-For")?;

        Some(Self {
            login,
            display_name,
            peer_ip,
        })
    }
}

/// Extract a header value as a non-empty trimmed `String`.
fn header_non_empty(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    let value = headers.get(name)?.to_str().ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

/// Check whether an IP address falls within the Tailscale CGNAT range.
///
/// Tailscale assigns addresses in the `100.64.0.0/10` block, which spans
/// `100.64.0.0` through `100.127.255.255`. Only IPv4 dotted-decimal strings
/// are accepted; anything else returns `false`.
pub fn is_tailscale_ip(ip: &str) -> bool {
    let parts: Vec<&str> = ip.split('.').collect();
    if parts.len() != 4 {
        return false;
    }

    let first: u8 = match parts[0].parse() {
        Ok(v) => v,
        Err(_) => return false,
    };
    let second: u8 = match parts[1].parse() {
        Ok(v) => v,
        Err(_) => return false,
    };

    // Validate remaining octets are valid u8 values.
    for part in &parts[2..] {
        if part.parse::<u8>().is_err() {
            return false;
        }
    }

    // 100.64.0.0/10 means first octet == 100, second octet in [64, 127].
    first == 100 && (64..=127).contains(&second)
}

/// Configuration for Tailscale integration.
#[derive(Debug, Clone)]
pub struct TailscaleConfig {
    /// Whether Tailscale authentication is enabled.
    pub enabled: bool,
    /// Path to the tailscaled Unix domain socket.
    pub socket_path: Option<String>,
}

impl Default for TailscaleConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            socket_path: Some("/var/run/tailscale/tailscaled.sock".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    #[test]
    fn test_extract_tailscale_identity_from_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("Tailscale-User-Login", "alice@example.com".parse().unwrap());
        headers.insert("Tailscale-User-Name", "Alice".parse().unwrap());
        headers.insert("X-Forwarded-For", "100.100.1.2".parse().unwrap());

        let id = TailscaleIdentity::from_headers(&headers).expect("should extract identity");
        assert_eq!(id.login, "alice@example.com");
        assert_eq!(id.display_name, "Alice");
        assert_eq!(id.peer_ip, "100.100.1.2");
    }

    #[test]
    fn test_missing_headers_returns_none() {
        let headers = HeaderMap::new();
        assert!(TailscaleIdentity::from_headers(&headers).is_none());
    }

    #[test]
    fn test_partial_headers_returns_none() {
        let mut headers = HeaderMap::new();
        headers.insert("Tailscale-User-Login", "alice@example.com".parse().unwrap());
        // Missing Tailscale-User-Name and X-Forwarded-For
        assert!(TailscaleIdentity::from_headers(&headers).is_none());
    }

    #[test]
    fn test_empty_header_value_returns_none() {
        let mut headers = HeaderMap::new();
        headers.insert("Tailscale-User-Login", "alice@example.com".parse().unwrap());
        headers.insert("Tailscale-User-Name", "".parse().unwrap());
        headers.insert("X-Forwarded-For", "100.100.1.2".parse().unwrap());

        assert!(TailscaleIdentity::from_headers(&headers).is_none());
    }

    #[test]
    fn test_is_tailscale_ip() {
        // Within 100.64.0.0/10
        assert!(is_tailscale_ip("100.64.0.1"));
        assert!(is_tailscale_ip("100.127.255.254"));
        assert!(is_tailscale_ip("100.100.50.25"));
        assert!(is_tailscale_ip("100.64.0.0"));
        assert!(is_tailscale_ip("100.127.255.255"));

        // Outside 100.64.0.0/10
        assert!(!is_tailscale_ip("192.168.1.1"));
        assert!(!is_tailscale_ip("10.0.0.1"));
        assert!(!is_tailscale_ip("100.63.255.255")); // second octet too low
        assert!(!is_tailscale_ip("100.128.0.0"));    // second octet too high

        // Invalid formats
        assert!(!is_tailscale_ip("not-an-ip"));
        assert!(!is_tailscale_ip("100.64.0"));
        assert!(!is_tailscale_ip(""));
    }

    #[test]
    fn test_tailscale_config_default() {
        let config = TailscaleConfig::default();
        assert!(!config.enabled);
        assert_eq!(
            config.socket_path.as_deref(),
            Some("/var/run/tailscale/tailscaled.sock")
        );
    }
}
