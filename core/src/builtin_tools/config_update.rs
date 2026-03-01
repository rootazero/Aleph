//! ConfigUpdateTool — LLM write tool for updating Aleph configuration
//!
//! Wraps [`ConfigPatcher`] for the Agent Loop. The key design point is
//! `requires_confirmation() = true` — this causes the Agent Loop to pause
//! and ask user permission before executing any config change.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::config::patcher::{ConfigPatcher, HealthCheckResult, PatchRequest};
use crate::error::Result;
use crate::tools::AlephTool;

use super::{notify_tool_result, notify_tool_start};

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for the config_update tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ConfigUpdateArgs {
    /// Target config path, e.g. "providers.deepseek", "memory", "dispatcher"
    pub path: String,
    /// Config values to set/update (JSON object, merged into existing config).
    /// Do NOT include api_key here — use the secrets field instead.
    pub values: serde_json::Value,
    /// Sensitive fields to store in SecretVault. Key = field name, Value = secret value.
    #[serde(default)]
    pub secrets: HashMap<String, String>,
    /// If true, only validate and preview without applying.
    #[serde(default)]
    pub dry_run: bool,
}

/// Output from the config_update tool
#[derive(Debug, Clone, Serialize)]
pub struct ConfigUpdateOutput {
    /// Whether the update was successful
    pub success: bool,
    /// Human-readable summary of the operation
    pub summary: String,
    /// List of dot-paths for fields that changed
    pub changed_fields: Vec<String>,
    /// Health check result (if performed)
    pub health_check: Option<String>,
    /// Non-fatal warnings
    pub warnings: Vec<String>,
}

// =============================================================================
// ConfigUpdateTool
// =============================================================================

/// Write tool for LLM to update Aleph's configuration.
///
/// Wraps [`ConfigPatcher`] and requires user confirmation before applying changes.
pub struct ConfigUpdateTool {
    patcher: Arc<ConfigPatcher>,
}

impl Clone for ConfigUpdateTool {
    fn clone(&self) -> Self {
        Self {
            patcher: Arc::clone(&self.patcher),
        }
    }
}

impl ConfigUpdateTool {
    /// Create a new ConfigUpdateTool with a shared ConfigPatcher reference
    pub fn new(patcher: Arc<ConfigPatcher>) -> Self {
        Self { patcher }
    }
}

// =============================================================================
// AlephTool implementation
// =============================================================================

#[async_trait]
impl AlephTool for ConfigUpdateTool {
    const NAME: &'static str = "config_update";
    const DESCRIPTION: &'static str = "Update Aleph configuration. Supports all config sections \
        (providers, memory, dispatcher, tools, policies, etc.). Sensitive fields (API keys, tokens) \
        are automatically encrypted in SecretVault. Changes require user confirmation before applying.";

    type Args = ConfigUpdateArgs;
    type Output = ConfigUpdateOutput;

    fn requires_confirmation(&self) -> bool {
        true
    }

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            r#"config_update(path="providers.deepseek", values={"model": "deepseek-chat", "base_url": "https://api.deepseek.com/v1"}, secrets={"api_key": "sk-xxx"}) — add a provider with secret"#.to_string(),
            r#"config_update(path="memory", values={"enabled": true, "max_facts": 10000}) — update memory settings"#.to_string(),
            r#"config_update(path="general", values={"language": "zh-CN"}) — update general settings"#.to_string(),
            r#"config_update(path="providers.openai", values={"model": "gpt-4o"}, dry_run=true) — preview changes without applying"#.to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let action = if args.dry_run { "Previewing" } else { "Updating" };
        notify_tool_start(
            Self::NAME,
            &format!("{} config: {}", action, args.path),
        );

        // Convert Args -> PatchRequest
        let request = PatchRequest {
            path: args.path.clone(),
            patch: args.values,
            secret_fields: args.secrets,
            health_check: !args.dry_run,
            dry_run: args.dry_run,
        };

        // Apply the patch
        let result = self.patcher.apply(request).await?;

        // Collect changed field paths from the diff
        let changed_fields: Vec<String> = result.diff.iter().map(|d| d.path.clone()).collect();

        // Build summary
        let summary = if changed_fields.is_empty() {
            format!("No changes needed for '{}'", args.path)
        } else if args.dry_run {
            format!(
                "[Dry run] Would change {} field(s) in '{}': {}",
                changed_fields.len(),
                args.path,
                changed_fields.join(", ")
            )
        } else {
            format!(
                "Updated {} field(s) in '{}': {}",
                changed_fields.len(),
                args.path,
                changed_fields.join(", ")
            )
        };

        // Convert HealthCheckResult to Option<String>
        let health_check = result.health_check.map(|hc| match hc {
            HealthCheckResult::Passed => "Passed".to_string(),
            HealthCheckResult::Failed { reason } => format!("Failed: {}", reason),
            HealthCheckResult::Skipped => "Skipped".to_string(),
        });

        notify_tool_result(Self::NAME, &summary, result.success);

        Ok(ConfigUpdateOutput {
            success: result.success,
            summary,
            changed_fields,
            health_check,
            warnings: result.warnings,
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_args_default_values() {
        // Deserialize with only required fields (path + values)
        let json = json!({
            "path": "providers.deepseek",
            "values": {"model": "deepseek-chat"}
        });

        let args: ConfigUpdateArgs = serde_json::from_value(json).unwrap();
        assert_eq!(args.path, "providers.deepseek");
        assert_eq!(args.values, json!({"model": "deepseek-chat"}));
        assert!(args.secrets.is_empty());
        assert!(!args.dry_run);
    }

    #[test]
    fn test_args_with_secrets() {
        let json = json!({
            "path": "providers.openai",
            "values": {"model": "gpt-4o"},
            "secrets": {"api_key": "sk-secret-123"},
            "dry_run": true
        });

        let args: ConfigUpdateArgs = serde_json::from_value(json).unwrap();
        assert_eq!(args.path, "providers.openai");
        assert_eq!(args.secrets.get("api_key").unwrap(), "sk-secret-123");
        assert!(args.dry_run);
    }

    #[test]
    fn test_output_serialization() {
        let output = ConfigUpdateOutput {
            success: true,
            summary: "Updated 2 field(s) in 'memory'".to_string(),
            changed_fields: vec!["memory.enabled".to_string(), "memory.max_facts".to_string()],
            health_check: Some("Passed".to_string()),
            warnings: vec!["Backup warning: disk full".to_string()],
        };

        let json = serde_json::to_value(&output).unwrap();
        assert_eq!(json["success"], true);
        assert_eq!(json["summary"], "Updated 2 field(s) in 'memory'");
        assert_eq!(json["changed_fields"].as_array().unwrap().len(), 2);
        assert_eq!(json["health_check"], "Passed");
        assert_eq!(json["warnings"].as_array().unwrap().len(), 1);
    }
}
