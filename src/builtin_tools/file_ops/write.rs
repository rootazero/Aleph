//! Independent FileWriteTool — write content to a file
//!
//! Unlike the combined FileOpsTool, this tool makes `content` a **required** field
//! (plain `String`, not `Option<String>`), so the JSON Schema enforces its presence
//! and the LLM cannot omit or null it.

use std::path::Path;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use super::ops::execute_write;
use super::path_utils::get_denied_paths;
use crate::builtin_tools::{notify_tool_result, notify_tool_start};
use crate::error::Result;
use crate::tools::AlephTool;

// ---------------------------------------------------------------------------
// Helper for serde default
// ---------------------------------------------------------------------------

fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------------------
// Args
// ---------------------------------------------------------------------------

/// Arguments for writing a file.
///
/// Both `file_path` and `content` are **required** — the generated JSON Schema
/// will list them under `"required"`, preventing the LLM from sending null.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FileWriteArgs {
    /// Path to write to (absolute, relative, or ~-prefixed)
    pub file_path: String,

    /// The full text content to write. REQUIRED — must not be null or omitted.
    pub content: String,

    /// Create parent directories if they don't exist (default: true)
    #[serde(default = "default_true")]
    pub create_parents: bool,
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// Result of a file write operation.
#[derive(Debug, Clone, Serialize)]
pub struct FileWriteOutput {
    pub success: bool,
    pub path: String,
    pub bytes_written: u64,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Tool
// ---------------------------------------------------------------------------

/// Standalone file-write tool that enforces `content` as a required parameter.
pub struct FileWriteTool {
    denied_paths: Vec<String>,
    tool_context_handle: Option<crate::tools::ToolContextHandle>,
}

impl FileWriteTool {
    /// Create a new `FileWriteTool` with default denied paths.
    pub fn new() -> Self {
        let denied_paths = get_denied_paths();
        info!(
            denied_paths_count = denied_paths.len(),
            "FileWriteTool: initialized"
        );
        Self {
            denied_paths,
            tool_context_handle: None,
        }
    }

    /// Attach a `ToolContextHandle` for workspace-scoped output path resolution.
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

impl Default for FileWriteTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for FileWriteTool {
    fn clone(&self) -> Self {
        Self {
            denied_paths: self.denied_paths.clone(),
            tool_context_handle: self.tool_context_handle.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// AlephTool impl
// ---------------------------------------------------------------------------

#[async_trait]
impl AlephTool for FileWriteTool {
    const NAME: &'static str = "file_write";
    const DESCRIPTION: &'static str = "Write content to a file. Creates the file if it doesn't \
        exist, overwrites if it does. Both file_path and content are required parameters.";

    type Args = FileWriteArgs;
    type Output = FileWriteOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let args_summary = format!("write: {}", &args.file_path);
        notify_tool_start(Self::NAME, &args_summary);

        info!(
            file_path = %args.file_path,
            content_len = args.content.len(),
            create_parents = args.create_parents,
            "FileWriteTool::call invoked"
        );

        let path = Path::new(&args.file_path);
        let output_dir = self.resolve_output_dir().await;
        let output_dir_ref = output_dir.as_deref();

        let result = execute_write(
            path,
            &args.content,
            args.create_parents,
            &self.denied_paths,
            output_dir_ref,
        )
        .await;

        match result {
            Ok(output) => {
                let write_output = FileWriteOutput {
                    success: output.success,
                    path: output.message.clone(),
                    bytes_written: output.bytes_written.unwrap_or(0),
                    message: output.message,
                };
                notify_tool_result(Self::NAME, &write_output.message, write_output.success);
                Ok(write_output)
            }
            Err(e) => {
                let msg = e.to_string();
                notify_tool_result(Self::NAME, &msg, false);
                Err(e.into())
            }
        }
    }
}
