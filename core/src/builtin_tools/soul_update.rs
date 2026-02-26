//! Soul Update Tool — AI self-evolution through identity modification
//!
//! Allows the AI to update its own SoulManifest based on interactions.
//! Changes are gradual — the tool enforces incremental modifications
//! rather than wholesale identity rewrites.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::Result;
use crate::thinker::soul::SoulManifest;
use crate::tools::AlephTool;

/// Which field of the SoulManifest to update
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SoulField {
    /// Core identity declaration (first-person)
    Identity,
    /// Communication tone
    Tone,
    /// Behavioral directives (positive guidance)
    Directives,
    /// Anti-patterns (what to avoid)
    AntiPatterns,
    /// Domain expertise areas
    Expertise,
    /// Custom prompt addendum
    Addendum,
}

impl std::fmt::Display for SoulField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Identity => write!(f, "identity"),
            Self::Tone => write!(f, "tone"),
            Self::Directives => write!(f, "directives"),
            Self::AntiPatterns => write!(f, "anti_patterns"),
            Self::Expertise => write!(f, "expertise"),
            Self::Addendum => write!(f, "addendum"),
        }
    }
}

/// What operation to perform on the field
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SoulOperation {
    /// Replace the field value entirely
    Set,
    /// Append to list fields, or concatenate for string fields
    Append,
    /// Remove an item from list fields
    Remove,
}

impl std::fmt::Display for SoulOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Set => write!(f, "set"),
            Self::Append => write!(f, "append"),
            Self::Remove => write!(f, "remove"),
        }
    }
}

/// Arguments for the soul_update tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SoulUpdateArgs {
    /// Which field to update
    pub field: SoulField,
    /// What operation to perform
    pub operation: SoulOperation,
    /// The value to set, append, or remove
    pub value: String,
    /// Reason for this change (for auditability)
    pub reason: String,
}

/// Output from the soul_update tool
#[derive(Debug, Clone, Serialize)]
pub struct SoulUpdateOutput {
    /// Whether the update succeeded
    pub success: bool,
    /// Human-readable result message
    pub message: String,
    /// Which field was updated
    pub field: String,
    /// What operation was performed
    pub operation: String,
}

/// Tool that allows the AI to update its own SoulManifest
#[derive(Clone)]
pub struct SoulUpdateTool {
    soul_path: std::path::PathBuf,
}

impl SoulUpdateTool {
    /// Create a new SoulUpdateTool pointing at the given soul file
    pub fn new(soul_path: std::path::PathBuf) -> Self {
        Self { soul_path }
    }

    /// Apply an operation to the soul manifest
    fn apply_operation(
        manifest: &mut SoulManifest,
        field: &SoulField,
        operation: &SoulOperation,
        value: &str,
    ) -> std::result::Result<String, String> {
        match (field, operation) {
            // Identity: string field
            (SoulField::Identity, SoulOperation::Set) => {
                manifest.identity = value.to_string();
                Ok("Identity updated".to_string())
            }
            (SoulField::Identity, SoulOperation::Append) => {
                if !manifest.identity.is_empty() {
                    manifest.identity.push(' ');
                }
                manifest.identity.push_str(value);
                Ok("Identity appended".to_string())
            }
            (SoulField::Identity, SoulOperation::Remove) => {
                Err("Cannot remove from identity string. Use 'set' to replace it.".to_string())
            }

            // Tone: string field (nested in voice)
            (SoulField::Tone, SoulOperation::Set) => {
                manifest.voice.tone = value.to_string();
                Ok("Tone updated".to_string())
            }
            (SoulField::Tone, SoulOperation::Append) => {
                if !manifest.voice.tone.is_empty() {
                    manifest.voice.tone.push_str(", ");
                }
                manifest.voice.tone.push_str(value);
                Ok("Tone appended".to_string())
            }
            (SoulField::Tone, SoulOperation::Remove) => {
                Err("Cannot remove from tone string. Use 'set' to replace it.".to_string())
            }

            // Directives: list field
            (SoulField::Directives, SoulOperation::Set) => {
                manifest.directives = vec![value.to_string()];
                Ok("Directives set to single item".to_string())
            }
            (SoulField::Directives, SoulOperation::Append) => {
                if manifest.directives.contains(&value.to_string()) {
                    return Ok("Directive already exists, skipping".to_string());
                }
                manifest.directives.push(value.to_string());
                Ok(format!(
                    "Directive appended (now {} total)",
                    manifest.directives.len()
                ))
            }
            (SoulField::Directives, SoulOperation::Remove) => {
                let before = manifest.directives.len();
                manifest.directives.retain(|d| d != value);
                let after = manifest.directives.len();
                if before == after {
                    Ok("Directive not found, no change".to_string())
                } else {
                    Ok(format!(
                        "Directive removed (now {} total)",
                        manifest.directives.len()
                    ))
                }
            }

            // AntiPatterns: list field
            (SoulField::AntiPatterns, SoulOperation::Set) => {
                manifest.anti_patterns = vec![value.to_string()];
                Ok("Anti-patterns set to single item".to_string())
            }
            (SoulField::AntiPatterns, SoulOperation::Append) => {
                if manifest.anti_patterns.contains(&value.to_string()) {
                    return Ok("Anti-pattern already exists, skipping".to_string());
                }
                manifest.anti_patterns.push(value.to_string());
                Ok(format!(
                    "Anti-pattern appended (now {} total)",
                    manifest.anti_patterns.len()
                ))
            }
            (SoulField::AntiPatterns, SoulOperation::Remove) => {
                let before = manifest.anti_patterns.len();
                manifest.anti_patterns.retain(|a| a != value);
                let after = manifest.anti_patterns.len();
                if before == after {
                    Ok("Anti-pattern not found, no change".to_string())
                } else {
                    Ok(format!(
                        "Anti-pattern removed (now {} total)",
                        manifest.anti_patterns.len()
                    ))
                }
            }

            // Expertise: list field
            (SoulField::Expertise, SoulOperation::Set) => {
                manifest.expertise = vec![value.to_string()];
                Ok("Expertise set to single item".to_string())
            }
            (SoulField::Expertise, SoulOperation::Append) => {
                if manifest.expertise.contains(&value.to_string()) {
                    return Ok("Expertise already exists, skipping".to_string());
                }
                manifest.expertise.push(value.to_string());
                Ok(format!(
                    "Expertise appended (now {} total)",
                    manifest.expertise.len()
                ))
            }
            (SoulField::Expertise, SoulOperation::Remove) => {
                let before = manifest.expertise.len();
                manifest.expertise.retain(|e| e != value);
                let after = manifest.expertise.len();
                if before == after {
                    Ok("Expertise not found, no change".to_string())
                } else {
                    Ok(format!(
                        "Expertise removed (now {} total)",
                        manifest.expertise.len()
                    ))
                }
            }

            // Addendum: optional string field
            (SoulField::Addendum, SoulOperation::Set) => {
                manifest.addendum = Some(value.to_string());
                Ok("Addendum updated".to_string())
            }
            (SoulField::Addendum, SoulOperation::Append) => {
                match &mut manifest.addendum {
                    Some(existing) => {
                        existing.push('\n');
                        existing.push_str(value);
                    }
                    None => {
                        manifest.addendum = Some(value.to_string());
                    }
                }
                Ok("Addendum appended".to_string())
            }
            (SoulField::Addendum, SoulOperation::Remove) => {
                manifest.addendum = None;
                Ok("Addendum cleared".to_string())
            }
        }
    }
}

#[async_trait]
impl AlephTool for SoulUpdateTool {
    const NAME: &'static str = "soul_update";
    const DESCRIPTION: &'static str =
        "Update your soul manifest. Use when you learn something new about yourself \
         or want to refine your personality based on interactions. Changes are gradual \
         — never rewrite your entire identity at once.";

    type Args = SoulUpdateArgs;
    type Output = SoulUpdateOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "soul_update(field='directives', operation='append', value='Always explain trade-offs when suggesting solutions', reason='User prefers seeing pros and cons')"
                .to_string(),
            "soul_update(field='anti_patterns', operation='append', value='Never use emojis in code comments', reason='User corrected me about emoji usage')"
                .to_string(),
            "soul_update(field='expertise', operation='append', value='Kubernetes cluster management', reason='Discovered through helping with k8s deployments')"
                .to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(
            field = %args.field,
            operation = %args.operation,
            reason = %args.reason,
            "Soul update requested"
        );

        // Load existing soul or use default
        let mut manifest = if self.soul_path.exists() {
            SoulManifest::from_file(&self.soul_path).unwrap_or_default()
        } else {
            SoulManifest::default()
        };

        // Apply the operation
        let result = Self::apply_operation(&mut manifest, &args.field, &args.operation, &args.value);

        match result {
            Ok(message) => {
                // Save back to file
                if let Err(e) = manifest.save_to_file(&self.soul_path) {
                    return Ok(SoulUpdateOutput {
                        success: false,
                        message: format!("Operation succeeded but save failed: {}", e),
                        field: args.field.to_string(),
                        operation: args.operation.to_string(),
                    });
                }

                info!(
                    field = %args.field,
                    operation = %args.operation,
                    message = %message,
                    "Soul updated successfully"
                );

                Ok(SoulUpdateOutput {
                    success: true,
                    message,
                    field: args.field.to_string(),
                    operation: args.operation.to_string(),
                })
            }
            Err(message) => Ok(SoulUpdateOutput {
                success: false,
                message,
                field: args.field.to_string(),
                operation: args.operation.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ========== Serialization Tests ==========

    #[test]
    fn test_soul_field_serialization_roundtrip() {
        let fields = vec![
            SoulField::Identity,
            SoulField::Tone,
            SoulField::Directives,
            SoulField::AntiPatterns,
            SoulField::Expertise,
            SoulField::Addendum,
        ];

        for field in fields {
            let json = serde_json::to_string(&field).unwrap();
            let parsed: SoulField = serde_json::from_str(&json).unwrap();
            // Verify roundtrip by serializing again
            let json2 = serde_json::to_string(&parsed).unwrap();
            assert_eq!(json, json2, "Roundtrip failed for {:?}", field);
        }
    }

    #[test]
    fn test_soul_field_snake_case() {
        assert_eq!(serde_json::to_string(&SoulField::Identity).unwrap(), "\"identity\"");
        assert_eq!(serde_json::to_string(&SoulField::Tone).unwrap(), "\"tone\"");
        assert_eq!(serde_json::to_string(&SoulField::Directives).unwrap(), "\"directives\"");
        assert_eq!(serde_json::to_string(&SoulField::AntiPatterns).unwrap(), "\"anti_patterns\"");
        assert_eq!(serde_json::to_string(&SoulField::Expertise).unwrap(), "\"expertise\"");
        assert_eq!(serde_json::to_string(&SoulField::Addendum).unwrap(), "\"addendum\"");
    }

    #[test]
    fn test_soul_operation_serialization_roundtrip() {
        let ops = vec![
            SoulOperation::Set,
            SoulOperation::Append,
            SoulOperation::Remove,
        ];

        for op in ops {
            let json = serde_json::to_string(&op).unwrap();
            let parsed: SoulOperation = serde_json::from_str(&json).unwrap();
            let json2 = serde_json::to_string(&parsed).unwrap();
            assert_eq!(json, json2, "Roundtrip failed for {:?}", op);
        }
    }

    #[test]
    fn test_soul_operation_snake_case() {
        assert_eq!(serde_json::to_string(&SoulOperation::Set).unwrap(), "\"set\"");
        assert_eq!(serde_json::to_string(&SoulOperation::Append).unwrap(), "\"append\"");
        assert_eq!(serde_json::to_string(&SoulOperation::Remove).unwrap(), "\"remove\"");
    }

    #[test]
    fn test_soul_update_args_deserialization() {
        let json = r#"{
            "field": "directives",
            "operation": "append",
            "value": "Always explain trade-offs",
            "reason": "User preference discovered"
        }"#;

        let args: SoulUpdateArgs = serde_json::from_str(json).unwrap();
        assert!(matches!(args.field, SoulField::Directives));
        assert!(matches!(args.operation, SoulOperation::Append));
        assert_eq!(args.value, "Always explain trade-offs");
        assert_eq!(args.reason, "User preference discovered");
    }

    #[test]
    fn test_soul_update_args_anti_patterns() {
        let json = r#"{
            "field": "anti_patterns",
            "operation": "append",
            "value": "Never use emojis",
            "reason": "User corrected me"
        }"#;

        let args: SoulUpdateArgs = serde_json::from_str(json).unwrap();
        assert!(matches!(args.field, SoulField::AntiPatterns));
        assert!(matches!(args.operation, SoulOperation::Append));
    }

    // ========== Tool Metadata Tests ==========

    #[test]
    fn test_tool_name_and_description() {
        assert_eq!(SoulUpdateTool::NAME, "soul_update");
        assert!(SoulUpdateTool::DESCRIPTION.contains("soul manifest"));
        assert!(SoulUpdateTool::DESCRIPTION.contains("gradual"));
    }

    #[test]
    fn test_tool_examples() {
        let tool = SoulUpdateTool::new(PathBuf::from("/tmp/test_soul.yaml"));
        let examples = tool.examples();
        assert!(examples.is_some());
        let examples = examples.unwrap();
        assert_eq!(examples.len(), 3);
        assert!(examples[0].contains("directives"));
        assert!(examples[1].contains("anti_patterns"));
        assert!(examples[2].contains("expertise"));
    }

    #[test]
    fn test_tool_definition() {
        let tool = SoulUpdateTool::new(PathBuf::from("/tmp/test_soul.yaml"));
        let def = AlephTool::definition(&tool);
        assert_eq!(def.name, "soul_update");
        assert!(def.llm_context.is_some());
        let context = def.llm_context.unwrap();
        assert!(context.contains("Usage Examples"));
    }

    // ========== Apply Operation Unit Tests ==========

    #[test]
    fn test_apply_set_identity() {
        let mut manifest = SoulManifest::default();
        let result = SoulUpdateTool::apply_operation(
            &mut manifest,
            &SoulField::Identity,
            &SoulOperation::Set,
            "I am a coding assistant specialized in Rust",
        );
        assert!(result.is_ok());
        assert_eq!(manifest.identity, "I am a coding assistant specialized in Rust");
    }

    #[test]
    fn test_apply_append_identity() {
        let mut manifest = SoulManifest {
            identity: "I am a coding assistant".to_string(),
            ..Default::default()
        };
        let result = SoulUpdateTool::apply_operation(
            &mut manifest,
            &SoulField::Identity,
            &SoulOperation::Append,
            "specialized in Rust",
        );
        assert!(result.is_ok());
        assert_eq!(manifest.identity, "I am a coding assistant specialized in Rust");
    }

    #[test]
    fn test_apply_remove_identity_errors() {
        let mut manifest = SoulManifest::default();
        let result = SoulUpdateTool::apply_operation(
            &mut manifest,
            &SoulField::Identity,
            &SoulOperation::Remove,
            "anything",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_apply_append_directive() {
        let mut manifest = SoulManifest {
            directives: vec!["Be helpful".to_string()],
            ..Default::default()
        };
        let result = SoulUpdateTool::apply_operation(
            &mut manifest,
            &SoulField::Directives,
            &SoulOperation::Append,
            "Explain trade-offs",
        );
        assert!(result.is_ok());
        assert_eq!(manifest.directives.len(), 2);
        assert_eq!(manifest.directives[1], "Explain trade-offs");
    }

    #[test]
    fn test_apply_append_duplicate_directive_skips() {
        let mut manifest = SoulManifest {
            directives: vec!["Be helpful".to_string()],
            ..Default::default()
        };
        let result = SoulUpdateTool::apply_operation(
            &mut manifest,
            &SoulField::Directives,
            &SoulOperation::Append,
            "Be helpful",
        );
        assert!(result.is_ok());
        assert_eq!(manifest.directives.len(), 1); // No duplicate
        assert!(result.unwrap().contains("already exists"));
    }

    #[test]
    fn test_apply_remove_directive() {
        let mut manifest = SoulManifest {
            directives: vec!["Be helpful".to_string(), "Be precise".to_string()],
            ..Default::default()
        };
        let result = SoulUpdateTool::apply_operation(
            &mut manifest,
            &SoulField::Directives,
            &SoulOperation::Remove,
            "Be helpful",
        );
        assert!(result.is_ok());
        assert_eq!(manifest.directives.len(), 1);
        assert_eq!(manifest.directives[0], "Be precise");
    }

    #[test]
    fn test_apply_set_tone() {
        let mut manifest = SoulManifest::default();
        let result = SoulUpdateTool::apply_operation(
            &mut manifest,
            &SoulField::Tone,
            &SoulOperation::Set,
            "warm and encouraging",
        );
        assert!(result.is_ok());
        assert_eq!(manifest.voice.tone, "warm and encouraging");
    }

    #[test]
    fn test_apply_append_expertise() {
        let mut manifest = SoulManifest {
            expertise: vec!["Rust".to_string()],
            ..Default::default()
        };
        let result = SoulUpdateTool::apply_operation(
            &mut manifest,
            &SoulField::Expertise,
            &SoulOperation::Append,
            "Kubernetes",
        );
        assert!(result.is_ok());
        assert_eq!(manifest.expertise.len(), 2);
        assert_eq!(manifest.expertise[1], "Kubernetes");
    }

    #[test]
    fn test_apply_set_addendum() {
        let mut manifest = SoulManifest::default();
        let result = SoulUpdateTool::apply_operation(
            &mut manifest,
            &SoulField::Addendum,
            &SoulOperation::Set,
            "Remember to be patient",
        );
        assert!(result.is_ok());
        assert_eq!(manifest.addendum, Some("Remember to be patient".to_string()));
    }

    #[test]
    fn test_apply_remove_addendum_clears() {
        let mut manifest = SoulManifest {
            addendum: Some("Old addendum".to_string()),
            ..Default::default()
        };
        let result = SoulUpdateTool::apply_operation(
            &mut manifest,
            &SoulField::Addendum,
            &SoulOperation::Remove,
            "", // value ignored for addendum remove
        );
        assert!(result.is_ok());
        assert!(manifest.addendum.is_none());
    }

    #[test]
    fn test_apply_append_anti_pattern() {
        let mut manifest = SoulManifest::default();
        let result = SoulUpdateTool::apply_operation(
            &mut manifest,
            &SoulField::AntiPatterns,
            &SoulOperation::Append,
            "Never use emojis in code",
        );
        assert!(result.is_ok());
        assert_eq!(manifest.anti_patterns.len(), 1);
        assert_eq!(manifest.anti_patterns[0], "Never use emojis in code");
    }

    // ========== Async Integration Tests ==========

    #[tokio::test]
    async fn test_async_append_directive_to_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let soul_path = dir.path().join("soul.yaml");

        // Create initial soul
        let initial = SoulManifest {
            identity: "I am a test assistant".to_string(),
            directives: vec!["Be helpful".to_string()],
            ..Default::default()
        };
        initial.save_to_file(&soul_path).unwrap();

        // Use the tool to append a directive
        let tool = SoulUpdateTool::new(soul_path.clone());
        let args = SoulUpdateArgs {
            field: SoulField::Directives,
            operation: SoulOperation::Append,
            value: "Always show examples".to_string(),
            reason: "User likes examples".to_string(),
        };

        let output = tool.call(args).await.unwrap();
        assert!(output.success);
        assert_eq!(output.field, "directives");
        assert_eq!(output.operation, "append");

        // Verify the file was updated
        let reloaded = SoulManifest::from_file(&soul_path).unwrap();
        assert_eq!(reloaded.directives.len(), 2);
        assert_eq!(reloaded.directives[0], "Be helpful");
        assert_eq!(reloaded.directives[1], "Always show examples");
        assert_eq!(reloaded.identity, "I am a test assistant");
    }

    #[tokio::test]
    async fn test_async_set_identity_in_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let soul_path = dir.path().join("soul.yaml");

        // No existing file — should start from default
        let tool = SoulUpdateTool::new(soul_path.clone());
        let args = SoulUpdateArgs {
            field: SoulField::Identity,
            operation: SoulOperation::Set,
            value: "I am Aleph, a personal AI assistant".to_string(),
            reason: "Initial identity setup".to_string(),
        };

        let output = tool.call(args).await.unwrap();
        assert!(output.success);
        assert_eq!(output.field, "identity");
        assert_eq!(output.operation, "set");
        assert!(output.message.contains("Identity updated"));

        // Verify the file was created and contains the identity
        let reloaded = SoulManifest::from_file(&soul_path).unwrap();
        assert_eq!(reloaded.identity, "I am Aleph, a personal AI assistant");
    }

    #[tokio::test]
    async fn test_async_invalid_operation_returns_failure() {
        let dir = tempfile::tempdir().unwrap();
        let soul_path = dir.path().join("soul.yaml");

        let tool = SoulUpdateTool::new(soul_path);
        let args = SoulUpdateArgs {
            field: SoulField::Identity,
            operation: SoulOperation::Remove,
            value: "anything".to_string(),
            reason: "Testing error case".to_string(),
        };

        let output = tool.call(args).await.unwrap();
        assert!(!output.success);
        assert!(output.message.contains("Cannot remove"));
    }

    // ========== Display Tests ==========

    #[test]
    fn test_soul_field_display() {
        assert_eq!(format!("{}", SoulField::Identity), "identity");
        assert_eq!(format!("{}", SoulField::Tone), "tone");
        assert_eq!(format!("{}", SoulField::Directives), "directives");
        assert_eq!(format!("{}", SoulField::AntiPatterns), "anti_patterns");
        assert_eq!(format!("{}", SoulField::Expertise), "expertise");
        assert_eq!(format!("{}", SoulField::Addendum), "addendum");
    }

    #[test]
    fn test_soul_operation_display() {
        assert_eq!(format!("{}", SoulOperation::Set), "set");
        assert_eq!(format!("{}", SoulOperation::Append), "append");
        assert_eq!(format!("{}", SoulOperation::Remove), "remove");
    }
}
