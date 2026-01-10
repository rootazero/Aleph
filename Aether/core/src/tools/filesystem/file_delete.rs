//! File Delete Tool
//!
//! Deletes files and directories from the filesystem.
//! Requires user confirmation before execution.

use async_trait::async_trait;
use serde::Deserialize;
use std::path::PathBuf;

use super::config::FilesystemContext;
use crate::error::Result;
use crate::tools::{AgentTool, ToolCategory, ToolDefinition, ToolResult};

/// Parameters for file_delete tool
#[derive(Debug, Deserialize)]
struct FileDeleteParams {
    /// Path to the file or directory to delete
    path: String,
}

/// File delete tool
///
/// Deletes a file or directory from the filesystem.
/// For directories, recursively deletes all contents.
/// Requires user confirmation as it is a destructive operation.
pub struct FileDeleteTool {
    ctx: FilesystemContext,
}

impl FileDeleteTool {
    /// Create a new FileDeleteTool
    pub fn new(ctx: FilesystemContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl AgentTool for FileDeleteTool {
    fn name(&self) -> &str {
        "file_delete"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "file_delete",
            "Delete a file or directory. For directories, recursively deletes all contents. This is a destructive operation.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute path to the file or directory to delete"
                    }
                },
                "required": ["path"]
            }),
            ToolCategory::Filesystem,
        )
        .with_confirmation(true)
    }

    fn requires_confirmation(&self) -> bool {
        true
    }

    async fn execute(&self, args: &str) -> Result<ToolResult> {
        // Parse arguments
        let params: FileDeleteParams = serde_json::from_str(args).map_err(|e| {
            crate::error::AetherError::InvalidConfig {
                message: format!("Invalid arguments: {}", e),
                suggestion: Some("Provide a valid JSON with 'path' field".to_string()),
            }
        })?;

        let path = PathBuf::from(&params.path);

        // Validate path security
        self.ctx.validate_path(&path)?;

        // Check if path exists
        let exists = self.ctx.fs.exists(&path).await?;
        if !exists {
            return Ok(ToolResult::error(format!(
                "Path does not exist: {}",
                params.path
            )));
        }

        // Check if it's a directory
        let is_dir = self.ctx.fs.is_dir(&path).await?;
        let item_type = if is_dir { "directory" } else { "file" };

        // Delete
        self.ctx.fs.delete(&path).await?;

        Ok(ToolResult::success_with_data(
            format!("Successfully deleted {}: {}", item_type, params.path),
            serde_json::json!({
                "path": params.path,
                "was_directory": is_dir,
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

    fn create_test_tool(temp_dir: &TempDir) -> FileDeleteTool {
        let config = FilesystemConfig::new(vec![temp_dir.path().to_path_buf()]);
        let ctx = FilesystemContext::new(config);
        FileDeleteTool::new(ctx)
    }

    #[test]
    fn test_definition() {
        let temp_dir = TempDir::new().unwrap();
        let tool = create_test_tool(&temp_dir);
        let def = tool.definition();

        assert_eq!(def.name, "file_delete");
        assert_eq!(def.category, ToolCategory::Filesystem);
        assert!(def.requires_confirmation);
    }

    #[test]
    fn test_requires_confirmation() {
        let temp_dir = TempDir::new().unwrap();
        let tool = create_test_tool(&temp_dir);
        assert!(tool.requires_confirmation());
    }

    #[tokio::test]
    async fn test_delete_file() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("to_delete.txt");
        std::fs::write(&file_path, "content").unwrap();
        assert!(file_path.exists());

        let tool = create_test_tool(&temp_dir);
        let args = serde_json::json!({
            "path": file_path.to_string_lossy()
        })
        .to_string();

        let result = tool.execute(&args).await.unwrap();

        assert!(result.is_success());
        assert!(result.content.contains("file"));
        assert!(!file_path.exists());
    }

    #[tokio::test]
    async fn test_delete_directory() {
        let temp_dir = TempDir::new().unwrap();
        let dir_path = temp_dir.path().join("to_delete_dir");
        std::fs::create_dir(&dir_path).unwrap();
        std::fs::write(dir_path.join("file.txt"), "content").unwrap();
        assert!(dir_path.exists());

        let tool = create_test_tool(&temp_dir);
        let args = serde_json::json!({
            "path": dir_path.to_string_lossy()
        })
        .to_string();

        let result = tool.execute(&args).await.unwrap();

        assert!(result.is_success());
        assert!(result.content.contains("directory"));
        assert!(!dir_path.exists());
    }

    #[tokio::test]
    async fn test_delete_nonexistent() {
        let temp_dir = TempDir::new().unwrap();
        let tool = create_test_tool(&temp_dir);

        let args = serde_json::json!({
            "path": temp_dir.path().join("nonexistent.txt").to_string_lossy()
        })
        .to_string();

        let result = tool.execute(&args).await.unwrap();

        assert!(!result.is_success());
        assert!(result.error_message().unwrap().contains("does not exist"));
    }

    #[tokio::test]
    async fn test_delete_outside_allowed_roots() {
        let temp_dir = TempDir::new().unwrap();
        let tool = create_test_tool(&temp_dir);

        let args = serde_json::json!({
            "path": "/tmp/unauthorized_delete.txt"
        })
        .to_string();

        let result = tool.execute(&args).await;
        assert!(result.is_err());
    }
}
