//! File Read Tool
//!
//! Reads file contents from the filesystem.

use async_trait::async_trait;
use serde::Deserialize;
use std::path::PathBuf;

use super::config::FilesystemContext;
use crate::error::Result;
use crate::tools::{AgentTool, ToolCategory, ToolDefinition, ToolResult};

/// Parameters for file_read tool
#[derive(Debug, Deserialize)]
struct FileReadParams {
    /// Path to the file to read
    path: String,
}

/// File read tool
///
/// Reads file contents from the filesystem as UTF-8 text.
/// Validates that the path is within allowed directories.
pub struct FileReadTool {
    ctx: FilesystemContext,
}

impl FileReadTool {
    /// Create a new FileReadTool
    pub fn new(ctx: FilesystemContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl AgentTool for FileReadTool {
    fn name(&self) -> &str {
        "file_read"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "file_read",
            "Read file contents from the filesystem. Returns the file content as UTF-8 text.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute path to the file to read"
                    }
                },
                "required": ["path"]
            }),
            ToolCategory::Native,
        )
    }

    async fn execute(&self, args: &str) -> Result<ToolResult> {
        // Parse arguments
        let params: FileReadParams = serde_json::from_str(args).map_err(|e| {
            crate::error::AetherError::InvalidConfig {
                message: format!("Invalid arguments: {}", e),
                suggestion: Some("Provide a valid JSON with 'path' field".to_string()),
            }
        })?;

        let path = PathBuf::from(&params.path);

        // Validate path security
        self.ctx.validate_path(&path)?;

        // Read file
        let content = self.ctx.fs.read_file(&path).await?;

        Ok(ToolResult::success_with_data(
            content.clone(),
            serde_json::json!({
                "path": params.path,
                "size": content.len(),
                "content": content,
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::filesystem::config::FilesystemConfig;
    use tempfile::TempDir;

    fn create_test_tool(temp_dir: &TempDir) -> FileReadTool {
        let config = FilesystemConfig::new(vec![temp_dir.path().to_path_buf()]);
        let ctx = FilesystemContext::new(config);
        FileReadTool::new(ctx)
    }

    #[test]
    fn test_definition() {
        let temp_dir = TempDir::new().unwrap();
        let tool = create_test_tool(&temp_dir);
        let def = tool.definition();

        assert_eq!(def.name, "file_read");
        assert_eq!(def.category, ToolCategory::Native);
        assert!(!def.requires_confirmation);
    }

    #[tokio::test]
    async fn test_read_file_success() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        std::fs::write(&file_path, "Hello, World!").unwrap();

        let tool = create_test_tool(&temp_dir);
        let args = serde_json::json!({
            "path": file_path.to_string_lossy()
        })
        .to_string();

        let result = tool.execute(&args).await.unwrap();

        assert!(result.is_success());
        assert_eq!(result.content, "Hello, World!");
        assert!(result.data.is_some());
    }

    #[tokio::test]
    async fn test_read_file_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let tool = create_test_tool(&temp_dir);
        let file_path = temp_dir.path().join("nonexistent.txt");

        let args = serde_json::json!({
            "path": file_path.to_string_lossy()
        })
        .to_string();

        let result = tool.execute(&args).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_read_file_outside_allowed_roots() {
        let temp_dir = TempDir::new().unwrap();
        let tool = create_test_tool(&temp_dir);

        let args = serde_json::json!({
            "path": "/etc/passwd"
        })
        .to_string();

        let result = tool.execute(&args).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_invalid_args() {
        let temp_dir = TempDir::new().unwrap();
        let tool = create_test_tool(&temp_dir);

        let result = tool.execute("invalid json").await;
        assert!(result.is_err());
    }
}
