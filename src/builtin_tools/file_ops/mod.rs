//! File operations tool for AI agent integration
//!
//! Implements rig's Tool trait to provide file system operations.
//! Supports: list, move, copy, delete, mkdir, search, stats, `batch_move`, organize

mod apply_patch;
mod batch;
pub(crate) mod edit;
mod edit_match;
mod image_read;
mod ops;
mod path_utils;
pub(crate) use path_utils::{
    check_and_resolve_path, get_denied_paths, is_blocked_proc_path, path_is_denied,
};
pub(crate) mod read;
mod read_cache;
mod search;
mod stats;
mod text;
pub(crate) use text::{clamp_line_to, is_binary};
mod tool;
mod types;
pub(crate) use types::SKIPPED_DIRS;
pub(crate) mod write;

// Re-export public API
pub use apply_patch::{ApplyPatchArgs, ApplyPatchOutput, ApplyPatchTool};
// Blast-radius extractor for the concurrency scheduler (crate-internal).
pub(crate) use apply_patch::patch_target_paths;
pub use edit::{EditOp, FileEditTool};
pub use read::FileReadTool;
pub use tool::FileOpsTool;
pub use types::{FileInfo, FileOperation, FileOpsArgs, FileOpsOutput, StatsSort};
pub use write::FileWriteTool;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::AlephTool;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_list_directory() {
        let dir = tempdir().unwrap();
        let tool = FileOpsTool::new();

        // Create test files
        fs::write(dir.path().join("test.txt"), "hello").unwrap();
        fs::create_dir(dir.path().join("subdir")).unwrap();

        let args = FileOpsArgs {
            operation: FileOperation::List,
            path: dir.path().to_string_lossy().to_string(),
            destination: None,
            pattern: None,
            create_parents: true,
            limit: None,
            sort_by: None,
        };

        let result = AlephTool::call(&tool, args).await.unwrap();
        assert!(result.success);
        assert_eq!(result.items_affected, Some(2));
    }

    #[tokio::test]
    async fn test_read_write_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");

        // Write via FileWriteTool
        let write_tool = write::FileWriteTool::new();
        let write_args = write::FileWriteArgs {
            file_path: file_path.to_string_lossy().to_string(),
            content: "Hello, World!".to_string(),
            create_parents: true,
        };
        let result = AlephTool::call(&write_tool, write_args).await.unwrap();
        assert!(result.success);
        assert_eq!(result.bytes_written, 13);

        // Read via FileReadTool
        let read_tool = read::FileReadTool::new();
        let read_args = read::FileReadArgs {
            path: file_path.to_string_lossy().to_string(),
            offset: None,
            limit: None,
        };
        let result = AlephTool::call(&read_tool, read_args).await.unwrap();
        assert!(result.success);
        // `file_read` renders `cat -n`-style line numbers; the file body is
        // present within the numbered output.
        assert!(
            result.content.contains("Hello, World!"),
            "content was: {:?}",
            result.content
        );
        assert_eq!(result.total_lines, 1);
    }

    #[tokio::test]
    async fn test_mkdir() {
        let dir = tempdir().unwrap();
        let tool = FileOpsTool::new();
        let new_dir = dir.path().join("new").join("nested").join("dir");

        let args = FileOpsArgs {
            operation: FileOperation::Mkdir,
            path: new_dir.to_string_lossy().to_string(),
            destination: None,
            pattern: None,
            create_parents: true,
            limit: None,
            sort_by: None,
        };

        let result = AlephTool::call(&tool, args).await.unwrap();
        assert!(result.success);
        assert!(new_dir.exists());
    }

    #[tokio::test]
    async fn test_move_file() {
        let dir = tempdir().unwrap();
        let tool = FileOpsTool::new();

        let from = dir.path().join("original.txt");
        let to = dir.path().join("moved.txt");

        fs::write(&from, "test content").unwrap();

        let args = FileOpsArgs {
            operation: FileOperation::Move,
            path: from.to_string_lossy().to_string(),
            destination: Some(to.to_string_lossy().to_string()),
            pattern: None,
            create_parents: true,
            limit: None,
            sort_by: None,
        };

        let result = AlephTool::call(&tool, args).await.unwrap();
        assert!(result.success);
        assert!(!from.exists());
        assert!(to.exists());
    }

    #[tokio::test]
    async fn test_search() {
        let dir = tempdir().unwrap();
        let tool = FileOpsTool::new();

        // Create test files
        fs::write(dir.path().join("test1.txt"), "").unwrap();
        fs::write(dir.path().join("test2.txt"), "").unwrap();
        fs::write(dir.path().join("other.pdf"), "").unwrap();

        let args = FileOpsArgs {
            operation: FileOperation::Search,
            path: dir.path().to_string_lossy().to_string(),
            destination: None,
            pattern: Some("*.txt".to_string()),
            create_parents: true,
            limit: None,
            sort_by: None,
        };

        let result = AlephTool::call(&tool, args).await.unwrap();
        assert!(result.success);
        assert_eq!(result.items_affected, Some(2));
    }

    #[tokio::test]
    async fn test_mkdir_relative_in_output_dir() {
        // Test that mkdir works correctly with an absolute path to a temp directory.
        // Relative paths without a working directory or output_dir override are rejected.
        let dir = tempdir().unwrap();
        let tool = FileOpsTool::new();
        let new_dir = dir.path().join("test_mkdir_relative_subdir");

        let args = FileOpsArgs {
            operation: FileOperation::Mkdir,
            path: new_dir.to_string_lossy().to_string(),
            destination: None,
            pattern: None,
            create_parents: true,
            limit: None,
            sort_by: None,
        };

        let result = AlephTool::call(&tool, args).await;
        assert!(result.is_ok());
        assert!(result.unwrap().success);
        assert!(new_dir.exists());
    }

    #[tokio::test]
    async fn test_check_path_denies_protected() {
        let tool = FileOpsTool::new();

        // Test that protected paths are denied — the original SSH/PGP/AWS set
        // plus the broadened cloud/registry/secret-store credential coverage.
        let protected_paths = vec![
            "~/.ssh/test",
            "~/.gnupg/test",
            "~/.aws/test",
            "~/.config/gcloud/credentials.db",
            "~/.kube/config",
            "~/.azure/accessTokens.json",
            "~/.docker/config.json",
            "~/.npmrc",
            "~/.pypirc",
            "~/.password-store/x.gpg",
            "~/.netrc",
            "~/.git-credentials",
        ];

        for path in protected_paths {
            let result = tool.check_path(std::path::Path::new(path));
            assert!(
                result.is_err(),
                "Path {} should be denied but was allowed",
                path
            );
        }
    }

    #[tokio::test]
    async fn test_check_path_allows_absolute_subdir() {
        let dir = tempdir().unwrap();
        let tool = FileOpsTool::new();

        // Absolute paths within a non-denied directory should be allowed.
        let abs_path = dir.path().join("chapter-1");
        let result = tool.check_path(&abs_path);
        assert!(
            result.is_ok(),
            "Absolute path under a temp directory should be allowed, got: {:?}",
            result
        );

        // Relative paths without a working directory or output_dir override are rejected.
        let rel_result = tool.check_path(std::path::Path::new("chapter-1"));
        assert!(
            rel_result.is_err(),
            "Relative path without a working dir should be rejected"
        );
    }
}
