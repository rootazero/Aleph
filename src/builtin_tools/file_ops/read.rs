//! Independent FileReadTool with explicit schema for file reading operations.

use std::path::Path;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use super::ops::execute_read;
use super::path_utils::get_denied_paths;
use crate::error::Result;
use crate::tools::AlephTool;

// =============================================================================
// Args & Output
// =============================================================================

/// Arguments for the file_read tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FileReadArgs {
    /// Absolute path to the file to read.
    pub path: String,

    /// Optional line offset to start reading from (0-based byte offset).
    #[serde(default)]
    pub offset: Option<u64>,

    /// Optional maximum number of bytes to read.
    #[serde(default)]
    pub limit: Option<u64>,
}

/// Output returned by the file_read tool.
#[derive(Debug, Clone, Serialize)]
pub struct FileReadOutput {
    /// Whether the operation succeeded.
    pub success: bool,
    /// The resolved file path.
    pub path: String,
    /// The file content (or partial content when offset/limit are used).
    pub content: String,
    /// File size in bytes.
    pub size: u64,
    /// Human-readable result message.
    pub message: String,
}

// =============================================================================
// FileReadTool
// =============================================================================

/// Standalone tool for reading file contents.
pub struct FileReadTool {
    /// Maximum file size allowed for read operations (default 100 MB).
    max_read_size: u64,
    /// Security-denied path patterns.
    denied_paths: Vec<String>,
    /// Optional ToolContext handle for workspace-scoped output path resolution.
    tool_context_handle: Option<crate::tools::ToolContextHandle>,
}

impl FileReadTool {
    /// Create a new FileReadTool with default settings.
    pub fn new() -> Self {
        let denied_paths = get_denied_paths();
        info!(
            denied_paths_count = denied_paths.len(),
            "FileReadTool: initialized with denied_paths"
        );

        Self {
            max_read_size: 100 * 1024 * 1024, // 100 MB
            denied_paths,
            tool_context_handle: None,
        }
    }

    /// Configure the tool to use a ToolContext handle for workspace-scoped output paths.
    pub fn with_tool_context(mut self, handle: crate::tools::ToolContextHandle) -> Self {
        self.tool_context_handle = Some(handle);
        self
    }

    /// Resolve the output directory from the ToolContext handle (if available).
    async fn resolve_output_dir(&self) -> Option<std::path::PathBuf> {
        if let Some(ref handle) = self.tool_context_handle {
            let ctx = handle.read().await;
            Some(ctx.output_dir.join("documents"))
        } else {
            None
        }
    }
}

impl Default for FileReadTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for FileReadTool {
    fn clone(&self) -> Self {
        Self {
            max_read_size: self.max_read_size,
            denied_paths: self.denied_paths.clone(),
            tool_context_handle: self.tool_context_handle.clone(),
        }
    }
}

// =============================================================================
// AlephTool impl
// =============================================================================

#[async_trait]
impl AlephTool for FileReadTool {
    const NAME: &'static str = "file_read";
    const DESCRIPTION: &'static str =
        "Read the contents of a file. Returns the text content and file size. \
         Use offset and limit for partial reads of large files.";

    type Args = FileReadArgs;
    type Output = FileReadOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        use crate::builtin_tools::{notify_tool_result, notify_tool_start};

        let args_summary = format!("read: {}", &args.path);
        notify_tool_start(Self::NAME, &args_summary);

        let path = Path::new(&args.path);
        let output_dir = self.resolve_output_dir().await;
        let output_dir_ref = output_dir.as_deref();

        let result = execute_read(path, &self.denied_paths, self.max_read_size, output_dir_ref)
            .await
            .map_err(|e| {
                notify_tool_result(Self::NAME, &e.to_string(), false);
                e
            })?;

        // Apply offset/limit slicing when requested.
        let full_content = result.content.unwrap_or_default();
        let full_size = full_content.len() as u64;

        let content = match (args.offset, args.limit) {
            (Some(offset), Some(limit)) => {
                let start = (offset as usize).min(full_content.len());
                let end = (start + limit as usize).min(full_content.len());
                full_content[start..end].to_string()
            }
            (Some(offset), None) => {
                let start = (offset as usize).min(full_content.len());
                full_content[start..].to_string()
            }
            (None, Some(limit)) => {
                let end = (limit as usize).min(full_content.len());
                full_content[..end].to_string()
            }
            (None, None) => full_content,
        };

        let message = result.message.clone();
        notify_tool_result(Self::NAME, &message, result.success);

        Ok(FileReadOutput {
            success: result.success,
            path: args.path,
            content,
            size: full_size,
            message,
        })
    }
}
