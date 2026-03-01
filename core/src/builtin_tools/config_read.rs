//! ConfigReadTool — LLM read-only tool for inspecting Aleph configuration
//!
//! Allows the LLM to read current configuration values with sensitive fields
//! automatically masked. Also provides JSON Schema for each section to help
//! the LLM understand valid field names and types.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::debug;

use crate::config::{generate_config_schema_json, Config};
use crate::error::Result;
use crate::tools::AlephTool;

use super::{notify_tool_result, notify_tool_start};

// =============================================================================
// Sensitive field names to mask
// =============================================================================

/// Field names whose string values should be replaced with "***".
/// Uses exact match (not substring) to avoid false positives on fields like `secret_name`.
const SENSITIVE_FIELDS: &[&str] = &[
    "api_key",
    "token",
    "password",
    "client_secret",
    "secret_key",
    "service_account_token_env",
];

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for the config_read tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ConfigReadArgs {
    /// Config section path. Examples: "providers", "memory", "general", "providers.openai"
    /// Use "all" for a summary of all top-level sections.
    pub path: String,
}

/// Output from the config_read tool
#[derive(Debug, Clone, Serialize)]
pub struct ConfigReadOutput {
    /// Config values with sensitive fields masked
    pub values: Value,
    /// JSON Schema for this section (if available)
    pub schema: Option<Value>,
}

// =============================================================================
// ConfigReadTool
// =============================================================================

/// Read-only tool for LLM to inspect Aleph's current configuration
pub struct ConfigReadTool {
    config: Arc<RwLock<Config>>,
}

impl Clone for ConfigReadTool {
    fn clone(&self) -> Self {
        Self {
            config: Arc::clone(&self.config),
        }
    }
}

impl ConfigReadTool {
    /// Create a new ConfigReadTool with a shared config reference
    pub fn new(config: Arc<RwLock<Config>>) -> Self {
        Self { config }
    }
}

// =============================================================================
// Sensitive field masking
// =============================================================================

/// Check if a field name matches any sensitive pattern
fn is_sensitive_field(key: &str) -> bool {
    let lower = key.to_lowercase();
    SENSITIVE_FIELDS.iter().any(|s| lower == *s)
}

/// Recursively mask sensitive fields in a JSON value.
/// Only masks non-empty string values.
pub fn mask_sensitive_fields(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, val) in map.iter_mut() {
                if is_sensitive_field(key) {
                    if let Value::String(s) = val {
                        if !s.is_empty() {
                            *val = Value::String("***".to_string());
                        }
                    }
                }
                // Always recurse into children
                mask_sensitive_fields(val);
            }
        }
        Value::Array(arr) => {
            for item in arr.iter_mut() {
                mask_sensitive_fields(item);
            }
        }
        _ => {}
    }
}

// =============================================================================
// Sub-schema extraction
// =============================================================================

/// Extract the JSON Schema properties for a given dot-separated path.
///
/// Navigates `properties.{segment}` at each level, resolving `$ref` if needed.
fn extract_sub_schema(full_schema: &Value, path: &str) -> Option<Value> {
    let segments: Vec<&str> = path.split('.').collect();
    let mut current = full_schema.clone();

    for segment in &segments {
        // Resolve $ref if present
        current = resolve_ref(full_schema, &current);

        // Navigate into properties.{segment}
        if let Some(prop) = current
            .get("properties")
            .and_then(|p| p.get(*segment))
        {
            current = prop.clone();
        } else {
            return None;
        }
    }

    // Final resolve
    current = resolve_ref(full_schema, &current);
    Some(current)
}

/// Resolve a JSON Schema `$ref` pointer (e.g., `#/definitions/Foo`).
fn resolve_ref(root: &Value, schema: &Value) -> Value {
    if let Some(Value::String(ref_path)) = schema.get("$ref") {
        if let Some(stripped) = ref_path.strip_prefix("#/") {
            let parts: Vec<&str> = stripped.split('/').collect();
            let mut node = root;
            for part in parts {
                if let Some(child) = node.get(part) {
                    node = child;
                } else {
                    return schema.clone();
                }
            }
            return node.clone();
        }
    }
    schema.clone()
}

// =============================================================================
// AlephTool implementation
// =============================================================================

#[async_trait]
impl AlephTool for ConfigReadTool {
    const NAME: &'static str = "config_read";
    const DESCRIPTION: &'static str = "Read current Aleph configuration. Returns config values \
        with sensitive fields masked. Also returns JSON Schema to help understand valid field \
        names and types.";

    type Args = ConfigReadArgs;
    type Output = ConfigReadOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            r#"config_read(path="all") — list all top-level config sections"#.to_string(),
            r#"config_read(path="providers") — show all AI provider configs"#.to_string(),
            r#"config_read(path="memory") — show memory system settings"#.to_string(),
            r#"config_read(path="providers.openai") — show a specific provider"#.to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        notify_tool_start(Self::NAME, &format!("path={}", args.path));

        let config = self.config.read().await;
        let full_schema = generate_config_schema_json();

        let path = args.path.trim();

        let output = if path.eq_ignore_ascii_case("all") {
            // Return list of top-level section names
            let config_json = serde_json::to_value(&*config)?;
            let sections: Vec<String> = if let Value::Object(map) = &config_json {
                let mut keys: Vec<String> = map.keys().cloned().collect();
                keys.sort();
                keys
            } else {
                vec![]
            };

            debug!(sections = ?sections, "config_read: listing all sections");

            ConfigReadOutput {
                values: serde_json::to_value(&sections)?,
                schema: None,
            }
        } else {
            // Serialize full config, navigate to requested path, mask sensitive fields
            let config_json = serde_json::to_value(&*config)?;

            // Navigate to the requested path
            let segments: Vec<&str> = path.split('.').collect();
            let mut current = &config_json;
            for segment in &segments {
                match current.get(*segment) {
                    Some(child) => current = child,
                    None => {
                        let output = ConfigReadOutput {
                            values: Value::Null,
                            schema: None,
                        };
                        notify_tool_result(
                            Self::NAME,
                            &format!("path '{}' not found", path),
                            false,
                        );
                        return Ok(output);
                    }
                }
            }

            // Clone the sub-value and mask sensitive fields
            let mut sub_value = current.clone();
            mask_sensitive_fields(&mut sub_value);

            // Extract sub-schema
            let sub_schema = extract_sub_schema(&full_schema, path);

            debug!(path = path, "config_read: returning section");

            ConfigReadOutput {
                values: sub_value,
                schema: sub_schema,
            }
        };

        notify_tool_result(Self::NAME, &format!("path={}", args.path), true);
        Ok(output)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_sensitive_fields() {
        let mut val = serde_json::json!({
            "api_key": "sk-secret-123",
            "model": "gpt-4",
            "secret_name": "my_secret"
        });
        mask_sensitive_fields(&mut val);
        assert_eq!(val["api_key"], "***");
        assert_eq!(val["model"], "gpt-4");
        // secret_name is a vault reference, NOT a secret — should NOT be masked
        assert_eq!(val["secret_name"], "my_secret");
    }

    #[test]
    fn test_mask_empty_sensitive_field() {
        let mut val = serde_json::json!({
            "api_key": "",
            "token": "real-token"
        });
        mask_sensitive_fields(&mut val);
        // Empty string should NOT be masked
        assert_eq!(val["api_key"], "");
        // Non-empty should be masked
        assert_eq!(val["token"], "***");
    }

    #[test]
    fn test_mask_nested_arrays() {
        let mut val = serde_json::json!({
            "providers": [
                {
                    "name": "openai",
                    "api_key": "sk-123",
                    "token": "tok-456"
                },
                {
                    "name": "anthropic",
                    "api_key": "ant-789"
                }
            ]
        });
        mask_sensitive_fields(&mut val);
        assert_eq!(val["providers"][0]["name"], "openai");
        assert_eq!(val["providers"][0]["api_key"], "***");
        assert_eq!(val["providers"][0]["token"], "***");
        assert_eq!(val["providers"][1]["api_key"], "***");
        assert_eq!(val["providers"][1]["name"], "anthropic");
    }
}
