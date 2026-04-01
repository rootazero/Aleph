//! Tool permission configuration for global and per-agent permission levels.
//!
//! Provides a two-layer permission system: Global (Policies) + Agent-level.
//! Each tool can be set to Allow, Ask (needs confirmation), or Deny.
//! When merging global and agent permissions, the most restrictive level wins.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::extension::PermissionAction;

/// Tool permissions configuration.
///
/// Controls per-tool permission levels with a default fallback.
/// Used at both the global policy level and the per-agent level.
///
/// # Example TOML
/// ```toml
/// [policies.tool_permissions]
/// default = "allow"
///
/// [policies.tool_permissions.overrides]
/// shell = "ask"
/// file_delete = "deny"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolPermissionsConfig {
    /// Default permission for tools not listed in overrides
    #[serde(default = "default_allow")]
    pub default: PermissionAction,

    /// Per-tool permission overrides
    #[serde(default)]
    pub overrides: HashMap<String, PermissionAction>,
}

fn default_allow() -> PermissionAction {
    PermissionAction::Allow
}

impl Default for ToolPermissionsConfig {
    fn default() -> Self {
        Self {
            default: PermissionAction::Allow,
            overrides: HashMap::new(),
        }
    }
}

impl ToolPermissionsConfig {
    /// Resolve the effective permission for a tool.
    ///
    /// Returns the override if present, otherwise the default.
    pub fn resolve(&self, tool_name: &str) -> PermissionAction {
        self.overrides
            .get(tool_name)
            .copied()
            .unwrap_or(self.default)
    }

    /// Merge global and agent-level permissions into an effective config.
    ///
    /// For each tool, the most restrictive permission wins:
    /// Deny > Ask > Allow (i.e., `min(global, agent)`).
    ///
    /// The effective default is `min(global.default, agent.default)`.
    /// Overrides are merged: if both layers specify a tool, the most
    /// restrictive level is used.
    pub fn merge(
        global: &ToolPermissionsConfig,
        agent: &ToolPermissionsConfig,
    ) -> ToolPermissionsConfig {
        let default = restrictive_min(global.default, agent.default);

        let mut overrides = HashMap::new();

        // Collect all tool names from both layers
        let all_keys: std::collections::HashSet<&String> = global
            .overrides
            .keys()
            .chain(agent.overrides.keys())
            .collect();

        for key in all_keys {
            let global_perm = global.resolve(key);
            let agent_perm = agent.resolve(key);
            let effective = restrictive_min(global_perm, agent_perm);
            // Only store as override if it differs from the merged default
            if effective != default {
                overrides.insert(key.clone(), effective);
            }
        }

        ToolPermissionsConfig { default, overrides }
    }
}

/// Return the more restrictive of two permission actions.
///
/// Ordering: Deny (most restrictive) > Ask > Allow (least restrictive).
fn restrictive_min(a: PermissionAction, b: PermissionAction) -> PermissionAction {
    use PermissionAction::*;
    match (a, b) {
        (Deny, _) | (_, Deny) => Deny,
        (Ask, _) | (_, Ask) => Ask,
        _ => Allow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::PermissionAction;

    #[test]
    fn test_default_config() {
        let config = ToolPermissionsConfig::default();
        assert_eq!(config.default, PermissionAction::Allow);
        assert!(config.overrides.is_empty());
    }

    #[test]
    fn test_resolve_with_override() {
        let mut config = ToolPermissionsConfig::default();
        config
            .overrides
            .insert("shell".to_string(), PermissionAction::Ask);

        assert_eq!(config.resolve("shell"), PermissionAction::Ask);
        assert_eq!(config.resolve("read_file"), PermissionAction::Allow);
    }

    #[test]
    fn test_resolve_uses_default() {
        let config = ToolPermissionsConfig {
            default: PermissionAction::Deny,
            overrides: HashMap::new(),
        };
        assert_eq!(config.resolve("anything"), PermissionAction::Deny);
    }

    #[test]
    fn test_merge_defaults_most_restrictive() {
        let global = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: HashMap::new(),
        };
        let agent = ToolPermissionsConfig {
            default: PermissionAction::Ask,
            overrides: HashMap::new(),
        };
        let merged = ToolPermissionsConfig::merge(&global, &agent);
        assert_eq!(merged.default, PermissionAction::Ask);
    }

    #[test]
    fn test_merge_deny_wins_over_allow() {
        let global = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [("shell".to_string(), PermissionAction::Deny)]
                .into_iter()
                .collect(),
        };
        let agent = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [("shell".to_string(), PermissionAction::Allow)]
                .into_iter()
                .collect(),
        };
        let merged = ToolPermissionsConfig::merge(&global, &agent);
        assert_eq!(merged.resolve("shell"), PermissionAction::Deny);
    }

    #[test]
    fn test_merge_agent_deny_overrides_global_allow() {
        let global = ToolPermissionsConfig::default();
        let agent = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [("file_delete".to_string(), PermissionAction::Deny)]
                .into_iter()
                .collect(),
        };
        let merged = ToolPermissionsConfig::merge(&global, &agent);
        assert_eq!(merged.resolve("file_delete"), PermissionAction::Deny);
        assert_eq!(merged.resolve("read_file"), PermissionAction::Allow);
    }

    #[test]
    fn test_merge_both_overrides_combined() {
        let global = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [("shell".to_string(), PermissionAction::Ask)]
                .into_iter()
                .collect(),
        };
        let agent = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [("file_delete".to_string(), PermissionAction::Deny)]
                .into_iter()
                .collect(),
        };
        let merged = ToolPermissionsConfig::merge(&global, &agent);
        assert_eq!(merged.resolve("shell"), PermissionAction::Ask);
        assert_eq!(merged.resolve("file_delete"), PermissionAction::Deny);
        assert_eq!(merged.resolve("read_file"), PermissionAction::Allow);
    }

    #[test]
    fn test_merge_symmetric_deny() {
        // Deny from either side should produce Deny
        for (g, a) in [
            (PermissionAction::Deny, PermissionAction::Allow),
            (PermissionAction::Allow, PermissionAction::Deny),
            (PermissionAction::Ask, PermissionAction::Deny),
            (PermissionAction::Deny, PermissionAction::Ask),
        ] {
            let global = ToolPermissionsConfig {
                default: g,
                overrides: HashMap::new(),
            };
            let agent = ToolPermissionsConfig {
                default: a,
                overrides: HashMap::new(),
            };
            let merged = ToolPermissionsConfig::merge(&global, &agent);
            assert_eq!(
                merged.default,
                PermissionAction::Deny,
                "merge({:?}, {:?}) should be Deny",
                g,
                a
            );
        }
    }

    #[test]
    fn test_deserialize_empty_toml() {
        let config: ToolPermissionsConfig = toml::from_str("").unwrap();
        assert_eq!(config.default, PermissionAction::Allow);
        assert!(config.overrides.is_empty());
    }

    #[test]
    fn test_deserialize_with_overrides() {
        let toml_str = r#"
            default = "ask"

            [overrides]
            shell = "deny"
            read_file = "allow"
        "#;
        let config: ToolPermissionsConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.default, PermissionAction::Ask);
        assert_eq!(config.overrides.get("shell"), Some(&PermissionAction::Deny));
        assert_eq!(
            config.overrides.get("read_file"),
            Some(&PermissionAction::Allow)
        );
    }

    #[test]
    fn test_serialization_roundtrip() {
        let config = ToolPermissionsConfig {
            default: PermissionAction::Ask,
            overrides: [
                ("shell".to_string(), PermissionAction::Deny),
                ("read_file".to_string(), PermissionAction::Allow),
            ]
            .into_iter()
            .collect(),
        };
        let toml_str = toml::to_string(&config).unwrap();
        let parsed: ToolPermissionsConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.default, config.default);
        assert_eq!(parsed.overrides.len(), config.overrides.len());
    }
}
