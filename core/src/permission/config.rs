// Aleph/core/src/permission/config.rs
//! Permission configuration parsing.

use super::rule::{PermissionRule, Ruleset};
use crate::extension::PermissionAction;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Permission configuration for a single permission type
///
/// Supports two formats:
/// - Simple: `"edit": "allow"` → allows all patterns
/// - Patterned: `"bash": { "git *": "allow", "*": "ask" }` → per-pattern rules
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PermissionConfig {
    /// Simple action for all patterns
    Simple(PermissionAction),
    /// Pattern-based rules
    Patterned(HashMap<String, PermissionAction>),
}

impl PermissionConfig {
    /// Convert to a list of rules for a given permission type
    pub fn to_rules(&self, permission: &str) -> Vec<PermissionRule> {
        match self {
            Self::Simple(action) => {
                vec![PermissionRule::new(permission, "*", *action)]
            }
            Self::Patterned(patterns) => {
                let mut rules: Vec<_> = patterns
                    .iter()
                    .map(|(pattern, action)| PermissionRule::new(permission, pattern, *action))
                    .collect();
                // Sort deterministically: Deny first, then Ask, then Allow;
                // within same action, sort by pattern for stable ordering
                rules.sort_by(|a, b| {
                    fn action_priority(a: &PermissionAction) -> u8 {
                        match a {
                            PermissionAction::Deny => 0,
                            PermissionAction::Ask => 1,
                            PermissionAction::Allow => 2,
                        }
                    }
                    action_priority(&a.action)
                        .cmp(&action_priority(&b.action))
                        .then_with(|| a.pattern.cmp(&b.pattern))
                });
                rules
            }
        }
    }
}

/// Complete permission configuration map
pub type PermissionConfigMap = HashMap<String, PermissionConfig>;

/// Parse a permission config map into a ruleset
pub fn config_to_ruleset(config: &PermissionConfigMap) -> Ruleset {
    config
        .iter()
        .flat_map(|(permission, perm_config)| perm_config.to_rules(permission))
        .collect()
}

/// Default permission configuration
pub fn default_config() -> PermissionConfigMap {
    let mut config = HashMap::new();

    // Read operations are generally safe
    config.insert("read".into(), PermissionConfig::Simple(PermissionAction::Allow));
    config.insert("glob".into(), PermissionConfig::Simple(PermissionAction::Allow));
    config.insert("grep".into(), PermissionConfig::Simple(PermissionAction::Allow));
    config.insert("list".into(), PermissionConfig::Simple(PermissionAction::Allow));

    // Edit operations need user consent by default
    config.insert("edit".into(), PermissionConfig::Simple(PermissionAction::Ask));

    // Bash commands need careful handling
    let mut bash_patterns = HashMap::new();
    bash_patterns.insert("git *".into(), PermissionAction::Allow);
    bash_patterns.insert("cargo *".into(), PermissionAction::Allow);
    bash_patterns.insert("npm *".into(), PermissionAction::Allow);
    bash_patterns.insert("pnpm *".into(), PermissionAction::Allow);
    bash_patterns.insert("yarn *".into(), PermissionAction::Allow);
    bash_patterns.insert("rm -rf *".into(), PermissionAction::Deny);
    bash_patterns.insert("*".into(), PermissionAction::Ask);
    config.insert("bash".into(), PermissionConfig::Patterned(bash_patterns));

    // External directory access
    config.insert(
        "external_directory".into(),
        PermissionConfig::Simple(PermissionAction::Ask),
    );

    // User interaction (question tool)
    config.insert("question".into(), PermissionConfig::Simple(PermissionAction::Allow));

    // Web operations
    config.insert("webfetch".into(), PermissionConfig::Simple(PermissionAction::Ask));
    config.insert("websearch".into(), PermissionConfig::Simple(PermissionAction::Ask));

    config
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_config() {
        let config = PermissionConfig::Simple(PermissionAction::Allow);
        let rules = config.to_rules("edit");

        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].permission, "edit");
        assert_eq!(rules[0].pattern, "*");
        assert_eq!(rules[0].action, PermissionAction::Allow);
    }

    #[test]
    fn test_patterned_config() {
        let mut patterns = HashMap::new();
        patterns.insert("git *".into(), PermissionAction::Allow);
        patterns.insert("*".into(), PermissionAction::Ask);

        let config = PermissionConfig::Patterned(patterns);
        let rules = config.to_rules("bash");

        assert_eq!(rules.len(), 2);
        assert!(rules.iter().any(|r| r.pattern == "git *" && r.action == PermissionAction::Allow));
        assert!(rules.iter().any(|r| r.pattern == "*" && r.action == PermissionAction::Ask));
    }

    #[test]
    fn test_config_to_ruleset() {
        let mut config = HashMap::new();
        config.insert("edit".into(), PermissionConfig::Simple(PermissionAction::Allow));
        config.insert("read".into(), PermissionConfig::Simple(PermissionAction::Allow));

        let ruleset = config_to_ruleset(&config);
        assert_eq!(ruleset.len(), 2);
    }

    #[test]
    fn test_config_serialization() {
        let config = PermissionConfig::Simple(PermissionAction::Allow);
        let json = serde_json::to_string(&config).unwrap();
        assert_eq!(json, "\"allow\"");

        let mut patterns = HashMap::new();
        patterns.insert("git *".into(), PermissionAction::Allow);
        let config = PermissionConfig::Patterned(patterns);
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("git *"));
    }

    #[test]
    fn test_default_config() {
        let config = default_config();

        // Read should be allowed
        assert!(matches!(
            config.get("read"),
            Some(PermissionConfig::Simple(PermissionAction::Allow))
        ));

        // Bash should have patterns
        assert!(matches!(config.get("bash"), Some(PermissionConfig::Patterned(_))));
    }
}
