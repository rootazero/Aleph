//! Keyword matching policy configuration
//!
//! Configurable keyword rules for intent detection using weighted matching.
//!
//! Note: Types are prefixed with "Policy" to avoid naming conflicts with
//! similar types in the smart_flow module.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Single keyword with weight for policy-based matching
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PolicyWeightedKeyword {
    /// The keyword to match
    pub word: String,
    /// Weight for scoring (default: 1.0)
    #[serde(default = "default_weight")]
    pub weight: f32,
}

fn default_weight() -> f32 {
    1.0
}

/// A keyword rule in policy config
///
/// Different from `smart_flow::KeywordRuleConfig` - this is for the policies
/// system and uses structured `PolicyWeightedKeyword` instead of string format.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PolicyKeywordRule {
    /// Unique identifier for this rule
    pub id: String,
    /// The intent type this rule matches (e.g., "FileOrganize")
    pub intent_type: String,
    /// List of weighted keywords
    pub keywords: Vec<PolicyWeightedKeyword>,
    /// Match mode: "any", "all", or "weighted"
    #[serde(default = "default_match_mode")]
    pub match_mode: String,
    /// Minimum score to trigger this rule
    #[serde(default = "default_min_score")]
    pub min_score: f32,
}

fn default_match_mode() -> String {
    "weighted".to_string()
}

fn default_min_score() -> f32 {
    0.5
}

/// Policy for keyword-based intent detection
///
/// Controls keyword matching rules and scoring thresholds for
/// fast intent classification without AI inference.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct KeywordPolicy {
    /// Whether keyword matching is enabled
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Global minimum score threshold
    #[serde(default = "default_global_min_score")]
    pub global_min_score: f32,
    /// Keyword matching rules
    #[serde(default)]
    pub rules: Vec<PolicyKeywordRule>,
}

fn default_enabled() -> bool {
    true
}

fn default_global_min_score() -> f32 {
    0.6
}

impl Default for KeywordPolicy {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            global_min_score: default_global_min_score(),
            rules: Vec::new(),
        }
    }
}

impl KeywordPolicy {
    /// Create with built-in rules
    pub fn with_builtin_rules() -> Self {
        Self {
            enabled: true,
            global_min_score: 0.6,
            rules: Self::builtin_rules(),
        }
    }

    /// Get the builtin keyword rules for common intents
    fn builtin_rules() -> Vec<PolicyKeywordRule> {
        vec![
            PolicyKeywordRule {
                id: "file_organize".to_string(),
                intent_type: "FileOrganize".to_string(),
                keywords: vec![
                    PolicyWeightedKeyword {
                        word: "整".to_string(),
                        weight: 1.0,
                    },
                    PolicyWeightedKeyword {
                        word: "理".to_string(),
                        weight: 1.0,
                    },
                    PolicyWeightedKeyword {
                        word: "organize".to_string(),
                        weight: 2.0,
                    },
                    PolicyWeightedKeyword {
                        word: "sort".to_string(),
                        weight: 1.5,
                    },
                    PolicyWeightedKeyword {
                        word: "文".to_string(),
                        weight: 0.5,
                    },
                    PolicyWeightedKeyword {
                        word: "件".to_string(),
                        weight: 0.5,
                    },
                    PolicyWeightedKeyword {
                        word: "files".to_string(),
                        weight: 1.0,
                    },
                ],
                match_mode: "weighted".to_string(),
                min_score: 0.5,
            },
            PolicyKeywordRule {
                id: "file_cleanup".to_string(),
                intent_type: "FileCleanup".to_string(),
                keywords: vec![
                    PolicyWeightedKeyword {
                        word: "删".to_string(),
                        weight: 1.0,
                    },
                    PolicyWeightedKeyword {
                        word: "除".to_string(),
                        weight: 1.0,
                    },
                    PolicyWeightedKeyword {
                        word: "清".to_string(),
                        weight: 1.0,
                    },
                    PolicyWeightedKeyword {
                        word: "理".to_string(),
                        weight: 0.5,
                    },
                    PolicyWeightedKeyword {
                        word: "delete".to_string(),
                        weight: 2.0,
                    },
                    PolicyWeightedKeyword {
                        word: "clean".to_string(),
                        weight: 1.5,
                    },
                    PolicyWeightedKeyword {
                        word: "remove".to_string(),
                        weight: 1.5,
                    },
                ],
                match_mode: "weighted".to_string(),
                min_score: 0.5,
            },
            PolicyKeywordRule {
                id: "code_execution".to_string(),
                intent_type: "CodeExecution".to_string(),
                keywords: vec![
                    PolicyWeightedKeyword {
                        word: "运".to_string(),
                        weight: 1.0,
                    },
                    PolicyWeightedKeyword {
                        word: "行".to_string(),
                        weight: 1.0,
                    },
                    PolicyWeightedKeyword {
                        word: "执".to_string(),
                        weight: 1.0,
                    },
                    PolicyWeightedKeyword {
                        word: "run".to_string(),
                        weight: 2.0,
                    },
                    PolicyWeightedKeyword {
                        word: "execute".to_string(),
                        weight: 2.0,
                    },
                    PolicyWeightedKeyword {
                        word: "script".to_string(),
                        weight: 1.0,
                    },
                ],
                match_mode: "weighted".to_string(),
                min_score: 0.5,
            },
        ]
    }

    /// Check if the policy configuration is valid
    pub fn is_valid(&self) -> bool {
        (0.0..=1.0).contains(&self.global_min_score)
            && self
                .rules
                .iter()
                .all(|r| (0.0..=1.0).contains(&r.min_score))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values() {
        let policy = KeywordPolicy::default();
        assert!(policy.enabled);
        assert_eq!(policy.global_min_score, 0.6);
        assert!(policy.rules.is_empty());
    }

    #[test]
    fn test_builtin_rules() {
        let policy = KeywordPolicy::with_builtin_rules();
        assert!(policy.enabled);
        assert_eq!(policy.global_min_score, 0.6);
        assert_eq!(policy.rules.len(), 3);

        // Check rule IDs
        let rule_ids: Vec<&str> = policy.rules.iter().map(|r| r.id.as_str()).collect();
        assert!(rule_ids.contains(&"file_organize"));
        assert!(rule_ids.contains(&"file_cleanup"));
        assert!(rule_ids.contains(&"code_execution"));

        // Check intent types
        let intent_types: Vec<&str> = policy
            .rules
            .iter()
            .map(|r| r.intent_type.as_str())
            .collect();
        assert!(intent_types.contains(&"FileOrganize"));
        assert!(intent_types.contains(&"FileCleanup"));
        assert!(intent_types.contains(&"CodeExecution"));
    }

    #[test]
    fn test_weighted_keyword_default() {
        let toml = r#"
            word = "test"
        "#;
        let keyword: PolicyWeightedKeyword = toml::from_str(toml).unwrap();
        assert_eq!(keyword.word, "test");
        assert_eq!(keyword.weight, 1.0);
    }

    #[test]
    fn test_weighted_keyword_with_weight() {
        let toml = r#"
            word = "important"
            weight = 2.5
        "#;
        let keyword: PolicyWeightedKeyword = toml::from_str(toml).unwrap();
        assert_eq!(keyword.word, "important");
        assert_eq!(keyword.weight, 2.5);
    }

    #[test]
    fn test_keyword_rule_config_defaults() {
        let toml = r#"
            id = "test_rule"
            intent_type = "TestIntent"
            keywords = [
                { word = "test" },
                { word = "check", weight = 1.5 }
            ]
        "#;
        let rule: PolicyKeywordRule = toml::from_str(toml).unwrap();
        assert_eq!(rule.id, "test_rule");
        assert_eq!(rule.intent_type, "TestIntent");
        assert_eq!(rule.match_mode, "weighted");
        assert_eq!(rule.min_score, 0.5);
        assert_eq!(rule.keywords.len(), 2);
        assert_eq!(rule.keywords[0].weight, 1.0);
        assert_eq!(rule.keywords[1].weight, 1.5);
    }

    #[test]
    fn test_keyword_policy_partial_deserialization() {
        let toml = r#"
            enabled = false
            global_min_score = 0.8
        "#;
        let policy: KeywordPolicy = toml::from_str(toml).unwrap();
        assert!(!policy.enabled);
        assert_eq!(policy.global_min_score, 0.8);
        assert!(policy.rules.is_empty());
    }

    #[test]
    fn test_keyword_policy_with_rules() {
        let toml = r#"
            enabled = true
            global_min_score = 0.7

            [[rules]]
            id = "custom_rule"
            intent_type = "CustomIntent"
            match_mode = "any"
            min_score = 0.3
            keywords = [
                { word = "custom", weight = 2.0 },
                { word = "special" }
            ]
        "#;
        let policy: KeywordPolicy = toml::from_str(toml).unwrap();
        assert!(policy.enabled);
        assert_eq!(policy.global_min_score, 0.7);
        assert_eq!(policy.rules.len(), 1);

        let rule = &policy.rules[0];
        assert_eq!(rule.id, "custom_rule");
        assert_eq!(rule.intent_type, "CustomIntent");
        assert_eq!(rule.match_mode, "any");
        assert_eq!(rule.min_score, 0.3);
        assert_eq!(rule.keywords.len(), 2);
    }

    #[test]
    fn test_validity_check() {
        let mut policy = KeywordPolicy::default();
        assert!(policy.is_valid());

        policy.global_min_score = 1.5;
        assert!(!policy.is_valid());

        policy.global_min_score = 0.6;
        policy.rules = vec![PolicyKeywordRule {
            id: "invalid".to_string(),
            intent_type: "Test".to_string(),
            keywords: vec![],
            match_mode: "any".to_string(),
            min_score: -0.1,
        }];
        assert!(!policy.is_valid());
    }

    #[test]
    fn test_serialization_roundtrip() {
        let policy = KeywordPolicy::with_builtin_rules();
        let toml_str = toml::to_string(&policy).unwrap();
        let parsed: KeywordPolicy = toml::from_str(&toml_str).unwrap();

        assert_eq!(policy.enabled, parsed.enabled);
        assert_eq!(policy.global_min_score, parsed.global_min_score);
        assert_eq!(policy.rules.len(), parsed.rules.len());
    }
}
