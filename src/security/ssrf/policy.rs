//! SSRF policy configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Policy controlling SSRF validation behavior.
///
/// Every field carries a serde default (matching [`SsrfPolicy::default`]) so a
/// partially-populated `[ssrf]` TOML table — e.g. one that only sets
/// `allowed_hosts` — still deserializes instead of erroring on the missing
/// siblings. This keeps hand-edited and Panel-written configs robust.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SsrfPolicy {
    /// Master switch for SSRF protection.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Whether to allow requests to private/internal IP ranges.
    /// Even when true, loopback and cloud metadata endpoints remain blocked.
    #[serde(default)]
    pub allow_private_network: bool,
    /// Hosts that bypass the blocklist. Supports exact matches
    /// (e.g., "api.example.com") and wildcard subdomain matches (e.g., "*.example.com").
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    /// Hosts that are always blocked. Same matching rules as allowed_hosts.
    #[serde(default)]
    pub blocked_hosts: Vec<String>,
    /// Maximum number of HTTP redirects to follow.
    #[serde(default = "default_max_redirects")]
    pub max_redirects: u8,
    /// Strip Authorization/Cookie headers on cross-origin redirects.
    #[serde(default = "default_strip_auth")]
    pub strip_auth_on_cross_origin: bool,
}

const fn default_enabled() -> bool {
    true
}

const fn default_max_redirects() -> u8 {
    5
}

const fn default_strip_auth() -> bool {
    true
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
    #[must_use]
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
