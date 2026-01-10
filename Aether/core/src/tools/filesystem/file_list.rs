//! File List Tool
//!
//! Lists directory contents on the filesystem.

use async_trait::async_trait;
use serde::Deserialize;
use std::path::PathBuf;

use super::config::FilesystemContext;
use crate::error::Result;
use crate::tools::{AgentTool, ToolCategory, ToolDefinition, ToolResult};

/// Parameters for file_list tool
#[derive(Debug, Deserialize)]
struct FileListParams {
    /// Path to the directory to list
    path: String,
}

/// File list tool
///
/// Lists the contents of a directory, showing files and subdirectories
/// with their sizes and modification times.
pub struct FileListTool {
    ctx: FilesystemContext,
}

impl FileListTool {
    /// Create a new FileListTool
    pub fn new(ctx: FilesystemContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl AgentTool for FileListTool {
    fn name(&self) -> &str {
        "file_list"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "file_list",
            "List files and directories in a path. Returns name, path, size, and type for each entry.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute path to the directory to list"
                    }
                },
                "required": ["path"]
            }),
            ToolCategory::Filesystem,
        )
    }

    async fn execute(&self, args: &str) -> Result<ToolResult> {
        // Parse arguments
        let params: FileListParams = serde_json::from_str(args).map_err(|e| {
            crate::error::AetherError::InvalidConfig {
                message: format!("Invalid arguments: {}", e),
                suggestion: Some("Provide a valid JSON with 'path' field".to_string()),
            }
        })?;

        let path = PathBuf::from(&params.path);

        // Validate path security
        self.ctx.validate_path(&path)?;

        // List directory
        let entries = self.ctx.fs.list_dir(&path).await?;

        // Format output for human readability
        let mut output = format!("Contents of {}:\n\n", params.path);

        let mut dirs = Vec::new();
        let mut files = Vec::new();

        for entry in &entries {
            if entry.is_dir {
                dirs.push(entry);
            } else {
                files.push(entry);
            }
        }

        // List directories first
        for entry in &dirs {
            output.push_str(&format!("📁 {}/\n", entry.name));
        }

        // Then files with sizes
        for entry in &files {
            let size_str = format_size(entry.size);
            output.push_str(&format!("📄 {} ({})\n", entry.name, size_str));
        }

        output.push_str(&format!("\n{} directories, {} files", dirs.len(), files.len()));

        // Build JSON data
        let entries_json: Vec<serde_json::Value> = entries
            .iter()
            .map(|e| {
                serde_json::json!({
                    "name": e.name,
                    "path": e.path,
                    "is_dir": e.is_dir,
                    "size": e.size,
                    "modified": e.modified,
                })
            })
            .collect();

        Ok(ToolResult::success_with_data(
            output,
            serde_json::json!({
                "path": params.path,
                "entries": entries_json,
                "total_dirs": dirs.len(),
                "total_files": files.len(),
            }),
        ))
    }
}

/// Format file size for human readability
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::filesystem::config::FilesystemConfig;
    use tempfile::TempDir;

    fn create_test_tool(temp_dir: &TempDir) -> FileListTool {
        let config = FilesystemConfig::new(vec![temp_dir.path().to_path_buf()]);
        let ctx = FilesystemContext::new(config);
        FileListTool::new(ctx)
    }

    #[test]
    fn test_definition() {
        let temp_dir = TempDir::new().unwrap();
        let tool = create_test_tool(&temp_dir);
        let def = tool.definition();

        assert_eq!(def.name, "file_list");
        assert_eq!(def.category, ToolCategory::Filesystem);
        assert!(!def.requires_confirmation);
    }

    #[tokio::test]
    async fn test_list_directory() {
        let temp_dir = TempDir::new().unwrap();

        // Create some test files and directories
        std::fs::write(temp_dir.path().join("file1.txt"), "content1").unwrap();
        std::fs::write(temp_dir.path().join("file2.txt"), "content2").unwrap();
        std::fs::create_dir(temp_dir.path().join("subdir")).unwrap();

        let tool = create_test_tool(&temp_dir);
        let args = serde_json::json!({
            "path": temp_dir.path().to_string_lossy()
        })
        .to_string();

        let result = tool.execute(&args).await.unwrap();

        assert!(result.is_success());
        assert!(result.content.contains("file1.txt"));
        assert!(result.content.contains("file2.txt"));
        assert!(result.content.contains("subdir"));
        assert!(result.content.contains("1 directories, 2 files"));

        // Check JSON data
        let data = result.data.unwrap();
        assert_eq!(data["total_dirs"], 1);
        assert_eq!(data["total_files"], 2);
    }

    #[tokio::test]
    async fn test_list_empty_directory() {
        let temp_dir = TempDir::new().unwrap();

        let tool = create_test_tool(&temp_dir);
        let args = serde_json::json!({
            "path": temp_dir.path().to_string_lossy()
        })
        .to_string();

        let result = tool.execute(&args).await.unwrap();

        assert!(result.is_success());
        assert!(result.content.contains("0 directories, 0 files"));
    }

    #[tokio::test]
    async fn test_list_outside_allowed_roots() {
        let temp_dir = TempDir::new().unwrap();
        let tool = create_test_tool(&temp_dir);

        let args = serde_json::json!({
            "path": "/etc"
        })
        .to_string();

        let result = tool.execute(&args).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(1048576), "1.0 MB");
        assert_eq!(format_size(1073741824), "1.0 GB");
    }
}
