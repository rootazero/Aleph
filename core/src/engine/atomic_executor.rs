//! Atomic Executor - Executes atomic operations with security checks
//!
//! This module implements the executor that runs atomic operations (Read, Write, Edit, Bash)
//! with proper security validation and error handling.
//!
//! ## Architecture
//!
//! The executor uses a composition pattern with specialized handlers:
//! - **FileOpsHandler**: Read, Write, Move operations
//! - **EditOpsHandler**: Edit, Replace operations
//! - **BashOpsHandler**: Shell command execution
//! - **SearchOpsHandler**: Search operations
//!
//! All handlers share a common **ExecutorContext** for working directory and security checks.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::debug;

use super::atomic::{
    ExecutorContext, FileOpsHandler, EditOpsHandler,
    BashOpsHandler, SearchOpsHandler,
    FileOps, EditOps, BashOps, SearchOps,
};
use super::AtomicAction;
use crate::error::Result;

/// Atomic operation result
#[derive(Debug, Clone)]
pub struct AtomicResult {
    /// Whether the operation succeeded
    pub success: bool,
    /// Output from the operation
    pub output: String,
    /// Error message if operation failed
    pub error: Option<String>,
}

/// Atomic operation executor
///
/// Delegates operations to specialized handlers using composition pattern.
pub struct AtomicExecutor {
    /// Shared execution context
    #[allow(dead_code)]
    context: Arc<ExecutorContext>,

    /// File operations handler
    file_ops: FileOpsHandler,

    /// Edit operations handler
    edit_ops: EditOpsHandler,

    /// Bash operations handler
    bash_ops: BashOpsHandler,

    /// Search operations handler
    search_ops: SearchOpsHandler,
}

impl AtomicExecutor {
    /// Create a new atomic executor
    ///
    /// # Arguments
    ///
    /// * `working_dir` - The working directory for path resolution
    pub fn new(working_dir: PathBuf) -> Self {
        let context = Arc::new(ExecutorContext::new(working_dir));

        Self {
            context: context.clone(),
            file_ops: FileOpsHandler::new(context.clone(), 10 * 1024 * 1024), // 10MB
            edit_ops: EditOpsHandler::new(context.clone(), 10 * 1024 * 1024), // 10MB
            bash_ops: BashOpsHandler::new(context.clone(), Duration::from_secs(30)),
            search_ops: SearchOpsHandler::new(context.clone()),
        }
    }

    /// Execute an atomic action
    ///
    /// Delegates to the appropriate handler based on action type.
    pub async fn execute(&self, action: &AtomicAction) -> Result<AtomicResult> {
        debug!(action = ?action, "Executing atomic action");

        let result = match action {
            AtomicAction::Read { path, range } => {
                self.file_ops.read(path, range.as_ref()).await?
            }
            AtomicAction::Write { path, content, mode } => {
                self.file_ops.write(path, content, mode).await?
            }
            AtomicAction::Move { source, destination, update_imports, create_parent } => {
                // Convert PathBuf to &str for the handler
                let source_str = source.to_str().unwrap_or("");
                let dest_str = destination.to_str().unwrap_or("");
                self.file_ops.move_file(source_str, dest_str, *update_imports, *create_parent).await?
            }
            AtomicAction::Edit { path, patches } => {
                self.edit_ops.edit(path, patches).await?
            }
            AtomicAction::Replace { search, replacement, scope, preview, dry_run } => {
                self.edit_ops.replace(search, replacement, scope, *preview, *dry_run).await?
            }
            AtomicAction::Bash { command, cwd } => {
                self.bash_ops.execute(command, cwd.as_deref()).await?
            }
            AtomicAction::Search { pattern, scope, filters } => {
                self.search_ops.search(pattern, scope, filters).await?
            }
        };

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;

    fn create_test_executor() -> (AtomicExecutor, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let executor = AtomicExecutor::new(temp_dir.path().to_path_buf());
        (executor, temp_dir)
    }

    #[tokio::test]
    async fn test_execute_read() {
        let (executor, temp_dir) = create_test_executor();

        // Create a test file
        let test_file = temp_dir.path().join("test.txt");
        fs::write(&test_file, "Hello, World!\nLine 2\nLine 3\n").unwrap();

        // Read entire file
        let action = AtomicAction::Read {
            path: "test.txt".to_string(),
            range: None,
        };

        let result = executor.execute(&action).await.unwrap();
        assert!(result.success);
        assert_eq!(result.output, "Hello, World!\nLine 2\nLine 3\n");
    }

    #[tokio::test]
    async fn test_execute_write() {
        let (executor, temp_dir) = create_test_executor();

        // Write a file
        let action = AtomicAction::Write {
            path: "output.txt".to_string(),
            content: "Test content".to_string(),
            mode: super::super::WriteMode::Overwrite,
        };

        let result = executor.execute(&action).await.unwrap();
        assert!(result.success);

        // Verify file was written
        let content = fs::read_to_string(temp_dir.path().join("output.txt")).unwrap();
        assert_eq!(content, "Test content");
    }

    #[tokio::test]
    async fn test_execute_bash() {
        let (executor, _temp_dir) = create_test_executor();

        // Execute a simple command
        let action = AtomicAction::Bash {
            command: "echo 'Hello from bash'".to_string(),
            cwd: None,
        };

        let result = executor.execute(&action).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("Hello from bash"));
    }

    #[tokio::test]
    async fn test_execute_move() {
        let (executor, temp_dir) = create_test_executor();

        // Create a source file
        let source = temp_dir.path().join("source.txt");
        fs::write(&source, "content").unwrap();

        let dest = temp_dir.path().join("dest.txt");

        // Move file
        let action = AtomicAction::Move {
            source: source.clone(),
            destination: dest.clone(),
            update_imports: false,
            create_parent: false,
        };

        let result = executor.execute(&action).await.unwrap();
        assert!(result.success);

        // Verify source no longer exists
        assert!(!source.exists());

        // Verify destination exists
        assert!(dest.exists());
        let content = fs::read_to_string(&dest).unwrap();
        assert_eq!(content, "content");
    }

    #[tokio::test]
    async fn test_execute_move_directory() {
        let (executor, temp_dir) = create_test_executor();

        // Create a source directory with files
        let source_dir = temp_dir.path().join("source_dir");
        fs::create_dir(&source_dir).unwrap();
        fs::write(source_dir.join("file1.txt"), "content1\n").unwrap();
        fs::write(source_dir.join("file2.txt"), "content2\n").unwrap();

        let dest_dir = temp_dir.path().join("dest_dir");

        // Move directory
        let action = AtomicAction::Move {
            source: source_dir.clone(),
            destination: dest_dir.clone(),
            update_imports: false,
            create_parent: false,
        };

        let result = executor.execute(&action).await.unwrap();
        assert!(result.success);

        // Verify source directory no longer exists
        assert!(!source_dir.exists());

        // Verify destination directory exists with files
        assert!(dest_dir.exists());
        assert!(dest_dir.join("file1.txt").exists());
        assert!(dest_dir.join("file2.txt").exists());
    }

    #[tokio::test]
    async fn test_execute_move_with_import_updates() {
        let (executor, temp_dir) = create_test_executor();

        // Create a Rust source file
        let source = temp_dir.path().join("old_module.rs");
        fs::write(&source, "pub fn hello() {}\n").unwrap();

        // Create a file that imports the module
        let importer = temp_dir.path().join("main.rs");
        fs::write(&importer, "use old_module::hello;\n\nfn main() {}\n").unwrap();

        let dest = temp_dir.path().join("new_module.rs");

        // Move file with import updates
        let action = AtomicAction::Move {
            source: source.clone(),
            destination: dest.clone(),
            update_imports: true,
            create_parent: false,
        };

        let result = executor.execute(&action).await.unwrap();
        assert!(result.success);

        // Verify source no longer exists
        assert!(!source.exists());

        // Verify destination exists
        assert!(dest.exists());

        // Verify imports were updated
        let importer_content = fs::read_to_string(&importer).unwrap();
        assert!(importer_content.contains("use new_module::hello"));
        assert!(!importer_content.contains("use old_module::hello"));
    }
}
