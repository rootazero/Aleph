//! YAML Policy Schema Definitions

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A single rule from YAML configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct YamlRule {
    pub name: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub trigger: Trigger,
    #[serde(default)]
    pub constraints: Vec<Constraint>,
    #[serde(default)]
    pub conditions: Vec<Condition>,
    pub action: Action,
    pub risk: RiskLevel,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Trigger {
    pub event: String,
    #[serde(rename = "to")]
    pub to_activity: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Constraint {
    // TODO: Define constraint format
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Condition {
    pub expr: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Action {
    #[serde(rename = "type")]
    pub action_type: String,
    pub message: Option<String>,
    pub priority: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_yaml_rule() {
        let yaml = r#"
name: "Low Battery Alert"
trigger:
  event: resource_pressure_changed
  pressure_type: battery
constraints:
  - battery_level: "< 20"
action:
  type: notify
  message: "Battery low"
  priority: high
risk: low
"#;
        let rule: YamlRule = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(rule.name, "Low Battery Alert");
        assert_eq!(rule.risk, RiskLevel::Low);
        assert!(rule.enabled);
    }

    #[test]
    fn test_parse_complex_yaml_rule_with_conditions() {
        let yaml = r#"
name: "Smart Break Reminder"
enabled: true
trigger:
  event: activity_changed
  to: programming
conditions:
  - expr: |
      history.last("2h")
        .filter(|e| e.is_coding())
        .sum_duration() > duration("90m")
action:
  type: notify
  message: "Take a break"
risk: low
"#;
        let rule: YamlRule = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(rule.name, "Smart Break Reminder");
        assert_eq!(rule.conditions.len(), 1);
        assert!(rule.conditions[0].expr.contains("history.last"));
    }

    #[test]
    fn test_parse_rule_with_metadata() {
        let yaml = r#"
name: "Test Rule"
trigger:
  event: test
action:
  type: notify
risk: low
metadata:
  author: "aether-community"
  tags: ["test", "example"]
"#;
        let rule: YamlRule = serde_yaml::from_str(yaml).unwrap();
        assert!(rule.metadata.contains_key("author"));
    }
}
