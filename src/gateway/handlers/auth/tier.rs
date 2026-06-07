//! Pairing tier ↔ permissions/role mapping (single source of truth).
//!
//! A paired device is either the default **chat** tier (chat + read-only
//! dashboards, no Aleph-config rights) or the explicit **config** tier
//! (operator, full control plane). The tier is persisted purely as the
//! device's `permissions`: a `"*"` wildcard means config/operator, anything
//! else is chat. `role_for_permissions` derives the connect-response role
//! string that the dispatch loop feeds into the method-authz gate.

/// Wildcard permission that marks a full-access (operator/config) device.
pub const WILDCARD: &str = "*";

/// Permissions granted to a chat-tier device: converse + read.
pub const CHAT_PERMISSIONS: &[&str] = &["chat", "read"];

/// Pairing approval tier requested by the operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Default: chat + read-only dashboards, no config rights.
    Chat,
    /// Operator: full control plane.
    Config,
}

impl Tier {
    /// Parse the `level` pairing param. Unknown / missing → `Chat` (safe default).
    pub fn from_level(level: Option<&str>) -> Self {
        match level {
            Some(s) if s.eq_ignore_ascii_case("config") => Tier::Config,
            _ => Tier::Chat,
        }
    }

    /// The permission set persisted for a device approved at this tier.
    pub fn permissions(self) -> Vec<String> {
        match self {
            Tier::Config => vec![WILDCARD.to_string()],
            Tier::Chat => CHAT_PERMISSIONS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

/// Connect-response role string for a connection holding `permissions`.
/// `"operator"` iff the wildcard is present, else `"guest"` (chat tier).
/// This is the string the dispatch loop stores in `ConnectionState.role`
/// and `is_operator()` checks.
pub fn role_for_permissions(permissions: &[String]) -> &'static str {
    if permissions.iter().any(|p| p == WILDCARD) {
        "operator"
    } else {
        "guest"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_defaults_to_chat() {
        assert_eq!(Tier::from_level(None), Tier::Chat);
        assert_eq!(Tier::from_level(Some("")), Tier::Chat);
        assert_eq!(Tier::from_level(Some("bogus")), Tier::Chat);
        assert_eq!(Tier::from_level(Some("chat")), Tier::Chat);
    }

    #[test]
    fn level_config_is_explicit_and_case_insensitive() {
        assert_eq!(Tier::from_level(Some("config")), Tier::Config);
        assert_eq!(Tier::from_level(Some("CONFIG")), Tier::Config);
    }

    #[test]
    fn config_tier_is_wildcard_operator() {
        let perms = Tier::Config.permissions();
        assert_eq!(perms, vec!["*".to_string()]);
        assert_eq!(role_for_permissions(&perms), "operator");
    }

    #[test]
    fn chat_tier_is_non_operator() {
        let perms = Tier::Chat.permissions();
        assert_eq!(perms, vec!["chat".to_string(), "read".to_string()]);
        assert_eq!(role_for_permissions(&perms), "guest");
    }

    #[test]
    fn empty_permissions_are_non_operator() {
        assert_eq!(role_for_permissions(&[]), "guest");
    }
}
