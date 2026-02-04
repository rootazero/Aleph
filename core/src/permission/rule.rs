// Aleph/core/src/permission/rule.rs
//! Permission rule definitions and evaluation.

use crate::extension::PermissionAction;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// A permission rule that maps (permission_type, pattern) → action
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRule {
    /// Permission type (e.g., "edit", "bash", "read")
    pub permission: String,
    /// Match pattern (supports wildcards: "*", "?")
    pub pattern: String,
    /// Action to take when matched
    pub action: PermissionAction,
}

impl PermissionRule {
    /// Create a new permission rule
    pub fn new(
        permission: impl Into<String>,
        pattern: impl Into<String>,
        action: PermissionAction,
    ) -> Self {
        Self {
            permission: permission.into(),
            pattern: pattern.into(),
            action,
        }
    }

    /// Create an allow rule
    pub fn allow(permission: impl Into<String>, pattern: impl Into<String>) -> Self {
        Self::new(permission, pattern, PermissionAction::Allow)
    }

    /// Create a deny rule
    pub fn deny(permission: impl Into<String>, pattern: impl Into<String>) -> Self {
        Self::new(permission, pattern, PermissionAction::Deny)
    }

    /// Create an ask rule
    pub fn ask(permission: impl Into<String>, pattern: impl Into<String>) -> Self {
        Self::new(permission, pattern, PermissionAction::Ask)
    }
}

/// A collection of permission rules
pub type Ruleset = Vec<PermissionRule>;

/// Mapping from tool names to permission types
#[derive(Debug, Clone, Default)]
pub struct PermissionMapping {
    /// Tools that map to "edit" permission
    edit_tools: HashSet<String>,
    /// Tools that map to "read" permission
    read_tools: HashSet<String>,
}

impl PermissionMapping {
    /// Create a new permission mapping with default tool mappings
    pub fn new() -> Self {
        let mut mapping = Self::default();

        // Edit tools (like OpenCode's EDIT_TOOLS)
        mapping.edit_tools.extend([
            "edit".into(),
            "write".into(),
            "patch".into(),
            "file_write".into(),
            "multiedit".into(),
        ]);

        // Read tools
        mapping.read_tools.extend([
            "read".into(),
            "file_read".into(),
            "glob".into(),
            "grep".into(),
            "list".into(),
        ]);

        mapping
    }

    /// Get the permission type for a tool
    pub fn permission_for_tool<'a>(&self, tool_name: &'a str) -> &'a str {
        if self.edit_tools.contains(tool_name) {
            "edit"
        } else if self.read_tools.contains(tool_name) {
            "read"
        } else {
            tool_name
        }
    }

    /// Add a tool to the edit tools set
    pub fn add_edit_tool(&mut self, tool: impl Into<String>) {
        self.edit_tools.insert(tool.into());
    }

    /// Add a tool to the read tools set
    pub fn add_read_tool(&mut self, tool: impl Into<String>) {
        self.read_tools.insert(tool.into());
    }
}

/// Permission rule evaluator
#[derive(Debug, Clone)]
pub struct PermissionEvaluator {
    /// Tool to permission type mapping
    mapping: PermissionMapping,
}

impl Default for PermissionEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

impl PermissionEvaluator {
    /// Create a new evaluator with default mappings
    pub fn new() -> Self {
        Self {
            mapping: PermissionMapping::new(),
        }
    }

    /// Create an evaluator with custom mapping
    pub fn with_mapping(mapping: PermissionMapping) -> Self {
        Self { mapping }
    }

    /// Get the permission type for a tool
    pub fn permission_for_tool<'a>(&self, tool_name: &'a str) -> &'a str {
        self.mapping.permission_for_tool(tool_name)
    }

    /// Evaluate a permission request against rulesets
    ///
    /// # Rule Priority
    /// Later rules in the merged ruleset take precedence (last match wins).
    /// Rulesets are merged in order, so:
    /// 1. Global config (lowest priority)
    /// 2. Session-level overrides
    /// 3. Runtime approvals (highest priority)
    ///
    /// # Default Behavior
    /// If no rule matches, returns "ask" action.
    pub fn evaluate(&self, permission: &str, pattern: &str, rulesets: &[&Ruleset]) -> PermissionRule {
        // Merge all rulesets in order
        let merged: Vec<&PermissionRule> = rulesets.iter().flat_map(|r| r.iter()).collect();

        // Find matching rule from end (later definitions win)
        for rule in merged.iter().rev() {
            if wildcard_match(permission, &rule.permission)
                && wildcard_match(pattern, &rule.pattern)
            {
                return (*rule).clone();
            }
        }

        // Default: ask
        PermissionRule {
            permission: permission.to_string(),
            pattern: "*".to_string(),
            action: PermissionAction::Ask,
        }
    }

    /// Check which tools are disabled by rules (have global "deny" with "*" pattern)
    pub fn disabled_tools(&self, tool_names: &[&str], ruleset: &Ruleset) -> HashSet<String> {
        let mut disabled = HashSet::new();

        for tool in tool_names {
            let permission = self.permission_for_tool(tool);

            // Find the last matching rule with "*" pattern
            let rule = ruleset
                .iter()
                .rev()
                .find(|r| wildcard_match(permission, &r.permission) && r.pattern == "*");

            if let Some(rule) = rule {
                if rule.action == PermissionAction::Deny {
                    disabled.insert((*tool).to_string());
                }
            }
        }

        disabled
    }
}

/// Simple wildcard matching (supports * and ?)
pub fn wildcard_match(text: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    let text_chars: Vec<char> = text.chars().collect();
    let pattern_chars: Vec<char> = pattern.chars().collect();

    wildcard_match_recursive(&text_chars, &pattern_chars, 0, 0)
}

fn wildcard_match_recursive(
    text: &[char],
    pattern: &[char],
    ti: usize,
    pi: usize,
) -> bool {
    // Both exhausted - match
    if ti == text.len() && pi == pattern.len() {
        return true;
    }

    // Pattern exhausted but text remains - no match
    if pi == pattern.len() {
        return false;
    }

    // Handle '*' - match zero or more characters
    if pattern[pi] == '*' {
        // Skip consecutive '*'
        let mut next_pi = pi;
        while next_pi < pattern.len() && pattern[next_pi] == '*' {
            next_pi += 1;
        }

        // Try matching * with zero to remaining characters
        for i in ti..=text.len() {
            if wildcard_match_recursive(text, pattern, i, next_pi) {
                return true;
            }
        }
        return false;
    }

    // Text exhausted but pattern remains (and not '*')
    if ti == text.len() {
        return false;
    }

    // Handle '?' - match any single character
    if pattern[pi] == '?' || pattern[pi] == text[ti] {
        return wildcard_match_recursive(text, pattern, ti + 1, pi + 1);
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wildcard_match_basic() {
        assert!(wildcard_match("hello", "*"));
        assert!(wildcard_match("hello", "hello"));
        assert!(wildcard_match("hello", "h*"));
        assert!(wildcard_match("hello", "*o"));
        assert!(wildcard_match("hello", "h*o"));
        assert!(wildcard_match("hello", "h?llo"));
        assert!(wildcard_match("hello", "?????"));

        assert!(!wildcard_match("hello", "world"));
        assert!(!wildcard_match("hello", "h*x"));
        assert!(!wildcard_match("hello", "????"));
    }

    #[test]
    fn test_wildcard_match_commands() {
        assert!(wildcard_match("git push", "git *"));
        assert!(wildcard_match("git pull origin main", "git *"));
        assert!(wildcard_match("rm -rf /tmp/test", "rm -rf *"));
        assert!(!wildcard_match("cargo build", "git *"));
    }

    #[test]
    fn test_wildcard_match_paths() {
        assert!(wildcard_match("/home/user/src/main.rs", "*/src/*"));
        assert!(wildcard_match("/home/user/.secrets/key", "*/.secrets/*"));
        assert!(wildcard_match("~/Workspace/project", "~/Workspace/*"));
    }

    #[test]
    fn test_permission_mapping() {
        let mapping = PermissionMapping::new();

        assert_eq!(mapping.permission_for_tool("edit"), "edit");
        assert_eq!(mapping.permission_for_tool("write"), "edit");
        assert_eq!(mapping.permission_for_tool("file_write"), "edit");
        assert_eq!(mapping.permission_for_tool("read"), "read");
        assert_eq!(mapping.permission_for_tool("glob"), "read");
        assert_eq!(mapping.permission_for_tool("bash"), "bash");
        assert_eq!(mapping.permission_for_tool("custom_tool"), "custom_tool");
    }

    #[test]
    fn test_evaluator_basic() {
        let evaluator = PermissionEvaluator::new();

        // Note: "last rule wins" - order matters!
        // More specific deny rules should come AFTER general ask rules
        let rules = vec![
            PermissionRule::allow("edit", "*"),
            PermissionRule::ask("bash", "*"),       // General: ask for all bash
            PermissionRule::deny("bash", "rm -rf *"), // Specific: deny rm -rf (after ask)
        ];

        // Edit should be allowed
        let result = evaluator.evaluate("edit", "src/main.rs", &[&rules]);
        assert_eq!(result.action, PermissionAction::Allow);

        // rm -rf should be denied (matches the later deny rule)
        let result = evaluator.evaluate("bash", "rm -rf /", &[&rules]);
        assert_eq!(result.action, PermissionAction::Deny);

        // Other bash commands should ask (only matches the ask rule)
        let result = evaluator.evaluate("bash", "git push", &[&rules]);
        assert_eq!(result.action, PermissionAction::Ask);
    }

    #[test]
    fn test_evaluator_precedence() {
        let evaluator = PermissionEvaluator::new();

        // Later rules should win
        let global = vec![PermissionRule::ask("bash", "*")];
        let approved = vec![PermissionRule::allow("bash", "git *")];

        // With only global rules, should ask
        let result = evaluator.evaluate("bash", "git push", &[&global]);
        assert_eq!(result.action, PermissionAction::Ask);

        // With approved rules, should allow
        let result = evaluator.evaluate("bash", "git push", &[&global, &approved]);
        assert_eq!(result.action, PermissionAction::Allow);
    }

    #[test]
    fn test_evaluator_default_ask() {
        let evaluator = PermissionEvaluator::new();
        let empty: Ruleset = vec![];

        // No rules - default to ask
        let result = evaluator.evaluate("unknown", "something", &[&empty]);
        assert_eq!(result.action, PermissionAction::Ask);
    }

    #[test]
    fn test_disabled_tools() {
        let evaluator = PermissionEvaluator::new();

        let rules = vec![
            PermissionRule::deny("edit", "*"),
            PermissionRule::allow("read", "*"),
            PermissionRule::ask("bash", "*"),
        ];

        let tools = ["edit", "write", "read", "bash"];
        let disabled = evaluator.disabled_tools(&tools, &rules);

        assert!(disabled.contains("edit"));
        assert!(disabled.contains("write")); // Maps to "edit" permission
        assert!(!disabled.contains("read"));
        assert!(!disabled.contains("bash"));
    }
}
