//! File Search Tool
//!
//! Searches for files matching glob patterns.

use async_trait::async_trait;
use serde::Deserialize;
use std::path::PathBuf;

use super::config::FilesystemContext;
use crate::error::Result;
use crate::tools::{AgentTool, ToolCategory, ToolDefinition, ToolResult};

/// Parameters for file_search tool
#[derive(Debug, Deserialize)]
struct FileSearchParams {
    /// Base directory to search from
    base: String,
    /// Glob pattern to match (e.g., "**/*.rs", "*.txt")
    pattern: String,
}

/// File search tool
///
/// Searches for files matching a glob pattern within a base directory.
/// Supports recursive patterns like `**/*.rs` to find all Rust files.
pub struct FileSearchTool {
    ctx: FilesystemContext,
}

impl FileSearchTool {
    /// Create a new FileSearchTool
    pub fn new(ctx: FilesystemContext) -> Self {
        Self { ctx }
    }
}

#[async_trait]
impl AgentTool for FileSearchTool {
    fn name(&self) -> &str {
        "file_search"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "file_search",
            "Search for files matching a glob pattern. Supports patterns like '**/*.rs' (all Rust files), '*.txt' (text files in directory), 'src/**/*.ts' (TypeScript in src).",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "base": {
                        "type": "string",
                        "description": "Base directory to search from"
                    },
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern to match (e.g., '**/*.rs', '*.txt', 'src/**/*.ts')"
                    }
                },
                "required": ["base", "pattern"]
            }),
            ToolCategory::Filesystem,
        )
    }

    async fn execute(&self, args: &str) -> Result<ToolResult> {
        // Parse arguments
        let params: FileSearchParams = serde_json::from_str(args).map_err(|e| {
            crate::error::AetherError::InvalidConfig {
                message: format!("Invalid arguments: {}", e),
                suggestion: Some("Provide a valid JSON with 'base' and 'pattern' fields".to_string()),
            }
        })?;

        let base = PathBuf::from(&params.base);

        // Validate path security
        self.ctx.validate_path(&base)?;

        // Search files
        let entries = self.ctx.fs.search(&base, &params.pattern).await?;

        // Format output
        let mut output = format!(
            "Search results for '{}' in {}:\n\n",
            params.pattern, params.base
        );

        if entries.is_empty() {
            output.push_str("No matching files found.");
        } else {
            for entry in &entries {
                let icon = if entry.is_dir { "📁" } else { "📄" };
                output.push_str(&format!("{} {}\n", icon, entry.path));
            }
            output.push_str(&format!("\nFound {} matches", entries.len()));
        }

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
                "base": params.base,
                "pattern": params.pattern,
                "matches": entries_json,
                "total_matches": entries.len(),
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::filesystem::config::FilesystemConfig;
    use tempfile::TempDir;

    fn create_test_tool(temp_dir: &TempDir) -> FileSearchTool {
        let config = FilesystemConfig::new(vec![temp_dir.path().to_path_buf()]);
        let ctx = FilesystemContext::new(config);
        FileSearchTool::new(ctx)
    }

    #[test]
    fn test_definition() {
        let temp_dir = TempDir::new().unwrap();
        let tool = create_test_tool(&temp_dir);
        let def = tool.definition();

        assert_eq!(def.name, "file_search");
        assert_eq!(def.category, ToolCategory::Filesystem);
        assert!(!def.requires_confirmation);
    }

    #[tokio::test]
    async fn test_search_txt_files() {
        let temp_dir = TempDir::new().unwrap();

        // Create test files
        std::fs::write(temp_dir.path().join("file1.txt"), "content").unwrap();
        std::fs::write(temp_dir.path().join("file2.txt"), "content").unwrap();
        std::fs::write(temp_dir.path().join("file3.rs"), "content").unwrap();

        let tool = create_test_tool(&temp_dir);
        let args = serde_json::json!({
            "base": temp_dir.path().to_string_lossy(),
            "pattern": "*.txt"
        })
        .to_string();

        let result = tool.execute(&args).await.unwrap();

        assert!(result.is_success());
        assert!(result.content.contains("file1.txt"));
        assert!(result.content.contains("file2.txt"));
        assert!(!result.content.contains("file3.rs"));

        let data = result.data.unwrap();
        assert_eq!(data["total_matches"], 2);
    }

    #[tokio::test]
    async fn test_search_recursive() {
        let temp_dir = TempDir::new().unwrap();

        // Create nested structure
        std::fs::create_dir_all(temp_dir.path().join("src/nested")).unwrap();
        std::fs::write(temp_dir.path().join("root.rs"), "content").unwrap();
        std::fs::write(temp_dir.path().join("src/main.rs"), "content").unwrap();
        std::fs::write(temp_dir.path().join("src/nested/lib.rs"), "content").unwrap();

        let tool = create_test_tool(&temp_dir);
        let args = serde_json::json!({
            "base": temp_dir.path().to_string_lossy(),
            "pattern": "**/*.rs"
        })
        .to_string();

        let result = tool.execute(&args).await.unwrap();

        assert!(result.is_success());
        let data = result.data.unwrap();
        assert_eq!(data["total_matches"], 3);
    }

    #[tokio::test]
    async fn test_search_no_matches() {
        let temp_dir = TempDir::new().unwrap();
        std::fs::write(temp_dir.path().join("file.txt"), "content").unwrap();

        let tool = create_test_tool(&temp_dir);
        let args = serde_json::json!({
            "base": temp_dir.path().to_string_lossy(),
            "pattern": "*.rs"
        })
        .to_string();

        let result = tool.execute(&args).await.unwrap();

        assert!(result.is_success());
        assert!(result.content.contains("No matching files"));
        let data = result.data.unwrap();
        assert_eq!(data["total_matches"], 0);
    }

    #[tokio::test]
    async fn test_search_outside_allowed_roots() {
        let temp_dir = TempDir::new().unwrap();
        let tool = create_test_tool(&temp_dir);

        let args = serde_json::json!({
            "base": "/etc",
            "pattern": "*.conf"
        })
        .to_string();

        let result = tool.execute(&args).await;
        assert!(result.is_err());
    }
}
