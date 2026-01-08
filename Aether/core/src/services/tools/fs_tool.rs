//! File System Tool
//!
//! Wraps `services::fs::LocalFs` with JSON interface for LLM tool invocation.
//! This is a Tier 1 System Tool, exposed at `/fs`.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::SystemTool;
use crate::error::{AetherError, Result};
use crate::mcp::types::{McpResource, McpTool, McpToolResult};
use crate::services::fs::{FileOps, LocalFs};

/// File system MCP service configuration
#[derive(Debug, Clone)]
pub struct FsServiceConfig {
    /// Allowed root directories for file operations
    pub allowed_roots: Vec<PathBuf>,
}

impl Default for FsServiceConfig {
    fn default() -> Self {
        Self {
            allowed_roots: vec![],
        }
    }
}

/// File system MCP service
///
/// Provides file operations (read, write, list, search) with path security.
pub struct FsService {
    fs: Arc<dyn FileOps>,
    config: FsServiceConfig,
}

impl FsService {
    /// Create a new FsService with default LocalFs implementation
    pub fn new(config: FsServiceConfig) -> Self {
        Self {
            fs: Arc::new(LocalFs::new()),
            config,
        }
    }

    /// Create a new FsService with custom FileOps implementation (for testing)
    pub fn with_file_ops(fs: Arc<dyn FileOps>, config: FsServiceConfig) -> Self {
        Self { fs, config }
    }

    /// Check if a path is within allowed roots
    fn is_path_allowed(&self, path: &Path) -> bool {
        // If no roots are configured, deny all access
        if self.config.allowed_roots.is_empty() {
            return false;
        }

        // Canonicalize the path to resolve .. and symlinks
        let canonical = match path.canonicalize() {
            Ok(p) => p,
            // If path doesn't exist, check against the parent
            Err(_) => {
                if let Some(parent) = path.parent() {
                    match parent.canonicalize() {
                        Ok(p) => p.join(path.file_name().unwrap_or_default()),
                        Err(_) => return false,
                    }
                } else {
                    return false;
                }
            }
        };

        // Check if path is within any allowed root
        self.config.allowed_roots.iter().any(|root| {
            if let Ok(canonical_root) = root.canonicalize() {
                canonical.starts_with(&canonical_root)
            } else {
                // For roots that don't exist yet, do prefix match
                canonical.starts_with(root)
            }
        })
    }

    /// Validate path and return error if not allowed
    fn validate_path(&self, path: &Path) -> Result<()> {
        if !self.is_path_allowed(path) {
            return Err(AetherError::PermissionDenied {
                message: format!("Path not allowed: {}", path.display()),
                suggestion: Some("Check that the path is within allowed_roots configuration".to_string()),
            });
        }
        Ok(())
    }

    /// Extract path from arguments
    fn get_path_arg(&self, args: &Value) -> Result<PathBuf> {
        args.get("path")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .ok_or_else(|| AetherError::InvalidConfig {
                message: "Missing 'path' argument".to_string(),
                suggestion: None,
            })
    }
}

#[async_trait]
impl SystemTool for FsService {
    fn name(&self) -> &str {
        "fs"
    }

    fn description(&self) -> &str {
        "File system operations (read, write, list, search)"
    }

    async fn list_resources(&self) -> Result<Vec<McpResource>> {
        // List allowed roots as resources
        let mut resources = Vec::new();
        for root in &self.config.allowed_roots {
            resources.push(McpResource {
                uri: format!("file://{}", root.display()),
                name: root.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| root.to_string_lossy().to_string()),
                description: Some(format!("Allowed root: {}", root.display())),
                mime_type: None,
            });
        }
        Ok(resources)
    }

    async fn read_resource(&self, uri: &str) -> Result<String> {
        // Parse file:// URI
        let path = uri.strip_prefix("file://")
            .ok_or_else(|| AetherError::InvalidConfig {
                message: format!("Invalid file URI: {}", uri),
                suggestion: Some("Use file:// prefix".to_string()),
            })?;

        let path = PathBuf::from(path);
        self.validate_path(&path)?;
        self.fs.read_file(&path).await
    }

    fn list_tools(&self) -> Vec<McpTool> {
        vec![
            McpTool {
                name: "file_list".to_string(),
                description: "List files and directories in a path".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Directory path to list"
                        }
                    },
                    "required": ["path"]
                }),
                requires_confirmation: false,
            },
            McpTool {
                name: "file_read".to_string(),
                description: "Read file contents".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "File path to read"
                        }
                    },
                    "required": ["path"]
                }),
                requires_confirmation: false,
            },
            McpTool {
                name: "file_write".to_string(),
                description: "Write content to a file".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "File path to write"
                        },
                        "content": {
                            "type": "string",
                            "description": "Content to write"
                        }
                    },
                    "required": ["path", "content"]
                }),
                requires_confirmation: true,
            },
            McpTool {
                name: "file_delete".to_string(),
                description: "Delete a file or directory".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to delete"
                        }
                    },
                    "required": ["path"]
                }),
                requires_confirmation: true,
            },
            McpTool {
                name: "file_search".to_string(),
                description: "Search for files matching a glob pattern".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "base": {
                            "type": "string",
                            "description": "Base directory to search from"
                        },
                        "pattern": {
                            "type": "string",
                            "description": "Glob pattern (e.g., '**/*.rs')"
                        }
                    },
                    "required": ["base", "pattern"]
                }),
                requires_confirmation: false,
            },
        ]
    }

    async fn call_tool(&self, name: &str, args: Value) -> Result<McpToolResult> {
        match name {
            "file_list" => {
                let path = self.get_path_arg(&args)?;
                self.validate_path(&path)?;

                let entries = self.fs.list_dir(&path).await?;
                let result: Vec<Value> = entries
                    .into_iter()
                    .map(|e| json!({
                        "name": e.name,
                        "path": e.path,
                        "is_dir": e.is_dir,
                        "size": e.size,
                        "modified": e.modified,
                    }))
                    .collect();

                Ok(McpToolResult::success(json!(result)))
            }

            "file_read" => {
                let path = self.get_path_arg(&args)?;
                self.validate_path(&path)?;

                let content = self.fs.read_file(&path).await?;
                Ok(McpToolResult::success(json!({
                    "content": content,
                    "path": path.to_string_lossy(),
                })))
            }

            "file_write" => {
                let path = self.get_path_arg(&args)?;
                self.validate_path(&path)?;

                let content = args.get("content")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AetherError::InvalidConfig {
                        message: "Missing 'content' argument".to_string(),
                        suggestion: None,
                    })?;

                self.fs.write_file(&path, content).await?;
                Ok(McpToolResult::success(json!({
                    "success": true,
                    "path": path.to_string_lossy(),
                })))
            }

            "file_delete" => {
                let path = self.get_path_arg(&args)?;
                self.validate_path(&path)?;

                self.fs.delete(&path).await?;
                Ok(McpToolResult::success(json!({
                    "success": true,
                    "path": path.to_string_lossy(),
                })))
            }

            "file_search" => {
                let base = args.get("base")
                    .and_then(|v| v.as_str())
                    .map(PathBuf::from)
                    .ok_or_else(|| AetherError::InvalidConfig {
                        message: "Missing 'base' argument".to_string(),
                        suggestion: None,
                    })?;

                let pattern = args.get("pattern")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| AetherError::InvalidConfig {
                        message: "Missing 'pattern' argument".to_string(),
                        suggestion: None,
                    })?;

                self.validate_path(&base)?;

                let entries = self.fs.search(&base, pattern).await?;
                let result: Vec<Value> = entries
                    .into_iter()
                    .map(|e| json!({
                        "name": e.name,
                        "path": e.path,
                        "is_dir": e.is_dir,
                        "size": e.size,
                        "modified": e.modified,
                    }))
                    .collect();

                Ok(McpToolResult::success(json!(result)))
            }

            _ => Ok(McpToolResult::error(format!("Unknown tool: {}", name))),
        }
    }

    fn requires_confirmation(&self, tool_name: &str) -> bool {
        matches!(tool_name, "file_write" | "file_delete")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_service(temp_dir: &TempDir) -> FsService {
        FsService::new(FsServiceConfig {
            allowed_roots: vec![temp_dir.path().to_path_buf()],
        })
    }

    #[tokio::test]
    async fn test_file_list() {
        let temp_dir = TempDir::new().unwrap();
        let service = create_test_service(&temp_dir);

        // Create test files
        std::fs::write(temp_dir.path().join("test.txt"), "hello").unwrap();
        std::fs::create_dir(temp_dir.path().join("subdir")).unwrap();

        let result = service.call_tool("file_list", json!({
            "path": temp_dir.path().to_string_lossy()
        })).await.unwrap();

        assert!(result.success);
        let entries: Vec<Value> = serde_json::from_value(result.content).unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[tokio::test]
    async fn test_file_read_write() {
        let temp_dir = TempDir::new().unwrap();
        let service = create_test_service(&temp_dir);

        let file_path = temp_dir.path().join("test.txt");

        // Write
        let result = service.call_tool("file_write", json!({
            "path": file_path.to_string_lossy(),
            "content": "Hello, World!"
        })).await.unwrap();
        assert!(result.success);

        // Read
        let result = service.call_tool("file_read", json!({
            "path": file_path.to_string_lossy()
        })).await.unwrap();
        assert!(result.success);
        assert_eq!(result.content["content"], "Hello, World!");
    }

    #[tokio::test]
    async fn test_path_security() {
        let temp_dir = TempDir::new().unwrap();
        let service = create_test_service(&temp_dir);

        // Try to read outside allowed roots
        let result = service.call_tool("file_read", json!({
            "path": "/etc/passwd"
        })).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_requires_confirmation() {
        let temp_dir = TempDir::new().unwrap();
        let service = create_test_service(&temp_dir);

        assert!(!service.requires_confirmation("file_list"));
        assert!(!service.requires_confirmation("file_read"));
        assert!(service.requires_confirmation("file_write"));
        assert!(service.requires_confirmation("file_delete"));
    }
}
