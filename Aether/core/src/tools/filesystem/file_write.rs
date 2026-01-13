//! File Write Tool
//!
//! Writes content to files on the filesystem.
//! Requires user confirmation before execution.

use async_trait::async_trait;
use serde::Deserialize;
use std::path::PathBuf;

use super::config::FilesystemContext;
use crate::error::Result;
use crate::tools::{AgentTool, ToolCategory, ToolDefinition, ToolResult};

/// Parameters for file_write tool
#[derive(Debug, Deserialize)]
struct FileWriteParams {
    /// Path to the file to write
    path: String,
    /// Content to write to the file
    content: String,
}

/// File write tool
///
/// Writes string content to a file on the filesystem.
/// Creates parent directories if they don't exist.
/// Requires user confirmation as it modifies the filesystem.
pub struct FileWriteTool {
    ctx: FilesystemContext,
}

impl FileWriteTool {
    /// Create a new FileWriteTool
    pub fn new(ctx: FilesystemContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl AgentTool for FileWriteTool {
    fn name(&self) -> &str {
        "file_write"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "file_write",
            "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Creates parent directories if needed.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute path to the file to write"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write to the file"
                    }
                },
                "required": ["path", "content"]
            }),
            ToolCategory::Native,
        )
        .with_confirmation(true)
    }

    fn requires_confirmation(&self) -> bool {
        true
    }

    async fn execute(&self, args: &str) -> Result<ToolResult> {
        // Parse arguments
        let params: FileWriteParams = serde_json::from_str(args).map_err(|e| {
            crate::error::AetherError::InvalidConfig {
                message: format!("Invalid arguments: {}", e),
                suggestion: Some("Provide a valid JSON with 'path' and 'content' fields".to_string()),
            }
        })?;

        let path = PathBuf::from(&params.path);

        // Validate path security
        self.ctx.validate_path(&path)?;

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                self.ctx.fs.create_dir(parent).await?;
            }
        }

        // Write file
        let content_len = params.content.len();
        self.ctx.fs.write_file(&path, &params.content).await?;

        Ok(ToolResult::success_with_data(
            format!("Successfully wrote {} bytes to {}", content_len, params.path),
            serde_json::json!({
                "path": params.path,
                "bytes_written": content_len,
                "success": true,
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::filesystem::config::FilesystemConfig;
    use tempfile::TempDir;

    fn create_test_tool(temp_dir: &TempDir) -> FileWriteTool {
        let config = FilesystemConfig::new(vec![temp_dir.path().to_path_buf()]);
        let ctx = FilesystemContext::new(config);
        FileWriteTool::new(ctx)
    }

    #[test]
    fn test_definition() {
        let temp_dir = TempDir::new().unwrap();
        let tool = create_test_tool(&temp_dir);
        let def = tool.definition();

        assert_eq!(def.name, "file_write");
        assert_eq!(def.category, ToolCategory::Native);
        assert!(def.requires_confirmation);
    }

    #[test]
    fn test_requires_confirmation() {
        let temp_dir = TempDir::new().unwrap();
        let tool = create_test_tool(&temp_dir);
        assert!(tool.requires_confirmation());
    }

    #[tokio::test]
    async fn test_write_file_success() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("output.txt");

        let tool = create_test_tool(&temp_dir);
        let args = serde_json::json!({
            "path": file_path.to_string_lossy(),
            "content": "Hello, World!"
        })
        .to_string();

        let result = tool.execute(&args).await.unwrap();

        assert!(result.is_success());
        assert!(result.content.contains("13 bytes"));

        // Verify file was written
        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "Hello, World!");
    }

    #[tokio::test]
    async fn test_write_creates_parent_dirs() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("subdir").join("nested").join("file.txt");

        let tool = create_test_tool(&temp_dir);
        let args = serde_json::json!({
            "path": file_path.to_string_lossy(),
            "content": "Nested content"
        })
        .to_string();

        let result = tool.execute(&args).await.unwrap();

        assert!(result.is_success());
        assert!(file_path.exists());
    }

    #[tokio::test]
    async fn test_write_outside_allowed_roots() {
        let temp_dir = TempDir::new().unwrap();
        let tool = create_test_tool(&temp_dir);

        let args = serde_json::json!({
            "path": "/tmp/unauthorized.txt",
            "content": "Should not write"
        })
        .to_string();

        let result = tool.execute(&args).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_overwrite_existing_file() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("existing.txt");
        std::fs::write(&file_path, "Old content").unwrap();

        let tool = create_test_tool(&temp_dir);
        let args = serde_json::json!({
            "path": file_path.to_string_lossy(),
            "content": "New content"
        })
        .to_string();

        let result = tool.execute(&args).await.unwrap();
        assert!(result.is_success());

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "New content");
    }
}
