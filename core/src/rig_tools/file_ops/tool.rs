//! FileOpsTool implementation with AetherTool trait

use std::path::Path;

use async_trait::async_trait;
use tracing::info;

use crate::error::Result;
use crate::rig_tools::error::ToolError;
use crate::tools::AetherTool;
use super::batch::{execute_batch_move, execute_organize};
use super::ops::{
    execute_copy, execute_delete, execute_list, execute_mkdir, execute_move, execute_read,
    execute_write,
};
use super::path_utils::{check_and_resolve_path, get_denied_paths};
use super::search::execute_search;
use super::types::{FileOperation, FileOpsArgs, FileOpsOutput};

/// File operations tool
pub struct FileOpsTool {
    /// Maximum file size for read operations (100MB default)
    max_read_size: u64,
    /// Denied path patterns (security)
    denied_paths: Vec<String>,
}

impl FileOpsTool {
    /// Tool identifier
    pub const NAME: &'static str = "file_ops";

    /// Tool description for AI prompt
    pub const DESCRIPTION: &'static str = r#"Perform file system operations. Operations:
- list: List directory contents with file types and sizes
- read: Read file content (text files only)
- write: Write content to file
- move: Move/rename single file or directory
- copy: Copy single file or directory
- delete: Delete file or directory
- mkdir: Create directory
- search: Search files by glob pattern (e.g., "*.pdf", "**/*.jpg")
- batch_move: Move ALL files matching a pattern to destination (e.g., pattern="*.jpg" moves all JPGs)
- organize: Auto-organize files by type into categorized folders (Images, Documents, Videos, Audio, Archives, Code, Others)

PATH RESOLUTION:
- Relative paths (e.g., "output.pdf", "images/photo.jpg") → resolved to ~/.aether/output/
- Home paths (e.g., "~/Desktop/file.txt") → expanded to user's home directory
- Absolute paths (e.g., "/Users/name/file.txt") → used as-is

DEFAULT OUTPUT: When generating files (PDFs, images, translations), use relative paths like "article.pdf" or "translated.txt". They will be saved to the default output directory (~/.aether/output/), which is always writable.

IMPORTANT: For organizing multiple files, use 'organize' or 'batch_move' instead of multiple 'move' calls!"#;

    /// Create a new FileOpsTool with default settings
    pub fn new() -> Self {
        let denied_paths = get_denied_paths();
        info!(denied_paths_count = denied_paths.len(), "FileOpsTool: initialized with denied_paths");

        Self {
            max_read_size: 100 * 1024 * 1024, // 100MB
            denied_paths,
        }
    }

    /// Check if path is allowed (exposed for testing)
    pub fn check_path(&self, path: &Path) -> std::result::Result<std::path::PathBuf, ToolError> {
        check_and_resolve_path(path, &self.denied_paths)
    }

    /// Execute file operation based on args (internal implementation)
    async fn call_impl(&self, args: FileOpsArgs) -> std::result::Result<FileOpsOutput, ToolError> {
        use crate::rig_tools::{notify_tool_result, notify_tool_start};

        // Format operation description for notification
        let op_name = match &args.operation {
            FileOperation::List => "列出目录",
            FileOperation::Read => "读取文件",
            FileOperation::Write => "写入文件",
            FileOperation::Move => "移动文件",
            FileOperation::Copy => "复制文件",
            FileOperation::Delete => "删除文件",
            FileOperation::Mkdir => "创建目录",
            FileOperation::Search => "搜索文件",
            FileOperation::BatchMove => "批量移动",
            FileOperation::Organize => "整理文件",
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

        let result = match args.operation {
            FileOperation::List => execute_list(path, &self.denied_paths).await,
            FileOperation::Read => {
                execute_read(path, &self.denied_paths, self.max_read_size).await
            }
            FileOperation::Write => {
                let content = args.content.ok_or_else(|| {
                    ToolError::InvalidArgs("Content required for write operation".to_string())
                })?;
                execute_write(path, &content, args.create_parents, &self.denied_paths).await
            }
            FileOperation::Move => {
                let dest = args.destination.ok_or_else(|| {
                    ToolError::InvalidArgs("Destination required for move operation".to_string())
                })?;
                execute_move(path, Path::new(&dest), args.create_parents, &self.denied_paths).await
            }
            FileOperation::Copy => {
                let dest = args.destination.ok_or_else(|| {
                    ToolError::InvalidArgs("Destination required for copy operation".to_string())
                })?;
                execute_copy(path, Path::new(&dest), args.create_parents, &self.denied_paths).await
            }
            FileOperation::Delete => execute_delete(path, &self.denied_paths).await,
            FileOperation::Mkdir => {
                execute_mkdir(path, args.create_parents, &self.denied_paths).await
            }
            FileOperation::Search => {
                let pattern = args.pattern.ok_or_else(|| {
                    ToolError::InvalidArgs("Pattern required for search operation".to_string())
                })?;
                execute_search(path, &pattern, &self.denied_paths).await
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
                )
                .await
            }
            FileOperation::Organize => {
                execute_organize(path, args.create_parents, &self.denied_paths).await
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
            max_read_size: self.max_read_size,
            denied_paths: self.denied_paths.clone(),
        }
    }
}

/// Implementation of rig's Tool trait for FileOpsTool
/// Implementation of AetherTool trait for FileOpsTool
#[async_trait]
impl AetherTool for FileOpsTool {
    const NAME: &'static str = "file_ops";
    const DESCRIPTION: &'static str = r#"Perform file system operations. Operations:
- list: List directory contents with file types and sizes
- read: Read file content (text files only)
- write: Write content to file
- move: Move/rename single file or directory
- copy: Copy single file or directory
- delete: Delete file or directory
- mkdir: Create directory
- search: Search files by glob pattern
- batch_move: Move ALL files matching a pattern to destination
- organize: Auto-organize files by type into categorized folders"#;

    type Args = FileOpsArgs;
    type Output = FileOpsOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.call_impl(args).await.map_err(Into::into)
    }
}

