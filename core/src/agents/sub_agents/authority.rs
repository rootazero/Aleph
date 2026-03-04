//! SubAgent authorization enforcement.
//!
//! Checks whether a parent agent is allowed to delegate to a child agent
//! based on the SubagentPolicy defined in config.

use std::collections::HashMap;

use crate::config::types::agents_def::SubagentPolicy;

/// Trait for checking sub-agent delegation authorization.
pub trait SubagentAuthority: Send + Sync {
    /// Check if parent_agent_id is allowed to delegate to child_agent_id.
    fn can_delegate(&self, parent_agent_id: &str, child_agent_id: &str) -> bool;
}

/// Config-driven authorization using SubagentPolicy from agent definitions.
pub struct ConfigDrivenAuthority {
    /// Map of agent_id -> SubagentPolicy (only agents with explicit config)
    policies: HashMap<String, SubagentPolicy>,
}

impl ConfigDrivenAuthority {
    /// Create from resolved agent definitions.
    ///
    /// Only agents with explicit `[agents.list.subagents]` config get entries.
    /// Agents without config are not in the map (-> allow all, backward compat).
    pub fn from_policies(policies: HashMap<String, SubagentPolicy>) -> Self {
        Self { policies }
    }

    /// Create from resolved agents, extracting only non-default policies.
    pub fn from_resolved(agents: &[crate::config::agent_resolver::ResolvedAgent]) -> Self {
        let policies = agents
            .iter()
            .filter(|a| !a.subagent_policy.allow.is_empty())
            .map(|a| (a.id.clone(), a.subagent_policy.clone()))
            .collect();
        Self { policies }
    }
}

impl SubagentAuthority for ConfigDrivenAuthority {
    fn can_delegate(&self, parent_id: &str, child_id: &str) -> bool {
        match self.policies.get(parent_id) {
            // No explicit policy -> allow all (backward compat)
            None => true,
            Some(policy) => policy.allow.iter().any(|a| a == "*" || a == child_id),
        }
    }
}

/// Permissive authority that allows all delegation (default/fallback).
pub struct PermissiveAuthority;

impl SubagentAuthority for PermissiveAuthority {
    fn can_delegate(&self, _parent_id: &str, _child_id: &str) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permissive_allows_all() {
        let auth = PermissiveAuthority;
        assert!(auth.can_delegate("any", "any"));
    }

    #[test]
    fn test_config_no_policy_allows_all() {
        let auth = ConfigDrivenAuthority::from_policies(HashMap::new());
        assert!(auth.can_delegate("main", "coding"));
    }

    #[test]
    fn test_config_wildcard_allows_all() {
        let mut policies = HashMap::new();
        policies.insert(
            "main".to_string(),
            SubagentPolicy {
                allow: vec!["*".to_string()],
            },
        );
        let auth = ConfigDrivenAuthority::from_policies(policies);
        assert!(auth.can_delegate("main", "coding"));
        assert!(auth.can_delegate("main", "reviewer"));
    }

    #[test]
    fn test_config_specific_allows_listed() {
        let mut policies = HashMap::new();
        policies.insert(
            "main".to_string(),
            SubagentPolicy {
                allow: vec!["coding".to_string(), "reviewer".to_string()],
            },
        );
        let auth = ConfigDrivenAuthority::from_policies(policies);
        assert!(auth.can_delegate("main", "coding"));
        assert!(auth.can_delegate("main", "reviewer"));
        assert!(!auth.can_delegate("main", "hacker"));
    }

    #[test]
    fn test_config_empty_allow_denies_all() {
        let mut policies = HashMap::new();
        policies.insert(
            "strict".to_string(),
            SubagentPolicy {
                allow: vec![],
            },
        );
        let auth = ConfigDrivenAuthority::from_policies(policies);
        assert!(!auth.can_delegate("strict", "anything"));
        assert!(auth.can_delegate("other", "anything"));
    }
}
