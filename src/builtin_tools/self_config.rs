//! SelfConfigTool — structured access to identity files and config.toml
//!
//! Gives the LLM the ability to list, read, and write identity files
//! (SOUL.md, IDENTITY.md, AGENTS.md, TOOLS.md, MEMORY.md, HEARTBEAT.md)
//! and to read/update config.toml sections via the ConfigPatcher pipeline.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::sync::RwLock;

use super::{notify_tool_result, notify_tool_start};
use crate::config::patcher::{get_nested_value, ConfigPatcher, PatchRequest};
use crate::config::Config;
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

use super::error::ToolError;

// =============================================================================
// Args
// =============================================================================

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SelfConfigArgs {
    /// List all identity files and their status (exists, size)
    ListFiles,
    /// Read an identity file by name
    ReadFile {
        /// File name: MEMORY.md, SOUL.md, AGENTS.md, IDENTITY.md, TOOLS.md, or HEARTBEAT.md
        file_name: String,
    },
    /// Write content to an identity file (creates if not exists)
    WriteFile {
        /// File name (must be one of the allowed identity files)
        file_name: String,
        /// The full content to write to the file
        content: String,
    },
    /// Read a config section as JSON
    ReadConfig {
        /// Dot-path to config section, e.g. "memory", "providers.openai", "general"
        config_path: String,
    },
    /// Update a config section via deep-merge patch
    UpdateConfig {
        /// Dot-path to the config section to update
        config_path: String,
        /// JSON value to deep-merge into the section
        config_value: serde_json::Value,
        /// Preview changes without persisting (default: false)
        #[serde(default)]
        dry_run: bool,
    },
}

// =============================================================================
// Output
// =============================================================================

#[derive(Debug, Serialize)]
pub struct SelfConfigOutput {
    pub success: bool,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

// =============================================================================
// Tool Struct
// =============================================================================

#[derive(Clone)]
pub struct SelfConfigTool {
    agent_dir: PathBuf,
    agent_id: String,
    config: Option<Arc<RwLock<Config>>>,
    config_patcher: Option<Arc<ConfigPatcher>>,
}

impl SelfConfigTool {
    pub fn new(agent_id: impl Into<String>) -> Self {
        let agent_id = agent_id.into();
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
        let agent_dir = home.join(".aleph").join("agents").join(&agent_id);
        Self {
            agent_dir,
            agent_id,
            config: None,
            config_patcher: None,
        }
    }

    pub fn with_config(mut self, config: Arc<RwLock<Config>>) -> Self {
        self.config = Some(config);
        self
    }

    pub fn with_patcher(mut self, patcher: Arc<ConfigPatcher>) -> Self {
        self.config_patcher = Some(patcher);
        self
    }
}

// =============================================================================
// Security Validation
// =============================================================================

fn validate_file_name(name: &str) -> std::result::Result<(), ToolError> {
    use crate::thinker::identity_files::IDENTITY_FILE_NAMES;
    if !IDENTITY_FILE_NAMES.contains(&name) {
        return Err(ToolError::InvalidArgs(format!(
            "Invalid file name '{}'. Allowed: {:?}",
            name, IDENTITY_FILE_NAMES
        )));
    }
    if name.contains("..") || name.contains('/') || name.contains('\\') || name.contains('\0') {
        return Err(ToolError::InvalidArgs(
            "Invalid characters in file name".into(),
        ));
    }
    Ok(())
}

// =============================================================================
// Operation Implementations
// =============================================================================

impl SelfConfigTool {
    fn list_files(&self) -> Result<SelfConfigOutput> {
        use crate::thinker::identity_files::IDENTITY_FILE_NAMES;

        let mut entries = Vec::new();
        for &name in IDENTITY_FILE_NAMES {
            let path = self.agent_dir.join(name);
            let (exists, size) = match std::fs::metadata(&path) {
                Ok(meta) => (true, meta.len()),
                Err(_) => (false, 0),
            };
            entries.push(serde_json::json!({
                "name": name,
                "exists": exists,
                "size": size,
                "path": path.display().to_string(),
            }));
        }

        Ok(SelfConfigOutput {
            success: true,
            message: format!(
                "Found {} identity files for agent '{}'",
                entries.iter().filter(|e| e["exists"] == true).count(),
                self.agent_id
            ),
            data: Some(serde_json::Value::Array(entries)),
        })
    }

    fn read_file(&self, file_name: &str) -> Result<SelfConfigOutput> {
        validate_file_name(file_name)?;
        let path = self.agent_dir.join(file_name);
        match std::fs::read_to_string(&path) {
            Ok(content) => Ok(SelfConfigOutput {
                success: true,
                message: format!("Read {} ({} bytes)", file_name, content.len()),
                data: Some(serde_json::Value::String(content)),
            }),
            Err(e) => Ok(SelfConfigOutput {
                success: false,
                message: format!("Failed to read {}: {}", file_name, e),
                data: None,
            }),
        }
    }

    fn write_file(&self, file_name: &str, content: &str) -> Result<SelfConfigOutput> {
        validate_file_name(file_name)?;

        // Ensure agent directory exists
        if let Err(e) = std::fs::create_dir_all(&self.agent_dir) {
            return Ok(SelfConfigOutput {
                success: false,
                message: format!("Failed to create agent directory: {}", e),
                data: None,
            });
        }

        let path = self.agent_dir.join(file_name);
        match std::fs::write(&path, content) {
            Ok(()) => {
                let bytes = content.len();
                Ok(SelfConfigOutput {
                    success: true,
                    message: format!(
                        "Written {} bytes to {}. Changes will take effect on the next turn.",
                        bytes, file_name
                    ),
                    data: Some(serde_json::json!({ "bytes_written": bytes })),
                })
            }
            Err(e) => Ok(SelfConfigOutput {
                success: false,
                message: format!("Failed to write {}: {}", file_name, e),
                data: None,
            }),
        }
    }

    async fn read_config(&self, config_path: &str) -> Result<SelfConfigOutput> {
        let config = match &self.config {
            Some(c) => c,
            None => {
                return Ok(SelfConfigOutput {
                    success: false,
                    message: "Config handle not available".into(),
                    data: None,
                });
            }
        };

        let config_guard = config.read().await;
        let config_json = serde_json::to_value(&*config_guard).map_err(|e| {
            ToolError::Execution(format!("Failed to serialize config: {}", e))
        })?;

        let value = get_nested_value(&config_json, config_path);
        match value {
            Some(v) => Ok(SelfConfigOutput {
                success: true,
                message: format!("Config at '{}'", config_path),
                data: Some(v.clone()),
            }),
            None => Ok(SelfConfigOutput {
                success: false,
                message: format!("Config path '{}' not found", config_path),
                data: None,
            }),
        }
    }

    async fn update_config(
        &self,
        config_path: &str,
        config_value: serde_json::Value,
        dry_run: bool,
    ) -> Result<SelfConfigOutput> {
        let patcher = match &self.config_patcher {
            Some(p) => p,
            None => {
                return Ok(SelfConfigOutput {
                    success: false,
                    message: "Config patcher not available".into(),
                    data: None,
                });
            }
        };

        let request = PatchRequest {
            path: config_path.to_string(),
            patch: config_value,
            secret_fields: std::collections::HashMap::new(),
            health_check: false,
            dry_run,
        };

        match patcher.apply(request).await {
            Ok(result) => {
                let mode = if dry_run { "dry-run" } else { "applied" };
                Ok(SelfConfigOutput {
                    success: result.success,
                    message: format!(
                        "Config patch {} at '{}' ({} changes)",
                        mode,
                        config_path,
                        result.diff.len()
                    ),
                    data: Some(serde_json::to_value(&result).unwrap_or_default()),
                })
            }
            Err(e) => Ok(SelfConfigOutput {
                success: false,
                message: format!("Config patch failed: {}", e),
                data: None,
            }),
        }
    }
}

// =============================================================================
// AlephTool Implementation
// =============================================================================

#[async_trait]
impl AlephTool for SelfConfigTool {
    const NAME: &'static str = "self_config";
    const DESCRIPTION: &'static str = "Read and write Aleph identity files (MEMORY.md, SOUL.md, AGENTS.md, IDENTITY.md, TOOLS.md, HEARTBEAT.md) and modify config.toml with validation. Identity files live in the agent directory and are injected into your context on each turn. For config updates, use dot-path syntax (e.g. 'memory', 'providers.openai').";

    type Args = SelfConfigArgs;
    type Output = SelfConfigOutput;

    fn strict_schema(&self) -> bool {
        false
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match &args {
            SelfConfigArgs::ListFiles => notify_tool_start(Self::NAME, "list_files"),
            SelfConfigArgs::ReadFile { file_name } => {
                notify_tool_start(Self::NAME, &format!("read_file:{}", file_name))
            }
            SelfConfigArgs::WriteFile { file_name, .. } => {
                notify_tool_start(Self::NAME, &format!("write_file:{}", file_name))
            }
            SelfConfigArgs::ReadConfig { config_path } => {
                notify_tool_start(Self::NAME, &format!("read_config:{}", config_path))
            }
            SelfConfigArgs::UpdateConfig { config_path, .. } => {
                notify_tool_start(Self::NAME, &format!("update_config:{}", config_path))
            }
        }

        let result = match args {
            SelfConfigArgs::ListFiles => self.list_files(),
            SelfConfigArgs::ReadFile { file_name } => self.read_file(&file_name),
            SelfConfigArgs::WriteFile { file_name, content } => {
                self.write_file(&file_name, &content)
            }
            SelfConfigArgs::ReadConfig { config_path } => {
                self.read_config(&config_path).await
            }
            SelfConfigArgs::UpdateConfig {
                config_path,
                config_value,
                dry_run,
            } => {
                self.update_config(&config_path, config_value, dry_run)
                    .await
            }
        };

        match &result {
            Ok(output) => {
                notify_tool_result(Self::NAME, &output.message, output.success);
            }
            Err(e) => {
                notify_tool_result(Self::NAME, &e.to_string(), false);
            }
        }

        result
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Helper: create a SelfConfigTool pointing at a temp directory.
    fn tool_with_dir(dir: &std::path::Path) -> SelfConfigTool {
        SelfConfigTool {
            agent_dir: dir.to_path_buf(),
            agent_id: "test-agent".to_string(),
            config: None,
            config_patcher: None,
        }
    }

    #[tokio::test]
    async fn test_list_files() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("SOUL.md"), "soul content").unwrap();

        let tool = tool_with_dir(dir);
        let result = AlephTool::call(&tool, SelfConfigArgs::ListFiles)
            .await
            .unwrap();

        assert!(result.success);
        let data = result.data.unwrap();
        let arr = data.as_array().unwrap();
        assert_eq!(arr.len(), 6); // All IDENTITY_FILE_NAMES

        let soul = arr.iter().find(|e| e["name"] == "SOUL.md").unwrap();
        assert_eq!(soul["exists"], true);
        assert!(soul["size"].as_u64().unwrap() > 0);

        let memory = arr.iter().find(|e| e["name"] == "MEMORY.md").unwrap();
        assert_eq!(memory["exists"], false);
    }

    #[tokio::test]
    async fn test_read_write_file() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        let tool = tool_with_dir(dir);

        // Write MEMORY.md
        let write_result = AlephTool::call(
            &tool,
            SelfConfigArgs::WriteFile {
                file_name: "MEMORY.md".to_string(),
                content: "test memory content".to_string(),
            },
        )
        .await
        .unwrap();
        assert!(write_result.success);
        assert!(write_result.message.contains("19 bytes"));

        // Read it back
        let read_result = AlephTool::call(
            &tool,
            SelfConfigArgs::ReadFile {
                file_name: "MEMORY.md".to_string(),
            },
        )
        .await
        .unwrap();
        assert!(read_result.success);
        assert_eq!(
            read_result.data.unwrap().as_str().unwrap(),
            "test memory content"
        );
    }

    #[tokio::test]
    async fn test_write_rejects_invalid_name() {
        let tmp = TempDir::new().unwrap();
        let tool = tool_with_dir(tmp.path());

        let result = AlephTool::call(
            &tool,
            SelfConfigArgs::WriteFile {
                file_name: "../../etc/passwd".to_string(),
                content: "evil".to_string(),
            },
        )
        .await;

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Invalid"));
    }

    #[tokio::test]
    async fn test_write_creates_dir() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("deep").join("nested");
        let tool = SelfConfigTool {
            agent_dir: nested.clone(),
            agent_id: "test-agent".to_string(),
            config: None,
            config_patcher: None,
        };

        let result = AlephTool::call(
            &tool,
            SelfConfigArgs::WriteFile {
                file_name: "SOUL.md".to_string(),
                content: "created in nested dir".to_string(),
            },
        )
        .await
        .unwrap();

        assert!(result.success);
        assert!(nested.join("SOUL.md").exists());
    }

    #[tokio::test]
    async fn test_read_nonexistent_file() {
        let tmp = TempDir::new().unwrap();
        let tool = tool_with_dir(tmp.path());

        let result = AlephTool::call(
            &tool,
            SelfConfigArgs::ReadFile {
                file_name: "MEMORY.md".to_string(),
            },
        )
        .await
        .unwrap();

        assert!(!result.success);
        assert!(result.message.contains("Failed to read"));
    }
}
