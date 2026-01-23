//! Type definitions for file operations

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// File operation type
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileOperation {
    /// List directory contents
    List,
    /// Read file content
    Read,
    /// Write content to file
    Write,
    /// Move/rename file or directory
    Move,
    /// Copy file or directory
    Copy,
    /// Delete file or directory
    Delete,
    /// Create directory
    Mkdir,
    /// Search files by glob pattern
    Search,
    /// Batch move files matching a pattern to destination
    BatchMove,
    /// Auto-organize files by type into categorized folders
    Organize,
}

/// Arguments for file operations tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FileOpsArgs {
    /// The operation to perform
    pub operation: FileOperation,
    /// Primary path (source path for move/copy, target path for others)
    pub path: String,
    /// Destination path (for move/copy operations)
    #[serde(default)]
    pub destination: Option<String>,
    /// Content to write (for write operation)
    #[serde(default)]
    pub content: Option<String>,
    /// Search pattern (for search operation, glob syntax)
    #[serde(default)]
    pub pattern: Option<String>,
    /// Create parent directories if they don't exist
    #[serde(default = "default_true")]
    pub create_parents: bool,
}

fn default_true() -> bool {
    true
}

/// File metadata in output
#[derive(Debug, Clone, Serialize)]
pub struct FileInfo {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extension: Option<String>,
}

/// Output from file operations tool
#[derive(Debug, Clone, Serialize)]
pub struct FileOpsOutput {
    pub success: bool,
    pub operation: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<FileInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_written: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items_affected: Option<usize>,
}
