//! File operations tool for AI agent integration
//!
//! Implements rig's Tool trait to provide file system operations.
//! Supports: list, read, write, move, copy, delete, mkdir, search, batch_move, organize

mod batch;
mod ops;
mod path_utils;
mod search;
mod state;
mod tool;
mod types;

// Re-export public API
pub use state::{
    clear_written_files, get_working_dir, get_written_files, mark_session_start,
    record_written_file, scan_new_files_in_working_dir, set_working_dir, take_written_files,
    WrittenFile,
};
pub use tool::FileOpsTool;
pub use types::{FileInfo, FileOperation, FileOpsArgs, FileOpsOutput};

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
            content: None,
            pattern: None,
            create_parents: true,
        };

        let result = AlephTool::call(&tool, args).await.unwrap();
        assert!(result.success);
        assert_eq!(result.items_affected, Some(2));
    }

    #[tokio::test]
    async fn test_read_write_file() {
        let dir = tempdir().unwrap();
        let tool = FileOpsTool::new();
        let file_path = dir.path().join("test.txt");

        // Write
        let write_args = FileOpsArgs {
            operation: FileOperation::Write,
            path: file_path.to_string_lossy().to_string(),
            destination: None,
            content: Some("Hello, World!".to_string()),
            pattern: None,
            create_parents: true,
        };

        let result = AlephTool::call(&tool, write_args).await.unwrap();
        assert!(result.success);
        assert_eq!(result.bytes_written, Some(13));

        // Read
        let read_args = FileOpsArgs {
            operation: FileOperation::Read,
            path: file_path.to_string_lossy().to_string(),
            destination: None,
            content: None,
            pattern: None,
            create_parents: true,
        };

        let result = AlephTool::call(&tool, read_args).await.unwrap();
        assert!(result.success);
        assert_eq!(result.content, Some("Hello, World!".to_string()));
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
            content: None,
            pattern: None,
            create_parents: true,
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
            content: None,
            pattern: None,
            create_parents: true,
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
            content: None,
            pattern: Some("*.txt".to_string()),
            create_parents: true,
        };

        let result = AlephTool::call(&tool, args).await.unwrap();
        assert!(result.success);
        assert_eq!(result.items_affected, Some(2));
    }

    #[tokio::test]
    async fn test_mkdir_relative_in_output_dir() {
        // Test that relative paths resolve to the output directory correctly
        let tool = FileOpsTool::new();

        // "test_subdir" should resolve to ~/.aleph/output/test_subdir
        let args = FileOpsArgs {
            operation: FileOperation::Mkdir,
            path: "test_mkdir_relative_subdir".to_string(),
            destination: None,
            content: None,
            pattern: None,
            create_parents: true,
        };

        let result = AlephTool::call(&tool, args).await;

        // This should succeed - output directory should be writable
        match &result {
            Ok(output) => {
                assert!(output.success);
                println!("mkdir succeeded: {}", output.message);
                // Clean up - delete the created directory
                let output_dir = crate::utils::paths::get_output_dir().unwrap();
                let created_path = output_dir.join("test_mkdir_relative_subdir");
                if created_path.exists() {
                    fs::remove_dir(&created_path).ok();
                }
            }
            Err(e) => {
                panic!(
                    "mkdir for relative path should succeed in output dir, but got error: {:?}",
                    e
                );
            }
        }
    }

    #[tokio::test]
    async fn test_check_path_denies_protected() {
        let tool = FileOpsTool::new();

        // Test that protected paths are denied
        let protected_paths = vec!["~/.ssh/test", "~/.gnupg/test", "~/.aws/test"];

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
    async fn test_check_path_allows_output_subdir() {
        let tool = FileOpsTool::new();

        // Test that output subdirectories are allowed
        // Relative paths like "chapter-1" should resolve to ~/.aleph/output/chapter-1
        let result = tool.check_path(std::path::Path::new("chapter-1"));
        assert!(
            result.is_ok(),
            "Relative path 'chapter-1' should be allowed in output directory"
        );

        if let Ok(canonical) = result {
            let output_dir = crate::utils::paths::get_output_dir().unwrap();
            assert!(
                canonical.starts_with(&output_dir),
                "Resolved path should be under output directory"
            );
        }
    }
}
