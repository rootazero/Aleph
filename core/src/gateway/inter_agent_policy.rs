//! Agent-to-Agent Communication Policy
//!
//! Controls which agents can communicate with which other agents.
//! Used by the sessions_send tool to authorize inter-agent messaging.

use serde::{Deserialize, Serialize};

/// Policy for agent-to-agent communication
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentToAgentPolicy {
    /// Whether A2A communication is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Patterns for allowed agent communication
    /// - `"*"` = match all agents
    /// - `"work-*"` = prefix match (e.g., matches "work-agent", "work-123")
    #[serde(default)]
    pub allow_patterns: Vec<String>,
}

fn default_true() -> bool {
    true
}

impl Default for AgentToAgentPolicy {
    fn default() -> Self {
        Self::permissive()
    }
}

impl AgentToAgentPolicy {
    /// Create a new A2A policy with specified settings
    pub fn new(enabled: bool, allow_patterns: Vec<String>) -> Self {
        Self {
            enabled,
            allow_patterns,
        }
    }

    /// Create a permissive policy that allows all agent communication
    ///
    /// `enabled=true, allow_patterns=["*"]`
    pub fn permissive() -> Self {
        Self {
            enabled: true,
            allow_patterns: vec!["*".to_string()],
        }
    }

    /// Create a disabled policy that blocks all cross-agent communication
    ///
    /// Same-agent communication is still allowed even when disabled.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            allow_patterns: vec![],
        }
    }

    /// Check if communication from requester to target is allowed
    ///
    /// Rules:
    /// 1. Same-agent communication is always allowed (even if disabled)
    /// 2. If disabled, only same-agent is allowed
    /// 3. If enabled, check allow_patterns for a match
    ///
    /// Pattern matching:
    /// - `"*"` matches any agent
    /// - `"prefix-*"` matches any agent starting with "prefix-"
    /// - Exact string matches the exact agent name
    pub fn is_allowed(&self, requester: &str, target: &str) -> bool {
        // Same-agent communication is always allowed
        if requester == target {
            return true;
        }

        // If disabled, only same-agent is allowed
        if !self.enabled {
            return false;
        }

        // Check allow patterns
        self.matches_any_pattern(target)
    }

    /// Check if the target matches any of the allow patterns
    fn matches_any_pattern(&self, target: &str) -> bool {
        for pattern in &self.allow_patterns {
            if Self::matches_pattern(pattern, target) {
                return true;
            }
        }
        false
    }

    /// Check if a target matches a single pattern
    ///
    /// - `"*"` matches anything
    /// - `"prefix-*"` matches anything starting with "prefix-"
    /// - Otherwise, exact match is required
    fn matches_pattern(pattern: &str, target: &str) -> bool {
        if pattern == "*" {
            return true;
        }

        if let Some(prefix) = pattern.strip_suffix('*') {
            return target.starts_with(prefix);
        }

        pattern == target
    }

    /// Add an allow pattern
    pub fn add_pattern(&mut self, pattern: impl Into<String>) {
        self.allow_patterns.push(pattern.into());
    }

    /// Remove an allow pattern
    pub fn remove_pattern(&mut self, pattern: &str) -> bool {
        if let Some(pos) = self.allow_patterns.iter().position(|p| p == pattern) {
            self.allow_patterns.remove(pos);
            true
        } else {
            false
        }
    }

    /// Check if the policy has any allow patterns
    pub fn has_patterns(&self) -> bool {
        !self.allow_patterns.is_empty()
    }

    /// Set enabled status
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permissive_policy() {
        let policy = AgentToAgentPolicy::permissive();
        assert!(policy.enabled);
        assert_eq!(policy.allow_patterns, vec!["*"]);

        // All communication should be allowed
        assert!(policy.is_allowed("agent-a", "agent-b"));
        assert!(policy.is_allowed("agent-a", "agent-a"));
        assert!(policy.is_allowed("main", "translator"));
    }

    #[test]
    fn test_disabled_policy() {
        let policy = AgentToAgentPolicy::disabled();
        assert!(!policy.enabled);
        assert!(policy.allow_patterns.is_empty());

        // Only same-agent should be allowed
        assert!(policy.is_allowed("agent-a", "agent-a"));
        assert!(!policy.is_allowed("agent-a", "agent-b"));
        assert!(!policy.is_allowed("main", "translator"));
    }

    #[test]
    fn test_same_agent_always_allowed() {
        // Even with empty patterns and disabled, same-agent is allowed
        let policy = AgentToAgentPolicy::new(false, vec![]);
        assert!(policy.is_allowed("main", "main"));
        assert!(policy.is_allowed("work-agent", "work-agent"));

        // With patterns but disabled
        let policy2 = AgentToAgentPolicy::new(false, vec!["*".to_string()]);
        assert!(policy2.is_allowed("main", "main"));
        assert!(!policy2.is_allowed("main", "other")); // disabled takes precedence
    }

    #[test]
    fn test_wildcard_pattern() {
        let policy = AgentToAgentPolicy::new(true, vec!["*".to_string()]);

        assert!(policy.is_allowed("a", "b"));
        assert!(policy.is_allowed("main", "translator"));
        assert!(policy.is_allowed("work-123", "personal-agent"));
    }

    #[test]
    fn test_prefix_pattern() {
        let policy = AgentToAgentPolicy::new(true, vec!["work-*".to_string()]);

        // Matches prefix
        assert!(policy.is_allowed("main", "work-agent"));
        assert!(policy.is_allowed("main", "work-123"));
        assert!(policy.is_allowed("main", "work-"));

        // Does not match
        assert!(!policy.is_allowed("main", "personal-agent"));
        assert!(!policy.is_allowed("main", "workagent")); // no hyphen
        assert!(!policy.is_allowed("main", "my-work-agent"));
    }

    #[test]
    fn test_exact_pattern() {
        let policy = AgentToAgentPolicy::new(true, vec!["translator".to_string()]);

        assert!(policy.is_allowed("main", "translator"));
        assert!(!policy.is_allowed("main", "translator-v2"));
        assert!(!policy.is_allowed("main", "other"));
    }

    #[test]
    fn test_multiple_patterns() {
        let policy = AgentToAgentPolicy::new(
            true,
            vec![
                "work-*".to_string(),
                "translator".to_string(),
                "personal-*".to_string(),
            ],
        );

        assert!(policy.is_allowed("main", "work-agent"));
        assert!(policy.is_allowed("main", "translator"));
        assert!(policy.is_allowed("main", "personal-assistant"));
        assert!(!policy.is_allowed("main", "other-agent"));
    }

    #[test]
    fn test_new_constructor() {
        let policy = AgentToAgentPolicy::new(true, vec!["test-*".to_string()]);
        assert!(policy.enabled);
        assert_eq!(policy.allow_patterns, vec!["test-*"]);
    }

    #[test]
    fn test_add_remove_pattern() {
        let mut policy = AgentToAgentPolicy::disabled();

        policy.set_enabled(true);
        assert!(policy.enabled);

        policy.add_pattern("work-*");
        assert!(policy.has_patterns());
        assert!(policy.is_allowed("main", "work-agent"));

        policy.add_pattern("personal-*");
        assert!(policy.is_allowed("main", "personal-agent"));

        let removed = policy.remove_pattern("work-*");
        assert!(removed);
        assert!(!policy.is_allowed("main", "work-agent"));
        assert!(policy.is_allowed("main", "personal-agent"));

        // Remove non-existent
        let removed2 = policy.remove_pattern("nonexistent");
        assert!(!removed2);
    }

    #[test]
    fn test_serialization() {
        let policy = AgentToAgentPolicy::new(true, vec!["work-*".to_string(), "main".to_string()]);

        let json = serde_json::to_string(&policy).unwrap();
        assert!(json.contains("\"enabled\":true"));
        assert!(json.contains("\"allow_patterns\""));

        let deserialized: AgentToAgentPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.enabled, policy.enabled);
        assert_eq!(deserialized.allow_patterns, policy.allow_patterns);
    }

    #[test]
    fn test_default_serialization() {
        // Test default values during deserialization
        let json = "{}";
        let policy: AgentToAgentPolicy = serde_json::from_str(json).unwrap();
        assert!(policy.enabled); // default_true
        assert!(policy.allow_patterns.is_empty()); // default empty vec
    }

    #[test]
    fn test_default_impl() {
        let policy = AgentToAgentPolicy::default();
        let permissive = AgentToAgentPolicy::permissive();

        assert_eq!(policy.enabled, permissive.enabled);
        assert_eq!(policy.allow_patterns, permissive.allow_patterns);
    }

    #[test]
    fn test_empty_pattern_behavior() {
        let policy = AgentToAgentPolicy::new(true, vec![]);

        // Enabled but no patterns means nothing matches
        assert!(policy.is_allowed("main", "main")); // same agent
        assert!(!policy.is_allowed("main", "other")); // different agent, no patterns
    }

    #[test]
    fn test_pattern_with_special_characters() {
        let policy = AgentToAgentPolicy::new(
            true,
            vec![
                "agent_with_underscore".to_string(),
                "agent-123-*".to_string(),
            ],
        );

        assert!(policy.is_allowed("main", "agent_with_underscore"));
        assert!(policy.is_allowed("main", "agent-123-abc"));
        assert!(!policy.is_allowed("main", "agent-12-abc")); // doesn't match prefix
    }
}
