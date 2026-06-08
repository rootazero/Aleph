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

/// UI-facing tier label for a device holding `permissions`.
/// `"config"` iff the wildcard is present, else `"chat"` (the default tier).
/// Mirrors `role_for_permissions` but yields the label the Panel shows on a
/// device card, decoupled from the connect-response role string.
pub fn tier_for_permissions(permissions: &[String]) -> &'static str {
    if permissions.iter().any(|p| p == WILDCARD) {
        "config"
    } else {
        "chat"
    }
}

/// Default permission tier for a freshly-attached connection, as a function of
/// its surface identity and whether it attaches over loopback (same machine).
///
/// SSOT for the rule that used to live implicitly in the shared-token / no-auth
/// operator grants: a same-machine attach is operator (the OS boundary is the
/// trust boundary); a remote attach defaults to chat and must be raised
/// explicitly (Spec B `devices.set_level`).
///
/// `channel_kind` is carried for identity and future per-kind refinement; today
/// the tier is determined solely by the attach boundary, so every kind maps the
/// same way. Keeping it in the signature makes the rule's shape explicit and
/// gives later phases one place to specialise.
pub fn default_tier(channel_kind: crate::gateway::surface::SurfaceKind, is_loopback: bool) -> Tier {
    let _ = channel_kind; // carried for identity; not yet a tier input (see doc).
    match is_loopback {
        // Same-machine attach: operator. Byte-equivalent to the legacy
        // `vec!["*"]` grant on the shared-token / no-auth loopback paths.
        true => Tier::Config,
        // Remote attach: chat by default (matches pairing's `Tier::from_level`
        // default; an operator raises it via `devices.set_level`).
        false => Tier::Chat,
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

    #[test]
    fn tier_label_config_for_wildcard() {
        assert_eq!(tier_for_permissions(&["*".to_string()]), "config");
    }

    #[test]
    fn tier_label_chat_for_non_wildcard() {
        assert_eq!(
            tier_for_permissions(&["chat".to_string(), "read".to_string()]),
            "chat"
        );
    }

    #[test]
    fn tier_label_chat_for_empty() {
        assert_eq!(tier_for_permissions(&[]), "chat");
    }

    #[test]
    fn loopback_attach_is_config_operator_for_every_kind() {
        use crate::gateway::surface::SurfaceKind;
        for k in [SurfaceKind::Desktop, SurfaceKind::Browser, SurfaceKind::Cli, SurfaceKind::Unknown] {
            assert_eq!(default_tier(k, true), Tier::Config);
            // Byte-equivalent to the legacy `vec!["*"]` loopback operator grant.
            assert_eq!(default_tier(k, true).permissions(), vec!["*".to_string()]);
            assert_eq!(role_for_permissions(&default_tier(k, true).permissions()), "operator");
        }
    }

    #[test]
    fn remote_attach_defaults_to_chat_for_every_kind() {
        use crate::gateway::surface::SurfaceKind;
        for k in [SurfaceKind::Desktop, SurfaceKind::Browser, SurfaceKind::Cli, SurfaceKind::Unknown] {
            assert_eq!(default_tier(k, false), Tier::Chat);
            assert_eq!(role_for_permissions(&default_tier(k, false).permissions()), "guest");
        }
    }
}
