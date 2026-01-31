//! Configuration diff detection for hot reload.
//!
//! This module provides utilities to compare two configurations and detect
//! which paths have changed. This is useful for hot-reload systems that need
//! to determine what actions to take based on configuration changes.

#![allow(dead_code)] // Reserved for hot-reload feature

use serde_json::Value;

/// Compare two configs and return changed paths.
///
/// This function serializes both configurations to JSON and performs a deep
/// comparison, returning a list of dot-separated paths that have changed.
///
/// # Examples
///
/// ```rust,ignore
/// use aethecore::config::diff_config;
///
/// let prev = Config { port: 8080, host: "localhost".to_string() };
/// let next = Config { port: 9090, host: "localhost".to_string() };
///
/// let changes = diff_config(&prev, &next);
/// assert!(changes.contains(&"port".to_string()));
/// ```
pub fn diff_config<T: serde::Serialize>(prev: &T, next: &T) -> Vec<String> {
    let prev_value = serde_json::to_value(prev).unwrap_or(Value::Null);
    let next_value = serde_json::to_value(next).unwrap_or(Value::Null);

    let mut changes = Vec::new();
    diff_values(&prev_value, &next_value, "", &mut changes);
    changes
}

/// Recursively compare two JSON values and collect changed paths.
fn diff_values(prev: &Value, next: &Value, prefix: &str, changes: &mut Vec<String>) {
    match (prev, next) {
        (Value::Object(prev_map), Value::Object(next_map)) => {
            // Check for removed/changed keys
            for (key, prev_val) in prev_map {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", prefix, key)
                };

                match next_map.get(key) {
                    Some(next_val) => {
                        diff_values(prev_val, next_val, &path, changes);
                    }
                    None => {
                        // Key was removed
                        changes.push(path);
                    }
                }
            }

            // Check for added keys
            for key in next_map.keys() {
                if !prev_map.contains_key(key) {
                    let path = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{}.{}", prefix, key)
                    };
                    changes.push(path);
                }
            }
        }
        (Value::Array(prev_arr), Value::Array(next_arr)) => {
            // For arrays, we treat them as atomic - if they differ at all, report the path
            if prev_arr != next_arr && !prefix.is_empty() {
                changes.push(prefix.to_string());
            }
        }
        _ => {
            // For primitive values, compare directly
            if prev != next && !prefix.is_empty() {
                changes.push(prefix.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize)]
    struct TestConfig {
        name: String,
        value: i32,
        nested: NestedConfig,
    }

    #[derive(Serialize, Deserialize)]
    struct NestedConfig {
        enabled: bool,
        items: Vec<String>,
    }

    #[test]
    fn test_no_changes() {
        let config = TestConfig {
            name: "test".to_string(),
            value: 42,
            nested: NestedConfig {
                enabled: true,
                items: vec!["a".to_string()],
            },
        };
        let changes = diff_config(&config, &config);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_simple_change() {
        let prev = TestConfig {
            name: "test".to_string(),
            value: 42,
            nested: NestedConfig {
                enabled: true,
                items: vec![],
            },
        };
        let next = TestConfig {
            name: "test".to_string(),
            value: 100,
            nested: NestedConfig {
                enabled: true,
                items: vec![],
            },
        };
        let changes = diff_config(&prev, &next);
        assert_eq!(changes, vec!["value"]);
    }

    #[test]
    fn test_nested_change() {
        let prev = TestConfig {
            name: "test".to_string(),
            value: 42,
            nested: NestedConfig {
                enabled: true,
                items: vec![],
            },
        };
        let next = TestConfig {
            name: "test".to_string(),
            value: 42,
            nested: NestedConfig {
                enabled: false,
                items: vec![],
            },
        };
        let changes = diff_config(&prev, &next);
        assert_eq!(changes, vec!["nested.enabled"]);
    }

    #[test]
    fn test_array_change() {
        let prev = TestConfig {
            name: "test".to_string(),
            value: 42,
            nested: NestedConfig {
                enabled: true,
                items: vec!["a".to_string()],
            },
        };
        let next = TestConfig {
            name: "test".to_string(),
            value: 42,
            nested: NestedConfig {
                enabled: true,
                items: vec!["a".to_string(), "b".to_string()],
            },
        };
        let changes = diff_config(&prev, &next);
        assert_eq!(changes, vec!["nested.items"]);
    }

    #[test]
    fn test_multiple_changes() {
        let prev = TestConfig {
            name: "test".to_string(),
            value: 42,
            nested: NestedConfig {
                enabled: true,
                items: vec![],
            },
        };
        let next = TestConfig {
            name: "changed".to_string(),
            value: 100,
            nested: NestedConfig {
                enabled: true,
                items: vec![],
            },
        };
        let changes = diff_config(&prev, &next);
        assert!(changes.contains(&"name".to_string()));
        assert!(changes.contains(&"value".to_string()));
        assert_eq!(changes.len(), 2);
    }

    #[test]
    fn test_added_field() {
        use serde_json::json;

        let prev = json!({ "name": "test" });
        let next = json!({ "name": "test", "new_field": "value" });

        let mut changes = Vec::new();
        diff_values(&prev, &next, "", &mut changes);
        assert!(changes.contains(&"new_field".to_string()));
    }

    #[test]
    fn test_removed_field() {
        use serde_json::json;

        let prev = json!({ "name": "test", "old_field": "value" });
        let next = json!({ "name": "test" });

        let mut changes = Vec::new();
        diff_values(&prev, &next, "", &mut changes);
        assert!(changes.contains(&"old_field".to_string()));
    }
}
