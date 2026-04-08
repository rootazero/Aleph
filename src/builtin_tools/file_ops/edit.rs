//! FileEditTool — string replacement editing tool
//!
//! Performs exact string replacements in files, aligned with claude-code's FileEditTool.

use std::path::Path;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use super::path_utils::{check_and_resolve_path, get_denied_paths};
use crate::builtin_tools::error::ToolError;
use crate::error::Result;
use crate::tools::AlephTool;

// =============================================================================
// Args & Output
// =============================================================================

/// Arguments for the file_edit tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FileEditArgs {
    /// Absolute or relative path to the file to edit
    pub file_path: String,
    /// The exact string to find in the file
    pub old_string: String,
    /// The replacement string
    pub new_string: String,
    /// Replace all occurrences (default: false — single match only)
    #[serde(default)]
    pub replace_all: bool,
}

/// Output from the file_edit tool
#[derive(Debug, Clone, Serialize)]
pub struct FileEditOutput {
    /// Whether the edit succeeded
    pub success: bool,
    /// Resolved canonical path of the edited file
    pub path: String,
    /// Number of replacements performed
    pub replacements: usize,
    /// Human-readable result message
    pub message: String,
}

// =============================================================================
// Tool struct
// =============================================================================

/// String-replacement file editing tool
pub struct FileEditTool {
    /// Denied path patterns (security)
    denied_paths: Vec<String>,
    /// Optional ToolContext handle for workspace-scoped output path resolution
    tool_context_handle: Option<crate::tools::ToolContextHandle>,
}

impl FileEditTool {
    /// Create a new FileEditTool with default denied paths
    pub fn new() -> Self {
        let denied_paths = get_denied_paths();
        info!(
            denied_paths_count = denied_paths.len(),
            "FileEditTool: initialized"
        );
        Self {
            denied_paths,
            tool_context_handle: None,
        }
    }

    /// Configure the tool to use a ToolContext handle for workspace-scoped output paths
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

    /// Internal implementation
    async fn call_impl(&self, args: FileEditArgs) -> std::result::Result<FileEditOutput, ToolError> {
        use crate::builtin_tools::{notify_tool_result, notify_tool_start};

        // Notify start
        let summary = format!("edit: {}", &args.file_path);
        notify_tool_start("file_edit", &summary);

        // Validate: old_string must differ from new_string
        if args.old_string == args.new_string {
            let err = ToolError::InvalidArgs(
                "old_string and new_string are identical; nothing to change".to_string(),
            );
            notify_tool_result("file_edit", &err.to_string(), false);
            return Err(err);
        }

        // Resolve & validate path
        let output_dir = self.resolve_output_dir().await;
        let output_dir_ref = output_dir.as_deref();
        let canonical = check_and_resolve_path(
            Path::new(&args.file_path),
            &self.denied_paths,
            output_dir_ref,
        )?;

        info!(path = %canonical.display(), "FileEditTool: reading file");

        // Read current content
        let content = std::fs::read_to_string(&canonical).map_err(|e| {
            ToolError::Execution(format!("Failed to read {}: {}", canonical.display(), e))
        })?;

        // Count matches
        let match_count = content.matches(&args.old_string).count();

        if match_count == 0 {
            let err = ToolError::Execution(
                "old_string not found in file, make sure it matches exactly".to_string(),
            );
            notify_tool_result("file_edit", &err.to_string(), false);
            return Err(err);
        }

        if match_count > 1 && !args.replace_all {
            let err = ToolError::Execution(format!(
                "Found {} matches of old_string; provide more context to make it unique or set replace_all=true",
                match_count
            ));
            notify_tool_result("file_edit", &err.to_string(), false);
            return Err(err);
        }

        // Perform replacement
        let (new_content, replacements) = if args.replace_all {
            let replaced = content.replace(&args.old_string, &args.new_string);
            (replaced, match_count)
        } else {
            let replaced = content.replacen(&args.old_string, &args.new_string, 1);
            (replaced, 1)
        };

        // Write back
        std::fs::write(&canonical, &new_content).map_err(|e| {
            ToolError::Execution(format!("Failed to write {}: {}", canonical.display(), e))
        })?;

        let path_str = canonical.to_string_lossy().to_string();
        let message = format!(
            "Replaced {} occurrence{} in {}",
            replacements,
            if replacements == 1 { "" } else { "s" },
            path_str
        );

        info!(replacements, path = %path_str, "FileEditTool: edit complete");
        notify_tool_result("file_edit", &message, true);

        Ok(FileEditOutput {
            success: true,
            path: path_str,
            replacements,
            message,
        })
    }
}

impl Default for FileEditTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for FileEditTool {
    fn clone(&self) -> Self {
        Self {
            denied_paths: self.denied_paths.clone(),
            tool_context_handle: self.tool_context_handle.clone(),
        }
    }
}

// =============================================================================
// AlephTool impl
// =============================================================================

#[async_trait]
impl AlephTool for FileEditTool {
    const NAME: &'static str = "file_edit";
    const DESCRIPTION: &'static str = r#"Perform exact string replacement in a file.

Finds `old_string` in the file and replaces it with `new_string`.
- By default, `old_string` must match exactly once; if multiple matches exist the call fails.
- Set `replace_all=true` to replace every occurrence.

Use this tool for surgical edits — it only changes what you specify, leaving the rest of the file intact."#;

    type Args = FileEditArgs;
    type Output = FileEditOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.call_impl(args).await.map_err(Into::into)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_single_replacement() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, "Hello World").unwrap();

        let tool = FileEditTool::new();
        let args = FileEditArgs {
            file_path: file.to_string_lossy().to_string(),
            old_string: "World".to_string(),
            new_string: "Rust".to_string(),
            replace_all: false,
        };

        let result = AlephTool::call(&tool, args).await.unwrap();
        assert!(result.success);
        assert_eq!(result.replacements, 1);
        assert_eq!(fs::read_to_string(&file).unwrap(), "Hello Rust");
    }

    #[tokio::test]
    async fn test_replace_all() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, "aaa bbb aaa").unwrap();

        let tool = FileEditTool::new();
        let args = FileEditArgs {
            file_path: file.to_string_lossy().to_string(),
            old_string: "aaa".to_string(),
            new_string: "ccc".to_string(),
            replace_all: true,
        };

        let result = AlephTool::call(&tool, args).await.unwrap();
        assert!(result.success);
        assert_eq!(result.replacements, 2);
        assert_eq!(fs::read_to_string(&file).unwrap(), "ccc bbb ccc");
    }

    #[tokio::test]
    async fn test_multiple_matches_without_replace_all_fails() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, "foo bar foo").unwrap();

        let tool = FileEditTool::new();
        let args = FileEditArgs {
            file_path: file.to_string_lossy().to_string(),
            old_string: "foo".to_string(),
            new_string: "baz".to_string(),
            replace_all: false,
        };

        let result = AlephTool::call(&tool, args).await;
        assert!(result.is_err());
        // File should be unchanged
        assert_eq!(fs::read_to_string(&file).unwrap(), "foo bar foo");
    }

    #[tokio::test]
    async fn test_old_string_not_found() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, "Hello World").unwrap();

        let tool = FileEditTool::new();
        let args = FileEditArgs {
            file_path: file.to_string_lossy().to_string(),
            old_string: "NotHere".to_string(),
            new_string: "Replaced".to_string(),
            replace_all: false,
        };

        let result = AlephTool::call(&tool, args).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_identical_strings_rejected() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, "Hello").unwrap();

        let tool = FileEditTool::new();
        let args = FileEditArgs {
            file_path: file.to_string_lossy().to_string(),
            old_string: "Hello".to_string(),
            new_string: "Hello".to_string(),
            replace_all: false,
        };

        let result = AlephTool::call(&tool, args).await;
        assert!(result.is_err());
    }
}
