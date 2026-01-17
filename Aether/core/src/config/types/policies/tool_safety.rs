//! Tool safety inference policies
//!
//! Defines configurable keyword patterns and fallback rules for inferring
//! tool safety levels without hardcoding them in the mechanism code.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Policy for inferring tool safety levels based on name patterns
///
/// This policy allows users to customize which keywords trigger which safety
/// classifications, and what fallback levels to use for each tool category.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSafetyPolicy {
    /// Keywords indicating high-risk irreversible operations
    /// Default: ["delete", "remove", "drop", "shell", "execute", "run_command",
    ///           "bash", "terminal", "destroy", "erase", "purge"]
    #[serde(default = "default_high_risk_keywords")]
    pub high_risk_keywords: Vec<String>,

    /// Keywords indicating low-risk irreversible operations
    /// Default: ["send", "notify", "post", "publish", "email", "message", "commit", "push"]
    #[serde(default = "default_low_risk_keywords")]
    pub low_risk_keywords: Vec<String>,

    /// Keywords indicating reversible operations
    /// Default: ["create", "copy", "move", "rename", "update", "write", "edit",
    ///           "modify", "set", "add", "insert"]
    #[serde(default = "default_reversible_keywords")]
    pub reversible_keywords: Vec<String>,

    /// Keywords indicating read-only operations
    /// Default: ["search", "query", "get", "read", "list", "show", "view",
    ///           "find", "fetch", "browse", "summarize", "translate", "analyze"]
    #[serde(default = "default_readonly_keywords")]
    pub readonly_keywords: Vec<String>,

    /// Default safety level for unmatched Builtin tools
    /// Values: "readonly", "reversible", "irreversible_low_risk", "irreversible_high_risk"
    #[serde(default = "default_builtin_fallback")]
    pub builtin_fallback: String,

    /// Default safety level for unmatched Native tools
    #[serde(default = "default_native_fallback")]
    pub native_fallback: String,

    /// Default safety level for unmatched MCP tools
    #[serde(default = "default_mcp_fallback")]
    pub mcp_fallback: String,

    /// Default safety level for unmatched Skill tools
    #[serde(default = "default_skill_fallback")]
    pub skill_fallback: String,

    /// Default safety level for unmatched Custom tools
    #[serde(default = "default_custom_fallback")]
    pub custom_fallback: String,
}

impl Default for ToolSafetyPolicy {
    fn default() -> Self {
        Self {
            high_risk_keywords: default_high_risk_keywords(),
            low_risk_keywords: default_low_risk_keywords(),
            reversible_keywords: default_reversible_keywords(),
            readonly_keywords: default_readonly_keywords(),
            builtin_fallback: default_builtin_fallback(),
            native_fallback: default_native_fallback(),
            mcp_fallback: default_mcp_fallback(),
            skill_fallback: default_skill_fallback(),
            custom_fallback: default_custom_fallback(),
        }
    }
}

impl ToolSafetyPolicy {
    /// Check if name contains any high-risk keywords
    pub fn is_high_risk(&self, name: &str) -> bool {
        let name_lower = name.to_lowercase();
        self.high_risk_keywords.iter().any(|k| name_lower.contains(k))
    }

    /// Check if name contains any low-risk keywords
    pub fn is_low_risk(&self, name: &str) -> bool {
        let name_lower = name.to_lowercase();
        self.low_risk_keywords.iter().any(|k| name_lower.contains(k))
    }

    /// Check if name contains any reversible keywords
    pub fn is_reversible(&self, name: &str) -> bool {
        let name_lower = name.to_lowercase();
        self.reversible_keywords.iter().any(|k| name_lower.contains(k))
    }

    /// Check if name contains any readonly keywords
    pub fn is_readonly(&self, name: &str) -> bool {
        let name_lower = name.to_lowercase();
        self.readonly_keywords.iter().any(|k| name_lower.contains(k))
    }

    /// Parse a safety level string to enum-compatible value
    /// Returns: "readonly", "reversible", "irreversible_low_risk", "irreversible_high_risk"
    pub fn parse_safety_level(&self, level: &str) -> &'static str {
        match level.to_lowercase().as_str() {
            "readonly" | "read_only" | "read-only" => "readonly",
            "reversible" => "reversible",
            "irreversible_low_risk" | "irreversible-low-risk" | "low_risk" | "low-risk" => {
                "irreversible_low_risk"
            }
            "irreversible_high_risk" | "irreversible-high-risk" | "high_risk" | "high-risk" => {
                "irreversible_high_risk"
            }
            _ => "irreversible_low_risk", // Default fallback
        }
    }

    /// Convert to HashSet for efficient lookup (internal use)
    pub fn high_risk_set(&self) -> HashSet<String> {
        self.high_risk_keywords.iter().cloned().collect()
    }

    /// Convert to HashSet for efficient lookup (internal use)
    pub fn low_risk_set(&self) -> HashSet<String> {
        self.low_risk_keywords.iter().cloned().collect()
    }

    /// Convert to HashSet for efficient lookup (internal use)
    pub fn reversible_set(&self) -> HashSet<String> {
        self.reversible_keywords.iter().cloned().collect()
    }

    /// Convert to HashSet for efficient lookup (internal use)
    pub fn readonly_set(&self) -> HashSet<String> {
        self.readonly_keywords.iter().cloned().collect()
    }
}

fn default_high_risk_keywords() -> Vec<String> {
    vec![
        "delete", "remove", "drop", "shell", "execute", "run_command",
        "bash", "terminal", "destroy", "erase", "purge",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn default_low_risk_keywords() -> Vec<String> {
    vec!["send", "notify", "post", "publish", "email", "message", "commit", "push"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

fn default_reversible_keywords() -> Vec<String> {
    vec![
        "create", "copy", "move", "rename", "update", "write", "edit",
        "modify", "set", "add", "insert",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn default_readonly_keywords() -> Vec<String> {
    vec![
        "search", "query", "get", "read", "list", "show", "view",
        "find", "fetch", "browse", "summarize", "translate", "analyze",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn default_builtin_fallback() -> String {
    "readonly".to_string()
}

fn default_native_fallback() -> String {
    "reversible".to_string()
}

fn default_mcp_fallback() -> String {
    "irreversible_low_risk".to_string()
}

fn default_skill_fallback() -> String {
    "irreversible_low_risk".to_string()
}

fn default_custom_fallback() -> String {
    "irreversible_low_risk".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_policy_has_expected_keywords() {
        let policy = ToolSafetyPolicy::default();
        assert!(policy.high_risk_keywords.contains(&"delete".to_string()));
        assert!(policy.high_risk_keywords.contains(&"shell".to_string()));
        assert!(policy.readonly_keywords.contains(&"search".to_string()));
        assert!(policy.readonly_keywords.contains(&"query".to_string()));
    }

    #[test]
    fn test_fallback_defaults() {
        let policy = ToolSafetyPolicy::default();
        assert_eq!(policy.builtin_fallback, "readonly");
        assert_eq!(policy.native_fallback, "reversible");
        assert_eq!(policy.mcp_fallback, "irreversible_low_risk");
    }

    #[test]
    fn test_partial_deserialization() {
        let toml = r#"
            high_risk_keywords = ["custom_delete", "danger"]
        "#;
        let policy: ToolSafetyPolicy = toml::from_str(toml).unwrap();
        // Custom keywords used
        assert!(policy.high_risk_keywords.contains(&"custom_delete".to_string()));
        assert!(policy.high_risk_keywords.contains(&"danger".to_string()));
        // Defaults NOT included when overridden
        assert!(!policy.high_risk_keywords.contains(&"delete".to_string()));
        // Other fields use defaults
        assert!(policy.readonly_keywords.contains(&"search".to_string()));
    }
}
