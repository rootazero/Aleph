//! SSRF policy configuration.

use serde::{Deserialize, Serialize};

/// Policy controlling SSRF validation behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SsrfPolicy {
    /// Master switch for SSRF protection.
    pub enabled: bool,
    /// Whether to allow requests to private/internal IP ranges.
    /// Even when true, loopback and cloud metadata endpoints remain blocked.
    pub allow_private_network: bool,
    /// Hosts that bypass the blocklist. Supports exact matches
    /// (e.g., "api.example.com") and wildcard subdomain matches (e.g., "*.example.com").
    pub allowed_hosts: Vec<String>,
    /// Hosts that are always blocked. Same matching rules as allowed_hosts.
    pub blocked_hosts: Vec<String>,
    /// Maximum number of HTTP redirects to follow.
    pub max_redirects: u8,
    /// Strip Authorization/Cookie headers on cross-origin redirects.
    pub strip_auth_on_cross_origin: bool,
}

impl Default for SsrfPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_private_network: false,
            allowed_hosts: Vec::new(),
            blocked_hosts: Vec::new(),
            max_redirects: 5,
            strip_auth_on_cross_origin: true,
        }
    }
}

impl SsrfPolicy {
    /// Returns a policy with SSRF protection disabled (bypass all checks).
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values_are_correct() {
        let policy = SsrfPolicy::default();
        assert!(policy.enabled);
        assert!(!policy.allow_private_network);
        assert!(policy.allowed_hosts.is_empty());
        assert!(policy.blocked_hosts.is_empty());
        assert_eq!(policy.max_redirects, 5);
        assert!(policy.strip_auth_on_cross_origin);
    }

    #[test]
    fn disabled_returns_enabled_false() {
        let policy = SsrfPolicy::disabled();
        assert!(!policy.enabled);
        // Other fields should still have defaults
        assert!(!policy.allow_private_network);
        assert_eq!(policy.max_redirects, 5);
    }

    #[test]
    fn serialization_roundtrip() {
        let policy = SsrfPolicy {
            enabled: true,
            allow_private_network: true,
            allowed_hosts: vec!["*.example.com".to_string()],
            blocked_hosts: vec!["evil.com".to_string()],
            max_redirects: 3,
            strip_auth_on_cross_origin: false,
        };
        let json = serde_json::to_string(&policy).unwrap();
        let restored: SsrfPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.max_redirects, 3);
        assert!(restored.allow_private_network);
        assert!(!restored.strip_auth_on_cross_origin);
    }
}
