//! Plugin owner trust policy — which origins may load a plugin at all.
//!
//! Mirrors openclaw's `passesManifestOwnerBasePolicy`. [`OwnerTrustPolicy`] is
//! consulted once per plugin directory in
//! [`ExtensionManager::load_plugins`](crate::extension::ExtensionManager), and a
//! `false` verdict is what keeps a stray `~/.aleph/plugins/installed/foo` from
//! silently registering its capabilities.
//!
//! ## What used to live here
//!
//! This file was `activation.rs` and carried openclaw's `activation-planner.ts`
//! port alongside the trust policy: `ActivationPlanner` plus `ActivationHints`,
//! `ActivationTrigger`, `TriggerKind`, `CapabilityKind`, `ActivationPlan`,
//! `ActivationReason`, `PlanInput`/`PlanRecord` and `tier_kinds` — roughly 600
//! lines whose only callers were their own tests.
//!
//! It was removed under R10's YAGNI clause, and the deletion was wider than
//! "the planner has no caller". `PluginManifest.activation` was never non-`None`
//! either: all three manifest adapters (`cc_plugin_json`, `cc_plugin_toml`,
//! `toml_types`) hardcoded `activation: None` at their construction sites, and
//! `cc_plugin_json` went as far as deserializing the block into its own DTO
//! before dropping it on the way out. So a plugin author who wrote an
//! `activation` block got no lazy loading *and* no diagnostic — the hints never
//! reached a planner that in turn had nobody to answer.
//!
//! Reconnecting it was not a matter of adding a call site. Lazy activation needs
//! a load path that is re-entrant on a trigger, and `load_plugins` is a
//! one-shot boot-time walk over plugin directories; that path would have had to
//! be built. Anyone reviving this should start from openclaw's
//! `activation-planner.ts` and the git history of this file
//! (`git log --follow src/extension/plugin_trust.rs`) rather than the deleted
//! Rust, which was never exercised against a real registry.

use std::collections::HashSet;

use crate::extension::types::PluginOrigin;

/// Plugin owner trust policy.
///
/// Mirrors openclaw's `passesManifestOwnerBasePolicy`:
/// - `Bundled` plugins (Aleph built-ins, marketplace `aleph-official`)
///   are always trusted.
/// - `Config` plugins (operator added to `aleph.jsonc`) are always
///   trusted.
/// - `Workspace` / `Global` plugins (anything in
///   `<project>/.aleph/plugins` or `~/.aleph/plugins/installed/`) require
///   an explicit allowlist entry. This prevents a stray `~/.aleph/extensions/foo`
///   directory from silently activating for any random command.
#[derive(Debug, Clone)]
pub struct OwnerTrustPolicy {
    /// Explicit allowlist of plugin ids that may load from non-trusted
    /// origins. Built by operators via `plugin trust <id>` or
    /// `~/.aleph/trusted-plugins.toml`. Empty allowlist + `restrictive()`
    /// = nothing from `Workspace`/`Global` may load.
    allowlist: HashSet<String>,
    /// When `true`, [`Self::allows`] enforces the allowlist for `Workspace`
    /// and `Global` origins. When `false`, every plugin passes (the legacy
    /// "load everything" default).
    enforce: bool,
}

impl OwnerTrustPolicy {
    /// Permissive policy: every plugin passes (legacy behaviour, used while
    /// operators haven't yet curated an allowlist).
    #[must_use]
    pub fn permissive() -> Self {
        Self {
            allowlist: HashSet::new(),
            enforce: false,
        }
    }

    /// Restrictive policy: only `Bundled` + `Config` origins + explicitly
    /// allowlisted `Workspace`/`Global` plugins pass. Use this when an
    /// operator has curated a trust list and wants Aleph to fail-loud on
    /// unknown sources.
    #[must_use]
    pub fn restrictive(allowlist: impl IntoIterator<Item = String>) -> Self {
        Self {
            allowlist: allowlist.into_iter().collect(),
            enforce: true,
        }
    }

    /// Add a plugin id to the allowlist. Used by `plugin trust <id>`.
    pub fn trust(&mut self, plugin_id: impl Into<String>) {
        self.allowlist.insert(plugin_id.into());
    }

    /// Remove a plugin id from the allowlist.
    pub fn untrust(&mut self, plugin_id: &str) -> bool {
        self.allowlist.remove(plugin_id)
    }

    /// Snapshot the current allowlist (sorted).
    #[must_use]
    pub fn allowlist(&self) -> Vec<String> {
        let mut list: Vec<String> = self.allowlist.iter().cloned().collect();
        list.sort();
        list
    }

    /// Whether the given plugin id may load under this policy.
    #[must_use]
    pub fn allows(&self, plugin_id: &str, origin: PluginOrigin) -> bool {
        if !self.enforce {
            return true;
        }
        match origin {
            // Bundled (built-in plugins) and Config (operator-explicit) are
            // always trusted — the operator put them there on purpose.
            PluginOrigin::Bundled | PluginOrigin::Config => true,
            PluginOrigin::Workspace | PluginOrigin::Global => self.allowlist.contains(plugin_id),
        }
    }
}

impl Default for OwnerTrustPolicy {
    fn default() -> Self {
        // Default to permissive: matching the historical behaviour where
        // any enabled plugin could load. Operators opt into restrictive
        // mode once they've curated an allowlist.
        Self::permissive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissive_policy_lets_everything_through() {
        let policy = OwnerTrustPolicy::permissive();
        assert!(policy.allows("any", PluginOrigin::Global));
        assert!(policy.allows("any", PluginOrigin::Workspace));
        assert!(policy.allows("any", PluginOrigin::Bundled));
        assert!(policy.allows("any", PluginOrigin::Config));
    }

    #[test]
    fn default_policy_is_permissive() {
        // `ExtensionManager` installs this default at construction, so a
        // deployment that never calls `set_owner_trust_policy` must keep
        // loading every plugin it used to.
        assert!(OwnerTrustPolicy::default().allows("any", PluginOrigin::Global));
    }

    #[test]
    fn restrictive_policy_blocks_untrusted_origins_without_allowlist() {
        let policy = OwnerTrustPolicy::restrictive(Vec::<String>::new());
        assert!(
            !policy.allows("unknown", PluginOrigin::Global),
            "Global origin must be blocked without an allowlist entry"
        );
        assert!(
            !policy.allows("unknown", PluginOrigin::Workspace),
            "Workspace origin must be blocked without an allowlist entry"
        );
    }

    #[test]
    fn allowlisting_readmits_an_untrusted_origin() {
        let mut policy = OwnerTrustPolicy::restrictive(Vec::<String>::new());
        assert!(!policy.allows("unknown", PluginOrigin::Global));
        policy.trust("unknown");
        assert!(policy.allows("unknown", PluginOrigin::Global));
        assert!(
            !policy.allows("other", PluginOrigin::Global),
            "allowlisting is per-id, not a global switch"
        );
    }

    #[test]
    fn bundled_and_config_origins_are_always_trusted_under_restrictive_policy() {
        let policy = OwnerTrustPolicy::restrictive(Vec::<String>::new());
        assert!(policy.allows("builtin", PluginOrigin::Bundled));
        assert!(policy.allows("operator-added", PluginOrigin::Config));
    }

    #[test]
    fn restrictive_seed_allowlist_is_honoured() {
        let policy = OwnerTrustPolicy::restrictive(["seeded".to_string()]);
        assert!(policy.allows("seeded", PluginOrigin::Workspace));
        assert!(!policy.allows("not-seeded", PluginOrigin::Workspace));
    }

    #[test]
    fn owner_trust_policy_allowlist_round_trip() {
        let mut p = OwnerTrustPolicy::permissive();
        assert!(p.allowlist().is_empty());
        p.trust("a");
        p.trust("b");
        assert_eq!(p.allowlist(), vec!["a".to_string(), "b".to_string()]);
        assert!(p.untrust("a"));
        assert_eq!(p.allowlist(), vec!["b".to_string()]);
        assert!(!p.untrust("never-added"));
    }
}
