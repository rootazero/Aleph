//! Routing Configuration
//!
//! Configuration for message routing, session resolution, and permission policies.

use serde::{Deserialize, Serialize};

/// DM (Direct Message) scope - how to isolate DM sessions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DmScope {
    /// All DMs share the main session
    Main,
    /// Each peer gets their own session (cross-channel)
    #[default]
    PerPeer,
    /// Each peer per channel gets their own session
    PerChannelPeer,
}

/// Configuration for inbound message routing.
///
/// One field, because only one was ever read. `default_agent`,
/// `auto_start_channels` and `pairing_code_expiry_secs` looked operator-settable
/// — serde defaults and all — while nothing anywhere consulted them: the
/// inbound router hardcodes its default agent, and the pairing store uses its
/// own constant. A knob that cannot change anything is worse than a missing one,
/// because it answers the operator's question with a lie. Removing them is
/// backward compatible: serde ignores unknown keys, so an existing config still
/// loads.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoutingConfig {
    /// How to scope DM sessions.
    #[serde(default)]
    pub dm_scope: DmScope,
}

impl RoutingConfig {
    /// Set DM scope.
    #[must_use]
    pub const fn with_dm_scope(mut self, scope: DmScope) -> Self {
        self.dm_scope = scope;
        self
    }
}

/// Bridge the routing-layer `SessionConfig` DM scope (`routing::session_key::DmScope`)
/// into the gateway `RoutingConfig` DM scope used by the zero-config fallback path.
/// The two enums are structurally identical but distinct types; this keeps a single
/// user-facing `[session] dm_scope` value driving both routing paths.
impl From<crate::routing::session_key::DmScope> for DmScope {
    fn from(scope: crate::routing::session_key::DmScope) -> Self {
        use crate::routing::session_key::DmScope as S;
        match scope {
            S::Main => Self::Main,
            S::PerPeer => Self::PerPeer,
            S::PerChannelPeer => Self::PerChannelPeer,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = RoutingConfig::default();
        assert_eq!(config.dm_scope, DmScope::PerPeer);
    }

    #[test]
    fn test_dm_scope_serialization() {
        let json = serde_json::to_string(&DmScope::PerChannelPeer).unwrap();
        assert_eq!(json, "\"per-channel-peer\"");

        let parsed: DmScope = serde_json::from_str("\"main\"").unwrap();
        assert_eq!(parsed, DmScope::Main);
    }

    #[test]
    fn test_config_builder() {
        let config = RoutingConfig::default().with_dm_scope(DmScope::Main);
        assert_eq!(config.dm_scope, DmScope::Main);
    }

    #[test]
    fn unknown_keys_from_an_older_config_still_deserialise() {
        // The three removed knobs may still be sitting in a deployed config.
        // Dropping them must not turn an existing file into a parse error.
        let cfg: RoutingConfig = serde_json::from_str(
            r#"{"default_agent":"main","auto_start_channels":true,"pairing_code_expiry_secs":86400,"dm_scope":"main"}"#,
        )
        .expect("legacy keys are ignored, not rejected");
        assert_eq!(cfg.dm_scope, DmScope::Main);
    }

    #[test]
    fn dm_scope_from_session_config_variant() {
        use crate::routing::session_key::DmScope as Sk;
        assert_eq!(DmScope::from(Sk::Main), DmScope::Main);
        assert_eq!(DmScope::from(Sk::PerPeer), DmScope::PerPeer);
        assert_eq!(DmScope::from(Sk::PerChannelPeer), DmScope::PerChannelPeer);
    }
}
