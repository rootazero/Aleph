//! Independent `FileWriteTool` — write content to a file
//!
//! Unlike the combined `FileOpsTool`, this tool makes `content` a **required** field
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

const fn default_true() -> bool {
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
    /// `true` when the destination already held byte-identical content and the
    /// atomic rename was therefore skipped — the file's `mtime` is preserved
    /// in that case. Lets the caller (and the model) tell "wrote new bytes"
    /// from "rewrote the same bytes" without re-stat'ing the file.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unchanged: bool,
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

    /// Resolve the output directory from the `ToolContext` handle (if available).
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

        // Defence in depth (audit-2026-08-26 BTS-1): `file_read` caps at
        // 100 MB and `apply_patch` caps the envelope at 4 MiB, but
        // `file_write` had no symmetric guard — a multi-GB `content` string
        // allocates the full buffer before any deny-checked write begins,
        // which OOMs the worker. Mirror the read cap so the LLM-callable
        // surface is bounded consistently.
        const MAX_WRITE_CONTENT_BYTES: usize = 32 * 1024 * 1024;
        if args.content.len() > MAX_WRITE_CONTENT_BYTES {
            notify_tool_result(
                Self::NAME,
                &format!(
                    "file_write content is {} bytes; the cap is {MAX_WRITE_CONTENT_BYTES}. \
                     Use apply_patch / batch for chunked rewrites."
                ),
                false,
            );
            return Err(crate::tools::ToolError::InvalidArgs(format!(
                "file_write content is {} bytes; the cap is {MAX_WRITE_CONTENT_BYTES}. \
                 Use apply_patch / batch for chunked rewrites.",
                args.content.len()
            ))
            .into());
        }

        let result = execute_write(
            path,
            &args.content,
            args.create_parents,
            &self.denied_paths,
            output_dir_ref,
        )
        .await;

        match result {
            Ok(outcome) => {
                let path = outcome.canonical.display().to_string();
                let message = if outcome.unchanged {
                    format!(
                        "No-op: {path} already contained {} identical bytes (mtime preserved)",
                        outcome.bytes
                    )
                } else {
                    format!("Wrote {} bytes to {}", outcome.bytes, path)
                };
                let write_output = FileWriteOutput {
                    success: true,
                    path,
                    bytes_written: outcome.bytes,
                    unchanged: outcome.unchanged,
                    message,
                };
                notify_tool_result(Self::NAME, &write_output.message, true);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;
    use tempfile::tempdir;

    /// Re-writing the same bytes reports `unchanged: true` and (importantly)
    /// leaves the file's mtime alone — build systems and file watchers that
    /// key on mtime must NOT see a no-op write as a fresh change.
    #[tokio::test]
    async fn noop_write_preserves_mtime() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "same content").unwrap();

        // Sleep long enough that a real write would produce a new mtime, even
        // on a filesystem with second-resolution timestamps.
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let before = fs::metadata(&file).unwrap().modified().unwrap();

        tokio::time::sleep(Duration::from_millis(1100)).await;
        let tool = FileWriteTool::new();
        let out = AlephTool::call(
            &tool,
            FileWriteArgs {
                file_path: file.to_string_lossy().to_string(),
                content: "same content".to_string(),
                create_parents: true,
            },
        )
        .await
        .unwrap();

        assert!(out.success);
        assert!(
            out.unchanged,
            "unchanged must be true for a byte-equal rewrite"
        );
        assert_eq!(out.bytes_written, "same content".len() as u64);
        let after = fs::metadata(&file).unwrap().modified().unwrap();
        assert_eq!(before, after, "no-op write must not touch mtime");
    }

    /// A genuinely different payload reports `unchanged: false` and the file
    /// is rewritten.
    #[tokio::test]
    async fn changed_write_reports_not_unchanged() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "v1").unwrap();

        let tool = FileWriteTool::new();
        let out = AlephTool::call(
            &tool,
            FileWriteArgs {
                file_path: file.to_string_lossy().to_string(),
                content: "v2".to_string(),
                create_parents: true,
            },
        )
        .await
        .unwrap();

        assert!(out.success);
        assert!(!out.unchanged);
        assert_eq!(fs::read_to_string(&file).unwrap(), "v2");
    }

    /// The very first write of a new file is not a no-op even if the
    /// short-circuit guard could conceivably fire — the file did not exist
    /// before, so the byte-equality comparison is meaningless and the write
    /// must happen.
    #[tokio::test]
    async fn first_write_of_a_new_file_is_not_a_noop() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("fresh.txt");
        assert!(!file.exists());

        let tool = FileWriteTool::new();
        let out = AlephTool::call(
            &tool,
            FileWriteArgs {
                file_path: file.to_string_lossy().to_string(),
                content: "first".to_string(),
                create_parents: true,
            },
        )
        .await
        .unwrap();

        assert!(out.success);
        assert!(!out.unchanged);
        assert_eq!(fs::read_to_string(&file).unwrap(), "first");
    }
}
