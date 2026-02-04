//! Markdown CLI Tool Adapter
//!
//! Implements AetherTool trait for Markdown-defined CLI tools.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{debug, info, warn};

use crate::dispatcher::{ToolCategory, ToolDefinition};
use crate::error::Result;
use crate::tools::AetherTool;

use super::parser::{extract_first_paragraph, extract_markdown_section};
use super::spec::{AetherSkillSpec, SandboxMode};

/// Dynamic CLI tool loaded from Markdown
#[derive(Clone)]
pub struct MarkdownCliTool {
    pub(crate) spec: AetherSkillSpec,
    /// Whether usage examples have been injected in current session
    context_injected: Arc<AtomicBool>,
}

impl MarkdownCliTool {
    /// Create a new Markdown CLI tool
    pub fn new(spec: AetherSkillSpec) -> Self {
        Self {
            spec,
            context_injected: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Build JSON Schema from input_hints
    fn build_dynamic_schema(&self) -> Value {
        let mut properties = serde_json::Map::new();
        let mut required_fields = Vec::new();

        // If input_hints exist, use them
        if let Some(aether) = &self.spec.metadata.aether {
            for (key, hint) in &aether.input_hints {
                let mut prop = serde_json::Map::new();

                // Validate and normalize type
                if let Some(hint_type) = &hint.hint_type {
                    let normalized_type = normalize_json_schema_type(hint_type);
                    prop.insert("type".to_string(), json!(normalized_type));
                }

                if let Some(pattern) = &hint.pattern {
                    prop.insert("pattern".to_string(), json!(pattern));
                }

                if let Some(values) = &hint.values {
                    prop.insert("enum".to_string(), json!(values));
                }

                if let Some(desc) = &hint.description {
                    prop.insert("description".to_string(), json!(desc));
                }

                properties.insert(key.clone(), Value::Object(prop));

                // Add to required list if not explicitly optional
                if !hint.optional {
                    required_fields.push(key.clone());
                }
            }
        }

        // Fallback: use args array (safer than single command string)
        if properties.is_empty() {
            properties.insert(
                "args".to_string(),
                json!({
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Command-line arguments to pass to the tool (as separate strings for safety)"
                }),
            );
            required_fields.push("args".to_string());
        }

        json!({
            "type": "object",
            "properties": properties,
            "required": required_fields,
            "additionalProperties": false
        })
    }

    pub(crate) fn get_sandbox_mode(&self) -> SandboxMode {
        self.spec
            .metadata
            .aether
            .as_ref()
            .map(|a| a.security.sandbox.clone())
            .unwrap_or(SandboxMode::Host)
    }
}

/// Normalize type names to valid JSON Schema types
fn normalize_json_schema_type(hint_type: &str) -> &str {
    match hint_type.to_lowercase().as_str() {
        // Aliases first
        "str" | "text" => "string",
        "int" => "integer",
        "num" | "float" => "number",
        "bool" => "boolean",
        "arr" | "list" => "array",
        "obj" | "dict" => "object",
        // Already valid (exact matches)
        "string" | "integer" | "number" | "boolean" | "array" | "object" => hint_type,
        // Unknown: default to string for safety
        _ => {
            warn!("Unknown type hint '{}', defaulting to 'string'", hint_type);
            "string"
        }
    }
}

/// Generic output for Markdown CLI tools
#[derive(Debug, Serialize)]
pub struct MarkdownToolOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

#[async_trait]
impl AetherTool for MarkdownCliTool {
    const NAME: &'static str = "dynamic"; // Overridden by definition()
    const DESCRIPTION: &'static str = "Dynamic Markdown skill";

    type Args = Value; // Accept any JSON
    type Output = MarkdownToolOutput;

    fn definition(&self) -> ToolDefinition {
        let schema = self.build_dynamic_schema();

        // Extract examples section for LLM context
        let llm_context = extract_markdown_section(&self.spec.markdown_content, "Examples")
            .or_else(|| {
                // Fallback: use first paragraph
                Some(extract_first_paragraph(&self.spec.markdown_content))
            });

        let mut def = ToolDefinition::new(
            &self.spec.name,
            &self.spec.description,
            schema,
            ToolCategory::Skills,
        )
        .with_confirmation(self.requires_confirmation());

        if let Some(ctx) = llm_context {
            def = def.with_llm_context(ctx);
        }

        def
    }

    fn requires_confirmation(&self) -> bool {
        if let Some(aether) = &self.spec.metadata.aether {
            matches!(
                aether.security.confirmation,
                super::spec::ConfirmationMode::Always
                    | super::spec::ConfirmationMode::Write
            )
        } else {
            false // OpenClaw skills default to no confirmation
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Build CLI command from args
        let cli_args = self.args_to_cli(&args).map_err(|e| {
            crate::error::AetherError::IoError(
                format!("[{}] Failed to convert args to CLI: {}", self.spec.name, e)
            )
        })?;

        // Execute based on sandbox mode
        let output = match self.get_sandbox_mode() {
            SandboxMode::Host => self.execute_on_host(&cli_args).await.map_err(|e| {
                crate::error::AetherError::IoError(
                    format!("[{}] Host execution failed: {}", self.spec.name, e)
                )
            })?,
            SandboxMode::Docker => self.execute_in_docker(&cli_args).await.map_err(|e| {
                crate::error::AetherError::IoError(
                    format!("[{}] Docker execution failed: {}", self.spec.name, e)
                )
            })?,
            SandboxMode::VirtualFs => {
                return Err(crate::error::AetherError::IoError(
                    format!("[{}] VirtualFs sandbox not yet implemented", self.spec.name)
                ));
            }
        };

        Ok(output)
    }
}

impl MarkdownCliTool {
    /// Convert JSON args to CLI arguments (SAFETY-FIRST APPROACH)
    pub(crate) fn args_to_cli(&self, args: &Value) -> anyhow::Result<Vec<String>> {
        // ==========================================
        // PRIMARY MODE: Direct args array (RECOMMENDED)
        // ==========================================
        if let Some(args_array) = args.get("args").and_then(|v| v.as_array()) {
            return Ok(args_array
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect());
        }

        // ==========================================
        // FALLBACK MODE: Typed object (LIMITED USE)
        // ==========================================
        warn!(
            tool = %self.spec.name,
            "Using typed-object mode for CLI args. Consider using 'args' array for safety."
        );

        if let Some(obj) = args.as_object() {
            // Extract hints for ordering (if available)
            let hints = self
                .spec
                .metadata
                .aether
                .as_ref()
                .and_then(|a| Some(&a.input_hints));

            let mut cli_args = Vec::new();

            // Try to preserve order from input_hints definition
            if let Some(hints) = hints {
                // Ordered by hint definition
                for (key, _hint) in hints {
                    if let Some(value) = obj.get(key) {
                        self.append_flag_arg(&mut cli_args, key, value);
                    }
                }
                // Handle extra keys not in hints
                for (key, value) in obj {
                    if !hints.contains_key(key) {
                        self.append_flag_arg(&mut cli_args, key, value);
                    }
                }
            } else {
                // No hints: alphabetical order for determinism
                let mut keys: Vec<_> = obj.keys().collect();
                keys.sort();
                for key in keys {
                    self.append_flag_arg(&mut cli_args, key, obj.get(key).unwrap());
                }
            }

            return Ok(cli_args);
        }

        anyhow::bail!("Invalid args format: expected {{args: [...]}} or typed object");
    }

    /// Append a single flag argument (simple --key value format)
    fn append_flag_arg(&self, cli_args: &mut Vec<String>, key: &str, value: &Value) {
        let flag = format!("--{}", key.replace('_', "-"));
        cli_args.push(flag);

        // Skip value for boolean true
        if let Some(true) = value.as_bool() {
            return;
        }

        // Add value
        if let Some(s) = value.as_str() {
            cli_args.push(s.to_string());
        } else if let Some(arr) = value.as_array() {
            // Repeated flags: --tag v1 --tag v2
            for item in arr {
                if let Some(s) = item.as_str() {
                    cli_args.push(s.to_string());
                }
            }
        } else {
            cli_args.push(value.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_json_schema_type() {
        assert_eq!(normalize_json_schema_type("str"), "string");
        assert_eq!(normalize_json_schema_type("int"), "integer");
        assert_eq!(normalize_json_schema_type("bool"), "boolean");
        assert_eq!(normalize_json_schema_type("string"), "string");
        assert_eq!(normalize_json_schema_type("unknown"), "string");
    }

    #[test]
    fn test_args_to_cli_array_mode() {
        let spec = AetherSkillSpec {
            name: "test".to_string(),
            description: "test".to_string(),
            metadata: Default::default(),
            markdown_content: String::new(),
        };
        let tool = MarkdownCliTool::new(spec);

        let args = json!({"args": ["--repo", "owner/name", "--number", "123"]});
        let cli_args = tool.args_to_cli(&args).unwrap();

        assert_eq!(cli_args, vec!["--repo", "owner/name", "--number", "123"]);
    }
}
