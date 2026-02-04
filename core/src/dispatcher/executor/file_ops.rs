//! File operations executor
//!
//! Implements the TaskExecutor trait for file system operations.
//! Supports read, write, move, copy, delete, search, and list operations.

use std::fs::{self, File, OpenOptions};
use std::io::{Read as IoRead, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::time::{Instant, UNIX_EPOCH};

use async_trait::async_trait;
use glob::glob;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{debug, info};

use super::permission::PathPermissionChecker;
use super::{ExecutionContext, TaskExecutor};
use crate::dispatcher::agent_types::{FileOp, Task, TaskResult, TaskType};
use crate::dispatcher::{
    DEFAULT_MAX_FILE_SIZE, DEFAULT_REQUIRE_CONFIRMATION_FOR_DELETE,
    DEFAULT_REQUIRE_CONFIRMATION_FOR_WRITE,
};
use crate::error::{AlephError, Result};

/// File metadata returned by operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMetadata {
    pub path: PathBuf,
    pub size: u64,
    pub is_dir: bool,
    pub is_file: bool,
    pub modified: Option<u64>,
    pub created: Option<u64>,
    pub readonly: bool,
}

impl FileMetadata {
    /// Create metadata from a path
    pub fn from_path(path: &Path) -> std::io::Result<Self> {
        let metadata = fs::metadata(path)?;

        let modified = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs());

        let created = metadata
            .created()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs());

        Ok(Self {
            path: path.to_path_buf(),
            size: metadata.len(),
            is_dir: metadata.is_dir(),
            is_file: metadata.is_file(),
            modified,
            created,
            readonly: metadata.permissions().readonly(),
        })
    }
}

/// Result of a file read operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadResult {
    pub content: String,
    pub metadata: FileMetadata,
    pub encoding: String,
}

/// Result of a file write operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteResult {
    pub path: PathBuf,
    pub bytes_written: u64,
    pub created: bool,
}

/// Result of a move/copy operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveResult {
    pub from: PathBuf,
    pub to: PathBuf,
    pub bytes: u64,
}

/// Result of a delete operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteResult {
    pub path: PathBuf,
    pub was_dir: bool,
    pub items_deleted: usize,
}

/// Result of a search operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub pattern: String,
    pub matches: Vec<FileMetadata>,
    pub total_matches: usize,
}

/// Result of a list operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListResult {
    pub path: PathBuf,
    pub entries: Vec<FileMetadata>,
    pub total_entries: usize,
}

/// File operations executor
pub struct FileOpsExecutor {
    /// Permission checker
    permission_checker: PathPermissionChecker,

    /// Require confirmation for write operations (reserved for future UI integration)
    #[allow(dead_code)]
    require_confirmation_for_write: bool,

    /// Require confirmation for delete operations (reserved for future UI integration)
    #[allow(dead_code)]
    require_confirmation_for_delete: bool,
}

impl FileOpsExecutor {
    /// Create a new file operations executor
    pub fn new(
        allowed_paths: Vec<String>,
        denied_paths: Vec<String>,
        max_file_size: u64,
        require_confirmation_for_write: bool,
        require_confirmation_for_delete: bool,
    ) -> Self {
        Self {
            permission_checker: PathPermissionChecker::new(
                allowed_paths,
                denied_paths,
                max_file_size,
            ),
            require_confirmation_for_write,
            require_confirmation_for_delete,
        }
    }

    /// Create with default settings (empty allowed paths = all allowed except denied)
    pub fn with_defaults() -> Self {
        Self::new(
            vec![],
            vec![],
            DEFAULT_MAX_FILE_SIZE,
            DEFAULT_REQUIRE_CONFIRMATION_FOR_WRITE,
            DEFAULT_REQUIRE_CONFIRMATION_FOR_DELETE,
        )
    }

    /// Execute a read operation
    async fn execute_read(&self, path: &Path, _ctx: &ExecutionContext) -> Result<TaskResult> {
        let start = Instant::now();

        // Check permission
        let canonical_path = self
            .permission_checker
            .check_path(path)
            .map_err(|e| AlephError::IoError(e.to_string()))?;

        // Check file size
        let metadata =
            fs::metadata(&canonical_path).map_err(|e| AlephError::IoError(e.to_string()))?;
        self.permission_checker
            .check_file_size(metadata.len())
            .map_err(|e| AlephError::IoError(e.to_string()))?;

        // Read file
        let mut file =
            File::open(&canonical_path).map_err(|e| AlephError::IoError(e.to_string()))?;
        let mut content = String::new();
        file.read_to_string(&mut content)
            .map_err(|e| AlephError::IoError(e.to_string()))?;

        let result = ReadResult {
            content,
            metadata: FileMetadata::from_path(&canonical_path)
                .map_err(|e| AlephError::IoError(e.to_string()))?,
            encoding: "utf-8".to_string(),
        };

        info!(
            "Read file: {:?} ({} bytes)",
            canonical_path, result.metadata.size
        );

        Ok(TaskResult::with_output(serde_json::to_value(result)?)
            .with_duration(start.elapsed())
            .with_summary(format!(
                "Read {} bytes from {:?}",
                metadata.len(),
                canonical_path
            )))
    }

    /// Execute a write operation
    async fn execute_write(
        &self,
        path: &Path,
        content: &str,
        _ctx: &ExecutionContext,
    ) -> Result<TaskResult> {
        let start = Instant::now();

        // Check permission
        let canonical_path = self
            .permission_checker
            .check_path(path)
            .map_err(|e| AlephError::IoError(e.to_string()))?;

        // Create parent directories if needed
        if let Some(parent) = canonical_path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).map_err(|e| AlephError::IoError(e.to_string()))?;
                debug!("Created parent directories: {:?}", parent);
            }
        }

        let created = !canonical_path.exists();

        // Write file
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&canonical_path)
            .map_err(|e| AlephError::IoError(e.to_string()))?;

        let bytes_written = content.len() as u64;
        file.write_all(content.as_bytes())
            .map_err(|e| AlephError::IoError(e.to_string()))?;

        let result = WriteResult {
            path: canonical_path.clone(),
            bytes_written,
            created,
        };

        info!(
            "Wrote file: {:?} ({} bytes, created={})",
            canonical_path, bytes_written, created
        );

        Ok(TaskResult::with_output(serde_json::to_value(result)?)
            .add_artifact(canonical_path.clone())
            .with_duration(start.elapsed())
            .with_summary(format!(
                "{} {} bytes to {:?}",
                if created { "Created" } else { "Updated" },
                bytes_written,
                canonical_path
            )))
    }

    /// Execute a move operation
    async fn execute_move(
        &self,
        from: &Path,
        to: &Path,
        _ctx: &ExecutionContext,
    ) -> Result<TaskResult> {
        let start = Instant::now();

        // Check permissions for both paths
        let from_canonical = self
            .permission_checker
            .check_path(from)
            .map_err(|e| AlephError::IoError(e.to_string()))?;

        let to_canonical = self
            .permission_checker
            .check_path(to)
            .map_err(|e| AlephError::IoError(e.to_string()))?;

        // Get file size before move
        let metadata =
            fs::metadata(&from_canonical).map_err(|e| AlephError::IoError(e.to_string()))?;
        let bytes = metadata.len();

        // Create parent directories if needed
        if let Some(parent) = to_canonical.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).map_err(|e| AlephError::IoError(e.to_string()))?;
            }
        }

        // Perform move
        fs::rename(&from_canonical, &to_canonical)
            .map_err(|e| AlephError::IoError(e.to_string()))?;

        let result = MoveResult {
            from: from_canonical.clone(),
            to: to_canonical.clone(),
            bytes,
        };

        info!("Moved {:?} -> {:?}", from_canonical, to_canonical);

        Ok(TaskResult::with_output(serde_json::to_value(result)?)
            .add_artifact(to_canonical.clone())
            .with_duration(start.elapsed())
            .with_summary(format!("Moved {:?} to {:?}", from_canonical, to_canonical)))
    }

    /// Execute a copy operation
    async fn execute_copy(
        &self,
        from: &Path,
        to: &Path,
        _ctx: &ExecutionContext,
    ) -> Result<TaskResult> {
        let start = Instant::now();

        // Check permissions for both paths
        let from_canonical = self
            .permission_checker
            .check_path(from)
            .map_err(|e| AlephError::IoError(e.to_string()))?;

        let to_canonical = self
            .permission_checker
            .check_path(to)
            .map_err(|e| AlephError::IoError(e.to_string()))?;

        // Create parent directories if needed
        if let Some(parent) = to_canonical.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).map_err(|e| AlephError::IoError(e.to_string()))?;
            }
        }

        // Perform copy
        let bytes = fs::copy(&from_canonical, &to_canonical)
            .map_err(|e| AlephError::IoError(e.to_string()))?;

        let result = MoveResult {
            from: from_canonical.clone(),
            to: to_canonical.clone(),
            bytes,
        };

        info!(
            "Copied {:?} -> {:?} ({} bytes)",
            from_canonical, to_canonical, bytes
        );

        Ok(TaskResult::with_output(serde_json::to_value(result)?)
            .add_artifact(to_canonical.clone())
            .with_duration(start.elapsed())
            .with_summary(format!(
                "Copied {} bytes from {:?} to {:?}",
                bytes, from_canonical, to_canonical
            )))
    }

    /// Execute a delete operation
    async fn execute_delete(&self, path: &Path, _ctx: &ExecutionContext) -> Result<TaskResult> {
        let start = Instant::now();

        // Check permission
        let canonical_path = self
            .permission_checker
            .check_path(path)
            .map_err(|e| AlephError::IoError(e.to_string()))?;

        let metadata =
            fs::metadata(&canonical_path).map_err(|e| AlephError::IoError(e.to_string()))?;
        let was_dir = metadata.is_dir();
        let mut items_deleted = 1;

        if was_dir {
            // Count items in directory
            items_deleted = fs::read_dir(&canonical_path)
                .map_err(|e| AlephError::IoError(e.to_string()))?
                .count()
                + 1;
            fs::remove_dir_all(&canonical_path).map_err(|e| AlephError::IoError(e.to_string()))?;
        } else {
            fs::remove_file(&canonical_path).map_err(|e| AlephError::IoError(e.to_string()))?;
        }

        let result = DeleteResult {
            path: canonical_path.clone(),
            was_dir,
            items_deleted,
        };

        info!(
            "Deleted {:?} (dir={}, items={})",
            canonical_path, was_dir, items_deleted
        );

        Ok(TaskResult::with_output(serde_json::to_value(result)?)
            .with_duration(start.elapsed())
            .with_summary(format!(
                "Deleted {} item(s) from {:?}",
                items_deleted, canonical_path
            )))
    }

    /// Execute a search operation
    async fn execute_search(
        &self,
        pattern: &str,
        dir: &Path,
        _ctx: &ExecutionContext,
    ) -> Result<TaskResult> {
        let start = Instant::now();

        // Check permission for search directory
        let canonical_dir = self
            .permission_checker
            .check_path(dir)
            .map_err(|e| AlephError::IoError(e.to_string()))?;

        // Build full glob pattern
        let full_pattern = canonical_dir.join(pattern);
        let pattern_str = full_pattern.to_string_lossy();

        let mut matches = Vec::new();

        for entry in glob(&pattern_str).map_err(|e| AlephError::IoError(e.to_string()))? {
            match entry {
                Ok(path) => {
                    // Check if path is allowed (not in denied paths)
                    if self.permission_checker.check_path(&path).is_ok() {
                        if let Ok(metadata) = FileMetadata::from_path(&path) {
                            matches.push(metadata);
                        }
                    }
                }
                Err(e) => {
                    debug!("Glob error for entry: {}", e);
                }
            }
        }

        let total_matches = matches.len();

        let result = SearchResult {
            pattern: pattern.to_string(),
            matches,
            total_matches,
        };

        info!(
            "Search '{}' in {:?}: {} matches",
            pattern, canonical_dir, total_matches
        );

        Ok(TaskResult::with_output(serde_json::to_value(result)?)
            .with_duration(start.elapsed())
            .with_summary(format!(
                "Found {} files matching '{}'",
                total_matches, pattern
            )))
    }

    /// Execute a list operation
    async fn execute_list(&self, path: &Path, _ctx: &ExecutionContext) -> Result<TaskResult> {
        let start = Instant::now();

        // Check permission
        let canonical_path = self
            .permission_checker
            .check_path(path)
            .map_err(|e| AlephError::IoError(e.to_string()))?;

        let mut entries = Vec::new();

        for entry in
            fs::read_dir(&canonical_path).map_err(|e| AlephError::IoError(e.to_string()))?
        {
            let entry = entry.map_err(|e| AlephError::IoError(e.to_string()))?;
            let entry_path = entry.path();

            // Check if entry path is allowed
            if self.permission_checker.check_path(&entry_path).is_ok() {
                if let Ok(metadata) = FileMetadata::from_path(&entry_path) {
                    entries.push(metadata);
                }
            }
        }

        // Sort entries: directories first, then by name
        entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.path.cmp(&b.path),
        });

        let total_entries = entries.len();

        let result = ListResult {
            path: canonical_path.clone(),
            entries,
            total_entries,
        };

        info!("Listed {:?}: {} entries", canonical_path, total_entries);

        Ok(TaskResult::with_output(serde_json::to_value(result)?)
            .with_duration(start.elapsed())
            .with_summary(format!(
                "Listed {} entries in {:?}",
                total_entries, canonical_path
            )))
    }

    /// Execute a batch move operation
    async fn execute_batch_move(
        &self,
        operations: &[(PathBuf, PathBuf)],
        _ctx: &ExecutionContext,
    ) -> Result<TaskResult> {
        let start = Instant::now();

        let mut completed = Vec::new();
        let mut errors = Vec::new();

        for (from, to) in operations {
            // Check permissions
            let from_result = self.permission_checker.check_path(from);
            let to_result = self.permission_checker.check_path(to);

            match (from_result, to_result) {
                (Ok(from_canonical), Ok(to_canonical)) => {
                    // Create parent directories
                    if let Some(parent) = to_canonical.parent() {
                        if !parent.exists() {
                            let _ = fs::create_dir_all(parent);
                        }
                    }

                    // Perform move
                    match fs::rename(&from_canonical, &to_canonical) {
                        Ok(()) => {
                            completed.push(json!({
                                "from": from_canonical,
                                "to": to_canonical,
                                "status": "success",
                            }));
                        }
                        Err(e) => {
                            errors.push(json!({
                                "from": from,
                                "to": to,
                                "error": e.to_string(),
                            }));
                        }
                    }
                }
                (Err(e), _) | (_, Err(e)) => {
                    errors.push(json!({
                        "from": from,
                        "to": to,
                        "error": e.to_string(),
                    }));
                }
            }
        }

        let success_count = completed.len();
        let error_count = errors.len();

        info!(
            "Batch move: {}/{} succeeded",
            success_count,
            operations.len()
        );

        Ok(TaskResult::with_output(json!({
            "completed": completed,
            "errors": errors,
            "success_count": success_count,
            "error_count": error_count,
        }))
        .with_duration(start.elapsed())
        .with_summary(format!(
            "Moved {}/{} files",
            success_count,
            operations.len()
        )))
    }
}

#[async_trait]
impl TaskExecutor for FileOpsExecutor {
    fn supported_types(&self) -> Vec<&'static str> {
        vec!["file_operation"]
    }

    fn can_execute(&self, task_type: &TaskType) -> bool {
        matches!(task_type, TaskType::FileOperation(_))
    }

    async fn execute(&self, task: &Task, ctx: &ExecutionContext) -> Result<TaskResult> {
        let file_op = match &task.task_type {
            TaskType::FileOperation(op) => op,
            _ => {
                return Err(AlephError::other(format!(
                    "FileOpsExecutor cannot handle task type: {:?}",
                    task.task_type
                )));
            }
        };

        // Get content from parameters if needed for write operations
        let content = task
            .parameters
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        match file_op {
            FileOp::Read { path } => self.execute_read(path, ctx).await,
            FileOp::Write { path } => self.execute_write(path, content, ctx).await,
            FileOp::Move { from, to } => self.execute_move(from, to, ctx).await,
            FileOp::Copy { from, to } => self.execute_copy(from, to, ctx).await,
            FileOp::Delete { path } => self.execute_delete(path, ctx).await,
            FileOp::Search { pattern, dir } => self.execute_search(pattern, dir, ctx).await,
            FileOp::List { path } => self.execute_list(path, ctx).await,
            FileOp::BatchMove { operations } => self.execute_batch_move(operations, ctx).await,
        }
    }

    fn name(&self) -> &str {
        "FileOpsExecutor"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_executor() -> (FileOpsExecutor, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let executor = FileOpsExecutor::new(
            vec![temp_dir.path().to_string_lossy().to_string() + "/**"],
            vec![],
            10 * 1024 * 1024, // 10MB
            false,
            false,
        );
        (executor, temp_dir)
    }

    #[tokio::test]
    async fn test_read_file() {
        let (executor, temp_dir) = create_test_executor();
        let file_path = temp_dir.path().join("test.txt");
        fs::write(&file_path, "Hello, World!").unwrap();

        let task = Task::new(
            "test_read",
            "Read test file",
            TaskType::FileOperation(FileOp::Read {
                path: file_path.clone(),
            }),
        );

        let ctx = ExecutionContext::new("test_graph");
        let result = executor.execute(&task, &ctx).await.unwrap();

        let read_result: ReadResult = serde_json::from_value(result.output).unwrap();
        assert_eq!(read_result.content, "Hello, World!");
    }

    #[tokio::test]
    async fn test_write_file() {
        let (executor, temp_dir) = create_test_executor();
        let file_path = temp_dir.path().join("output.txt");

        let task = Task::new(
            "test_write",
            "Write test file",
            TaskType::FileOperation(FileOp::Write {
                path: file_path.clone(),
            }),
        )
        .with_parameters(json!({"content": "Test content"}));

        let ctx = ExecutionContext::new("test_graph");
        let result = executor.execute(&task, &ctx).await.unwrap();

        let write_result: WriteResult = serde_json::from_value(result.output).unwrap();
        assert!(write_result.created);
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "Test content");
    }

    #[tokio::test]
    async fn test_list_directory() {
        let (executor, temp_dir) = create_test_executor();

        // Create some test files
        fs::write(temp_dir.path().join("file1.txt"), "").unwrap();
        fs::write(temp_dir.path().join("file2.txt"), "").unwrap();
        fs::create_dir(temp_dir.path().join("subdir")).unwrap();

        let task = Task::new(
            "test_list",
            "List directory",
            TaskType::FileOperation(FileOp::List {
                path: temp_dir.path().to_path_buf(),
            }),
        );

        let ctx = ExecutionContext::new("test_graph");
        let result = executor.execute(&task, &ctx).await.unwrap();

        let list_result: ListResult = serde_json::from_value(result.output).unwrap();
        assert_eq!(list_result.total_entries, 3);
    }

}
