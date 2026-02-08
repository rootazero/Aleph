//! Atomic Executor - Executes atomic operations with security checks
//!
//! This module implements the executor that runs atomic operations (Read, Write, Edit, Bash)
//! with proper security validation and error handling.

use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;
use tracing::{debug, info, warn};
use regex::Regex;

use super::{AtomicAction, Patch, PatchApplier, WriteMode, SearchPattern, SearchScope, FileFilter};
use crate::error::{AlephError, Result};

/// Atomic operation executor
pub struct AtomicExecutor {
    /// Working directory for relative paths
    working_dir: PathBuf,

    /// Maximum file size for read/edit operations (10MB)
    max_file_size: u64,

    /// Command timeout (30 seconds)
    command_timeout: Duration,
}

impl AtomicExecutor {
    /// Create a new atomic executor
    pub fn new(working_dir: PathBuf) -> Self {
        Self {
            working_dir,
            max_file_size: 10 * 1024 * 1024, // 10MB
            command_timeout: Duration::from_secs(30),
        }
    }

    /// Execute an atomic action
    pub async fn execute(&self, action: &AtomicAction) -> Result<AtomicResult> {
        debug!(action = ?action, "Executing atomic action");

        let result = match action {
            AtomicAction::Read { path, range } => self.execute_read(path, range.as_ref()).await?,
            AtomicAction::Write { path, content, mode } => {
                self.execute_write(path, content, mode).await?
            }
            AtomicAction::Edit { path, patches } => self.execute_edit(path, patches).await?,
            AtomicAction::Bash { command, cwd } => {
                self.execute_bash(command, cwd.as_ref()).await?
            }
            AtomicAction::Search { pattern, scope, filters } => {
                self.execute_search(pattern, scope, filters).await?
            }
            AtomicAction::Replace { .. } => {
                // TODO: Implement replace operation
                AtomicResult {
                    success: false,
                    output: String::new(),
                    error: Some("Replace operation not yet implemented".to_string()),
                }
            }
            AtomicAction::Move { .. } => {
                // TODO: Implement move operation
                AtomicResult {
                    success: false,
                    output: String::new(),
                    error: Some("Move operation not yet implemented".to_string()),
                }
            }
        };

        Ok(result)
    }

    /// Execute Read operation
    async fn execute_read(
        &self,
        path: &str,
        range: Option<&super::LineRange>,
    ) -> Result<AtomicResult> {
        let resolved_path = self.resolve_path(path)?;

        // Check file exists
        if !resolved_path.exists() {
            return Ok(AtomicResult {
                success: false,
                output: String::new(),
                error: Some(format!("File not found: {}", resolved_path.display())),
            });
        }

        // Check file size
        let metadata = tokio::fs::metadata(&resolved_path).await?;
        if metadata.len() > self.max_file_size {
            return Ok(AtomicResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "File too large: {} bytes (max: {})",
                    metadata.len(),
                    self.max_file_size
                )),
            });
        }

        // Read file
        let content = tokio::fs::read_to_string(&resolved_path).await?;

        // Apply line range if specified
        let output = if let Some(range) = range {
            let lines: Vec<&str> = content.lines().collect();
            if range.start == 0 || range.start > lines.len() || range.end > lines.len() {
                return Ok(AtomicResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "Invalid line range: {}-{} (file has {} lines)",
                        range.start,
                        range.end,
                        lines.len()
                    )),
                });
            }
            lines[(range.start - 1)..range.end].join("\n")
        } else {
            content
        };

        Ok(AtomicResult {
            success: true,
            output,
            error: None,
        })
    }

    /// Execute Write operation
    async fn execute_write(
        &self,
        path: &str,
        content: &str,
        mode: &WriteMode,
    ) -> Result<AtomicResult> {
        let resolved_path = self.resolve_path(path)?;

        // Check mode
        match mode {
            WriteMode::CreateOnly => {
                if resolved_path.exists() {
                    return Ok(AtomicResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!("File already exists: {}", resolved_path.display())),
                    });
                }
            }
            WriteMode::Append => {
                if resolved_path.exists() {
                    let existing = tokio::fs::read_to_string(&resolved_path).await?;
                    let new_content = format!("{}{}", existing, content);
                    tokio::fs::write(&resolved_path, new_content).await?;
                    return Ok(AtomicResult {
                        success: true,
                        output: format!("Appended to {}", resolved_path.display()),
                        error: None,
                    });
                }
            }
            WriteMode::Overwrite => {
                // Default behavior
            }
        }

        // Write file
        tokio::fs::write(&resolved_path, content).await?;

        Ok(AtomicResult {
            success: true,
            output: format!("Wrote {} bytes to {}", content.len(), resolved_path.display()),
            error: None,
        })
    }

    /// Execute Edit operation
    async fn execute_edit(&self, path: &str, patches: &[Patch]) -> Result<AtomicResult> {
        let resolved_path = self.resolve_path(path)?;

        // Check file exists
        if !resolved_path.exists() {
            return Ok(AtomicResult {
                success: false,
                output: String::new(),
                error: Some(format!("File not found: {}", resolved_path.display())),
            });
        }

        // Read file
        let content = tokio::fs::read_to_string(&resolved_path).await?;

        // Apply patches
        let applier = PatchApplier::new(patches.to_vec());

        // Detect conflicts
        let conflicts = applier.detect_conflicts();
        if !conflicts.is_empty() {
            return Ok(AtomicResult {
                success: false,
                output: String::new(),
                error: Some(format!("Patch conflicts detected: {:?}", conflicts)),
            });
        }

        // Apply all patches
        let new_content = applier.apply_all(&content).map_err(|e| {
            AlephError::tool(format!("Failed to apply patches: {}", e))
        })?;

        // Write back
        tokio::fs::write(&resolved_path, new_content).await?;

        Ok(AtomicResult {
            success: true,
            output: format!(
                "Applied {} patches to {}",
                patches.len(),
                resolved_path.display()
            ),
            error: None,
        })
    }

    /// Execute Bash operation
    async fn execute_bash(&self, command: &str, cwd: Option<&String>) -> Result<AtomicResult> {
        let work_dir = if let Some(cwd) = cwd {
            PathBuf::from(cwd)
        } else {
            self.working_dir.clone()
        };

        // Execute command with timeout
        let output = tokio::time::timeout(
            self.command_timeout,
            Command::new("sh")
                .arg("-c")
                .arg(command)
                .current_dir(&work_dir)
                .output(),
        )
        .await
        .map_err(|_| AlephError::tool(format!("Command timeout after {:?}", self.command_timeout)))?
        .map_err(|e| AlephError::tool(format!("Failed to execute command: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if output.status.success() {
            Ok(AtomicResult {
                success: true,
                output: stdout,
                error: None,
            })
        } else {
            Ok(AtomicResult {
                success: false,
                output: stdout,
                error: Some(stderr),
            })
        }
    }

    /// Execute Search operation
    async fn execute_search(
        &self,
        pattern: &SearchPattern,
        scope: &SearchScope,
        filters: &[FileFilter],
    ) -> Result<AtomicResult> {
        debug!(pattern = ?pattern, scope = ?scope, "Executing search");

        // Collect files to search
        let files = self.collect_files_for_search(scope, filters).await?;

        if files.is_empty() {
            return Ok(AtomicResult {
                success: true,
                output: "No files found matching the search scope".to_string(),
                error: None,
            });
        }

        // Perform search based on pattern type
        let matches = match pattern {
            SearchPattern::Regex { pattern: regex_str } => {
                self.search_regex(&files, regex_str).await?
            }
            SearchPattern::Fuzzy { text, threshold } => {
                self.search_fuzzy(&files, text, *threshold).await?
            }
            SearchPattern::Ast { query, language } => {
                // AST search is complex, return not implemented for now
                return Ok(AtomicResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "AST search not yet implemented for language: {}",
                        language
                    )),
                });
            }
        };

        // Format results
        let output = if matches.is_empty() {
            "No matches found".to_string()
        } else {
            format!(
                "Found {} matches in {} files:\n{}",
                matches.len(),
                matches.iter().map(|m| &m.file).collect::<std::collections::HashSet<_>>().len(),
                matches
                    .iter()
                    .map(|m| format!("{}:{}:{}", m.file.display(), m.line_number, m.line_content))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };

        Ok(AtomicResult {
            success: true,
            output,
            error: None,
        })
    }

    /// Collect files for search based on scope and filters
    async fn collect_files_for_search(
        &self,
        scope: &SearchScope,
        filters: &[FileFilter],
    ) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();

        match scope {
            SearchScope::File { path } => {
                let resolved = self.resolve_path(path.to_str().unwrap_or(""))?;
                if resolved.exists() && self.should_include_file(&resolved, filters) {
                    files.push(resolved);
                }
            }
            SearchScope::Directory { path, recursive } => {
                let resolved = self.resolve_path(path.to_str().unwrap_or(""))?;
                if resolved.exists() && resolved.is_dir() {
                    self.collect_files_from_directory(&resolved, *recursive, filters, &mut files)
                        .await?;
                }
            }
            SearchScope::Workspace => {
                // Search in working directory recursively
                self.collect_files_from_directory(&self.working_dir, true, filters, &mut files)
                    .await?;
            }
        }

        Ok(files)
    }

    /// Recursively collect files from directory
    fn collect_files_from_directory<'a>(
        &'a self,
        dir: &'a Path,
        recursive: bool,
        filters: &'a [FileFilter],
        files: &'a mut Vec<PathBuf>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut entries = tokio::fs::read_dir(dir).await?;

            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();

                if path.is_file() {
                    if self.should_include_file(&path, filters) {
                        files.push(path);
                    }
                } else if path.is_dir() && recursive {
                    // Skip hidden directories and common ignore patterns
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if !name.starts_with('.') && name != "node_modules" && name != "target" {
                            self.collect_files_from_directory(&path, recursive, filters, files)
                                .await?;
                        }
                    }
                }
            }

            Ok(())
        })
    }

    /// Check if file should be included based on filters
    fn should_include_file(&self, path: &Path, filters: &[FileFilter]) -> bool {
        // If no filters, include all files
        if filters.is_empty() {
            return true;
        }

        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        for filter in filters {
            match filter {
                FileFilter::Code => {
                    // Common code file extensions
                    let code_exts = [
                        "rs", "py", "js", "ts", "jsx", "tsx", "go", "java", "c", "cpp", "h",
                        "hpp", "cs", "rb", "php", "swift", "kt", "scala", "sh", "bash",
                    ];
                    if !code_exts.contains(&extension) {
                        return false;
                    }
                }
                FileFilter::Text => {
                    // Common text file extensions
                    let text_exts = ["txt", "md", "rst", "log", "json", "yaml", "yml", "toml"];
                    if !text_exts.contains(&extension) {
                        return false;
                    }
                }
                FileFilter::Extension(ext) => {
                    if extension != ext {
                        return false;
                    }
                }
                FileFilter::Exclude(pattern) => {
                    // Simple glob-like matching
                    if filename.contains(pattern.trim_matches('*')) {
                        return false;
                    }
                }
            }
        }

        true
    }

    /// Search files using regex pattern
    async fn search_regex(&self, files: &[PathBuf], pattern: &str) -> Result<Vec<SearchMatch>> {
        let regex = Regex::new(pattern)
            .map_err(|e| AlephError::tool(format!("Invalid regex pattern: {}", e)))?;

        let mut matches = Vec::new();

        for file in files {
            // Skip files that are too large
            if let Ok(metadata) = tokio::fs::metadata(file).await {
                if metadata.len() > self.max_file_size {
                    continue;
                }
            }

            // Read file content
            if let Ok(content) = tokio::fs::read_to_string(file).await {
                for (line_num, line) in content.lines().enumerate() {
                    if regex.is_match(line) {
                        matches.push(SearchMatch {
                            file: file.clone(),
                            line_number: line_num + 1,
                            line_content: line.to_string(),
                            match_start: 0, // TODO: Calculate actual match position
                            match_end: line.len(),
                        });
                    }
                }
            }
        }

        Ok(matches)
    }

    /// Search files using fuzzy matching
    async fn search_fuzzy(
        &self,
        files: &[PathBuf],
        text: &str,
        threshold: f32,
    ) -> Result<Vec<SearchMatch>> {
        let mut matches = Vec::new();

        for file in files {
            // Skip files that are too large
            if let Ok(metadata) = tokio::fs::metadata(file).await {
                if metadata.len() > self.max_file_size {
                    continue;
                }
            }

            // Read file content
            if let Ok(content) = tokio::fs::read_to_string(file).await {
                for (line_num, line) in content.lines().enumerate() {
                    // Simple fuzzy matching: check if text appears as substring (case-insensitive)
                    // TODO: Implement proper fuzzy matching algorithm (e.g., Levenshtein distance)
                    let similarity = if line.to_lowercase().contains(&text.to_lowercase()) {
                        1.0
                    } else {
                        0.0
                    };

                    if similarity >= threshold {
                        matches.push(SearchMatch {
                            file: file.clone(),
                            line_number: line_num + 1,
                            line_content: line.to_string(),
                            match_start: 0,
                            match_end: line.len(),
                        });
                    }
                }
            }
        }

        Ok(matches)
    }

    /// Resolve path (relative to working directory)
    fn resolve_path(&self, path: &str) -> Result<PathBuf> {
        let path = Path::new(path);

        // If absolute, use as-is
        if path.is_absolute() {
            return Ok(path.to_path_buf());
        }

        // If starts with ~, expand home directory
        if let Some(path_str) = path.to_str() {
            if path_str.starts_with("~/") || path_str == "~" {
                if let Some(home) = dirs::home_dir() {
                    let relative = path_str.strip_prefix("~/").unwrap_or("");
                    return Ok(home.join(relative));
                }
            }
        }

        // Otherwise, resolve relative to working directory
        Ok(self.working_dir.join(path))
    }
}

/// Search match result
#[derive(Debug, Clone)]
struct SearchMatch {
    /// File where match was found
    file: PathBuf,
    /// Line number (1-indexed)
    line_number: usize,
    /// Content of the matching line
    line_content: String,
    /// Start position of match in line
    match_start: usize,
    /// End position of match in line
    match_end: usize,
}

/// Result of atomic operation execution
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtomicResult {
    /// Whether the operation succeeded
    pub success: bool,

    /// Output (stdout for bash, content for read, message for write/edit)
    pub output: String,

    /// Error message (if failed)
    pub error: Option<String>,
}

impl AtomicResult {
    /// Check if the result indicates success
    pub fn is_success(&self) -> bool {
        self.success
    }

    /// Get error message
    pub fn error_message(&self) -> String {
        self.error.clone().unwrap_or_default()
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

        // Create test file
        let test_file = temp_dir.path().join("test.txt");
        fs::write(&test_file, "line1\nline2\nline3\n").unwrap();

        // Read entire file
        let action = AtomicAction::Read {
            path: test_file.to_str().unwrap().to_string(),
            range: None,
        };
        let result = executor.execute(&action).await.unwrap();
        assert!(result.success);
        assert_eq!(result.output, "line1\nline2\nline3\n");
    }

    #[tokio::test]
    async fn test_execute_read_with_range() {
        let (executor, temp_dir) = create_test_executor();

        // Create test file
        let test_file = temp_dir.path().join("test.txt");
        fs::write(&test_file, "line1\nline2\nline3\nline4\n").unwrap();

        // Read lines 2-3
        let action = AtomicAction::Read {
            path: test_file.to_str().unwrap().to_string(),
            range: Some(super::super::LineRange { start: 2, end: 3 }),
        };
        let result = executor.execute(&action).await.unwrap();
        assert!(result.success);
        assert_eq!(result.output, "line2\nline3");
    }

    #[tokio::test]
    async fn test_execute_write() {
        let (executor, temp_dir) = create_test_executor();

        let test_file = temp_dir.path().join("test.txt");

        // Write file
        let action = AtomicAction::Write {
            path: test_file.to_str().unwrap().to_string(),
            content: "hello world".to_string(),
            mode: WriteMode::Overwrite,
        };
        let result = executor.execute(&action).await.unwrap();
        assert!(result.success);

        // Verify content
        let content = fs::read_to_string(&test_file).unwrap();
        assert_eq!(content, "hello world");
    }

    #[tokio::test]
    async fn test_execute_edit() {
        let (executor, temp_dir) = create_test_executor();

        // Create test file
        let test_file = temp_dir.path().join("test.txt");
        fs::write(&test_file, "line1\nline2\nline3\n").unwrap();

        // Edit file
        let action = AtomicAction::Edit {
            path: test_file.to_str().unwrap().to_string(),
            patches: vec![Patch {
                start_line: 2,
                end_line: 2,
                old_content: "line2".to_string(),
                new_content: "modified".to_string(),
            }],
        };
        let result = executor.execute(&action).await.unwrap();
        assert!(result.success);

        // Verify content
        let content = fs::read_to_string(&test_file).unwrap();
        assert_eq!(content, "line1\nmodified\nline3\n");
    }

    #[tokio::test]
    async fn test_execute_bash() {
        let (executor, _temp_dir) = create_test_executor();

        // Execute simple command
        let action = AtomicAction::Bash {
            command: "echo hello".to_string(),
            cwd: None,
        };
        let result = executor.execute(&action).await.unwrap();
        assert!(result.success);
        assert_eq!(result.output.trim(), "hello");
    }

    #[tokio::test]
    async fn test_execute_bash_failure() {
        let (executor, _temp_dir) = create_test_executor();

        // Execute failing command
        let action = AtomicAction::Bash {
            command: "exit 1".to_string(),
            cwd: None,
        };
        let result = executor.execute(&action).await.unwrap();
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_read_file_not_found() {
        let (executor, _temp_dir) = create_test_executor();

        let action = AtomicAction::Read {
            path: "/nonexistent/file.txt".to_string(),
            range: None,
        };
        let result = executor.execute(&action).await.unwrap();
        assert!(!result.success);
        assert!(result.error.is_some());
    }

    #[tokio::test]
    async fn test_write_create_only() {
        let (executor, temp_dir) = create_test_executor();

        let test_file = temp_dir.path().join("test.txt");

        // First write should succeed
        let action = AtomicAction::Write {
            path: test_file.to_str().unwrap().to_string(),
            content: "content".to_string(),
            mode: WriteMode::CreateOnly,
        };
        let result = executor.execute(&action).await.unwrap();
        assert!(result.success);

        // Second write should fail
        let result = executor.execute(&action).await.unwrap();
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_write_append() {
        let (executor, temp_dir) = create_test_executor();

        let test_file = temp_dir.path().join("test.txt");

        // Write initial content
        fs::write(&test_file, "initial\n").unwrap();

        // Append
        let action = AtomicAction::Write {
            path: test_file.to_str().unwrap().to_string(),
            content: "appended\n".to_string(),
            mode: WriteMode::Append,
        };
        let result = executor.execute(&action).await.unwrap();
        assert!(result.success);

        // Verify content
        let content = fs::read_to_string(&test_file).unwrap();
        assert_eq!(content, "initial\nappended\n");
    }

    #[tokio::test]
    async fn test_execute_search_regex() {
        let (executor, temp_dir) = create_test_executor();

        // Create test files
        let test_file1 = temp_dir.path().join("test1.rs");
        fs::write(&test_file1, "fn main() {\n    println!(\"TODO: implement\");\n}\n").unwrap();

        let test_file2 = temp_dir.path().join("test2.rs");
        fs::write(&test_file2, "// TODO: fix this\nfn helper() {}\n").unwrap();

        // Search for TODO comments
        let action = AtomicAction::Search {
            pattern: SearchPattern::Regex {
                pattern: r"TODO:.*".to_string(),
            },
            scope: SearchScope::Directory {
                path: temp_dir.path().to_path_buf(),
                recursive: false,
            },
            filters: vec![FileFilter::Extension("rs".to_string())],
        };

        let result = executor.execute(&action).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("Found 2 matches"));
        assert!(result.output.contains("test1.rs"));
        assert!(result.output.contains("test2.rs"));
    }

    #[tokio::test]
    async fn test_execute_search_fuzzy() {
        let (executor, temp_dir) = create_test_executor();

        // Create test file
        let test_file = temp_dir.path().join("test.txt");
        fs::write(&test_file, "Hello world\nHello Rust\nGoodbye\n").unwrap();

        // Fuzzy search for "hello"
        let action = AtomicAction::Search {
            pattern: SearchPattern::Fuzzy {
                text: "hello".to_string(),
                threshold: 0.8,
            },
            scope: SearchScope::File {
                path: test_file.clone(),
            },
            filters: vec![],
        };

        let result = executor.execute(&action).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("Found 2 matches"));
    }

    #[tokio::test]
    async fn test_execute_search_no_matches() {
        let (executor, temp_dir) = create_test_executor();

        // Create test file
        let test_file = temp_dir.path().join("test.txt");
        fs::write(&test_file, "Hello world\n").unwrap();

        // Search for non-existent pattern
        let action = AtomicAction::Search {
            pattern: SearchPattern::Regex {
                pattern: r"NOTFOUND".to_string(),
            },
            scope: SearchScope::File {
                path: test_file,
            },
            filters: vec![],
        };

        let result = executor.execute(&action).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("No matches found"));
    }

    #[tokio::test]
    async fn test_execute_search_with_filters() {
        let (executor, temp_dir) = create_test_executor();

        // Create test files with different extensions
        let rs_file = temp_dir.path().join("test.rs");
        fs::write(&rs_file, "fn main() {}\n").unwrap();

        let txt_file = temp_dir.path().join("test.txt");
        fs::write(&txt_file, "fn main() {}\n").unwrap();

        // Search only .rs files
        let action = AtomicAction::Search {
            pattern: SearchPattern::Regex {
                pattern: r"fn main".to_string(),
            },
            scope: SearchScope::Directory {
                path: temp_dir.path().to_path_buf(),
                recursive: false,
            },
            filters: vec![FileFilter::Extension("rs".to_string())],
        };

        let result = executor.execute(&action).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("test.rs"));
        assert!(!result.output.contains("test.txt"));
    }
}
