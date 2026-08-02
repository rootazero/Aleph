//! `FileOpsTool` implementation with `AlephTool` trait

use std::path::Path;

use async_trait::async_trait;
use tracing::info;

use super::batch::{execute_batch_move, execute_organize};
use super::ops::{execute_copy, execute_delete, execute_list, execute_mkdir, execute_move};
use super::path_utils::{check_and_resolve_path, get_denied_paths};
use super::search::execute_search;
use super::stats::execute_stats;
use super::types::{FileOperation, FileOpsArgs, FileOpsOutput};
use crate::builtin_tools::error::ToolError;
use crate::error::Result;
use crate::tools::AlephTool;

/// File operations tool
pub struct FileOpsTool {
    /// Denied path patterns (security)
    denied_paths: Vec<String>,
    /// Optional `ToolContext` handle for workspace-scoped output path resolution
    tool_context_handle: Option<crate::tools::ToolContextHandle>,
}

impl FileOpsTool {
    /// Tool identifier
    pub const NAME: &'static str = "file_ops";

    /// Create a new `FileOpsTool` with default settings
    pub fn new() -> Self {
        let denied_paths = get_denied_paths();
        info!(
            denied_paths_count = denied_paths.len(),
            "FileOpsTool: initialized with denied_paths"
        );

        Self {
            denied_paths,
            tool_context_handle: None,
        }
    }

    /// Configure the tool to use a `ToolContext` handle for workspace-scoped output paths
    pub fn with_tool_context(mut self, handle: crate::tools::ToolContextHandle) -> Self {
        self.tool_context_handle = Some(handle);
        self
    }

    /// Resolve the output directory from the `ToolContext` handle (if available).
    /// Returns the workspace `documents/` subdirectory when a handle is wired in.
    async fn resolve_output_dir(&self) -> Option<std::path::PathBuf> {
        if let Some(ref handle) = self.tool_context_handle {
            let ctx = handle.read().await;
            Some(ctx.output_dir.join("documents"))
        } else {
            None
        }
    }

    /// Check if path is allowed (exposed for testing)
    pub fn check_path(&self, path: &Path) -> std::result::Result<std::path::PathBuf, ToolError> {
        check_and_resolve_path(path, &self.denied_paths, None)
    }

    /// Execute file operation based on args (internal implementation)
    async fn call_impl(&self, args: FileOpsArgs) -> std::result::Result<FileOpsOutput, ToolError> {
        use crate::builtin_tools::{notify_tool_result, notify_tool_start};

        // Format operation description for notification
        let op_name = match &args.operation {
            FileOperation::List => "列出目录",
            FileOperation::Move => "移动文件",
            FileOperation::Copy => "复制文件",
            FileOperation::Delete => "删除文件",
            FileOperation::Mkdir => "创建目录",
            FileOperation::Search => "搜索文件",
            FileOperation::BatchMove => "批量移动",
            FileOperation::Organize => "整理文件",
            FileOperation::Stats => "统计行数",
        };

        // Notify tool start with operation details
        let args_summary = format!("{}: {}", op_name, &args.path);
        notify_tool_start(Self::NAME, &args_summary);

        info!(
            operation = ?args.operation,
            path = %args.path,
            destination = ?args.destination,
            "FileOpsTool::call invoked"
        );

        let path = Path::new(&args.path);

        // Resolve workspace-scoped output directory from ToolContext (if wired in).
        // This overrides the global get_output_dir() for relative path resolution.
        let output_dir = self.resolve_output_dir().await;
        let output_dir_ref = output_dir.as_deref();

        let result = match args.operation {
            FileOperation::List => {
                execute_list(path, &self.denied_paths, output_dir_ref, args.limit).await
            }
            FileOperation::Move => {
                let dest = args.destination.ok_or_else(|| {
                    ToolError::InvalidArgs("Destination required for move operation".to_string())
                })?;
                execute_move(
                    path,
                    Path::new(&dest),
                    args.create_parents,
                    &self.denied_paths,
                    output_dir_ref,
                )
                .await
            }
            FileOperation::Copy => {
                let dest = args.destination.ok_or_else(|| {
                    ToolError::InvalidArgs("Destination required for copy operation".to_string())
                })?;
                execute_copy(
                    path,
                    Path::new(&dest),
                    args.create_parents,
                    &self.denied_paths,
                    output_dir_ref,
                )
                .await
            }
            FileOperation::Delete => execute_delete(path, &self.denied_paths, output_dir_ref).await,
            FileOperation::Mkdir => {
                execute_mkdir(
                    path,
                    args.create_parents,
                    &self.denied_paths,
                    output_dir_ref,
                )
                .await
            }
            FileOperation::Search => {
                let pattern = args.pattern.ok_or_else(|| {
                    ToolError::InvalidArgs("Pattern required for search operation".to_string())
                })?;
                execute_search(
                    path,
                    &pattern,
                    &self.denied_paths,
                    output_dir_ref,
                    args.limit,
                )
                .await
            }
            FileOperation::BatchMove => {
                let pattern = args.pattern.ok_or_else(|| {
                    ToolError::InvalidArgs(
                        "Pattern required for batch_move operation (e.g., '*.jpg')".to_string(),
                    )
                })?;
                let dest = args.destination.ok_or_else(|| {
                    ToolError::InvalidArgs(
                        "Destination required for batch_move operation".to_string(),
                    )
                })?;
                execute_batch_move(
                    path,
                    &pattern,
                    Path::new(&dest),
                    args.create_parents,
                    &self.denied_paths,
                    output_dir_ref,
                )
                .await
            }
            FileOperation::Organize => {
                execute_organize(path, &self.denied_paths, output_dir_ref).await
            }
            FileOperation::Stats => {
                execute_stats(
                    path,
                    args.pattern.as_deref(),
                    &self.denied_paths,
                    output_dir_ref,
                    args.limit,
                )
                .await
            }
        };

        // Notify result
        match &result {
            Ok(output) => {
                notify_tool_result(Self::NAME, &output.message, output.success);
            }
            Err(e) => {
                notify_tool_result(Self::NAME, &e.to_string(), false);
            }
        }

        result
    }
}

impl Default for FileOpsTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for FileOpsTool {
    fn clone(&self) -> Self {
        Self {
            denied_paths: self.denied_paths.clone(),
            tool_context_handle: self.tool_context_handle.clone(),
        }
    }
}

/// Implementation of rig's Tool trait for `FileOpsTool`
/// Implementation of `AlephTool` trait for `FileOpsTool`
#[async_trait]
impl AlephTool for FileOpsTool {
    const NAME: &'static str = "file_ops";
    const DESCRIPTION: &'static str = r#"Perform file system operations (excluding read/write — use file_read / file_write instead). Operations:
- list: List directory contents with file types and sizes
- move: Move/rename single file or directory
- copy: Copy single file or directory
- delete: Delete file or directory
- mkdir: Create directory
- search: Search files by glob pattern (e.g., "*.pdf", "**/*.jpg")
- stats: Recursive line/byte counts. Returns an aggregate {total_files, total_lines, total_bytes} plus per-file FileInfo {size, lines}. Use this to answer "how many lines / files / bytes are in this directory" — DO NOT loop over file_read for that.
- batch_move: Move ALL files matching a pattern to destination (e.g., pattern="*.jpg" moves all JPGs)
- organize: Auto-organize files by type into categorized folders (Images, Documents, Videos, Audio, Archives, Code, Apps, Others)

LISTING LIMITS (list / search / stats):
- At most 500 entries are returned by default; `limit` raises it. The `message` says when the cap was reached and what to pass.
- `stats` aggregates over EVERY match regardless of the cap, so its totals are exact even when the per-file list is capped.
- Recursive walks skip generated/VCS directories (.git, node_modules, target, dist, build, .venv, __pycache__, …). Name one in `pattern` to include it, e.g. pattern="target/**/*.rs".

PATH RESOLUTION:
- Relative paths → resolved against the active workspace scope; rejected if no scope is set
- Home paths (e.g., "~/Desktop/file.txt") → expanded to user's home directory
- Absolute paths (e.g., "/Users/name/file.txt") → used as-is

IMPORTANT: For organizing multiple files, use 'organize' or 'batch_move' instead of multiple 'move' calls!"#;

    type Args = FileOpsArgs;
    type Output = FileOpsOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.call_impl(args).await.map_err(Into::into)
    }
}
