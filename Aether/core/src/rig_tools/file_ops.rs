//! File operations tool for AI agent integration
//!
//! Implements rig's Tool trait to provide file system operations.
//! Supports: list, read, write, move, copy, delete, mkdir, search

use std::fs;
use std::path::{Path, PathBuf};

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{debug, info};

use super::error::ToolError;

/// File operation type
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileOperation {
    /// List directory contents
    List,
    /// Read file content
    Read,
    /// Write content to file
    Write,
    /// Move/rename file or directory
    Move,
    /// Copy file or directory
    Copy,
    /// Delete file or directory
    Delete,
    /// Create directory
    Mkdir,
    /// Search files by glob pattern
    Search,
    /// Batch move files matching a pattern to destination
    BatchMove,
    /// Auto-organize files by type into categorized folders
    Organize,
}

/// Arguments for file operations tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FileOpsArgs {
    /// The operation to perform
    pub operation: FileOperation,
    /// Primary path (source path for move/copy, target path for others)
    pub path: String,
    /// Destination path (for move/copy operations)
    #[serde(default)]
    pub destination: Option<String>,
    /// Content to write (for write operation)
    #[serde(default)]
    pub content: Option<String>,
    /// Search pattern (for search operation, glob syntax)
    #[serde(default)]
    pub pattern: Option<String>,
    /// Create parent directories if they don't exist
    #[serde(default = "default_true")]
    pub create_parents: bool,
}

fn default_true() -> bool {
    true
}

/// File metadata in output
#[derive(Debug, Clone, Serialize)]
pub struct FileInfo {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extension: Option<String>,
}

/// Output from file operations tool
#[derive(Debug, Clone, Serialize)]
pub struct FileOpsOutput {
    pub success: bool,
    pub operation: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<FileInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_written: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items_affected: Option<usize>,
}

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

IMPORTANT: For organizing multiple files, use 'organize' or 'batch_move' instead of multiple 'move' calls!"#;

    /// Create a new FileOpsTool with default settings
    pub fn new() -> Self {
        let mut denied_paths = vec![
            // Unix sensitive directories
            "~/.ssh".to_string(),
            "~/.gnupg".to_string(),
            "~/.aws".to_string(),
        ];

        // Add Aether config directory dynamically (cross-platform)
        if let Ok(config_dir) = crate::utils::paths::get_config_dir() {
            denied_paths.push(config_dir.to_string_lossy().to_string());
        }

        // Add Unix-specific paths
        #[cfg(unix)]
        {
            denied_paths.extend([
                "/etc/passwd".to_string(),
                "/etc/shadow".to_string(),
            ]);
        }

        // Add Windows-specific sensitive paths
        #[cfg(target_os = "windows")]
        {
            denied_paths.extend([
                "%APPDATA%\\Microsoft\\Credentials".to_string(),
                "%LOCALAPPDATA%\\Microsoft\\Credentials".to_string(),
                "C:\\Windows\\System32\\config".to_string(),
            ]);
        }

        Self {
            max_read_size: 100 * 1024 * 1024, // 100MB
            denied_paths,
        }
    }

    /// Check if path is allowed
    fn check_path(&self, path: &Path) -> Result<PathBuf, ToolError> {
        // Expand ~ to home directory
        let expanded = if path.starts_with("~") {
            let home = dirs::home_dir().ok_or_else(|| {
                ToolError::InvalidArgs("Cannot determine home directory".to_string())
            })?;
            home.join(path.strip_prefix("~").unwrap())
        } else {
            path.to_path_buf()
        };

        // Canonicalize if exists, otherwise use as-is for new files
        let canonical = if expanded.exists() {
            expanded
                .canonicalize()
                .map_err(|e| ToolError::Execution(format!("Failed to resolve path: {}", e)))?
        } else {
            expanded
        };

        // Check against denied paths
        let path_str = canonical.to_string_lossy();
        for denied in &self.denied_paths {
            let denied_expanded = if denied.starts_with("~") {
                if let Some(home) = dirs::home_dir() {
                    home.join(denied.strip_prefix("~/").unwrap_or(denied))
                        .to_string_lossy()
                        .to_string()
                } else {
                    denied.clone()
                }
            } else {
                denied.clone()
            };

            if path_str.starts_with(&denied_expanded) {
                return Err(ToolError::InvalidArgs(format!(
                    "Access denied: {} is in a protected location",
                    path.display()
                )));
            }
        }

        Ok(canonical)
    }

    /// Execute a list operation
    async fn execute_list(&self, path: &Path) -> Result<FileOpsOutput, ToolError> {
        let canonical = self.check_path(path)?;

        if !canonical.exists() {
            return Err(ToolError::Execution(format!(
                "Directory not found: {}",
                path.display()
            )));
        }

        if !canonical.is_dir() {
            return Err(ToolError::InvalidArgs(format!(
                "Not a directory: {}",
                path.display()
            )));
        }

        let mut files = Vec::new();
        for entry in fs::read_dir(&canonical)
            .map_err(|e| ToolError::Execution(format!("Failed to read directory: {}", e)))?
        {
            let entry =
                entry.map_err(|e| ToolError::Execution(format!("Failed to read entry: {}", e)))?;

            let metadata = entry
                .metadata()
                .map_err(|e| ToolError::Execution(format!("Failed to get metadata: {}", e)))?;

            let entry_path = entry.path();
            files.push(FileInfo {
                name: entry.file_name().to_string_lossy().to_string(),
                path: entry_path.to_string_lossy().to_string(),
                is_dir: metadata.is_dir(),
                size: metadata.len(),
                extension: entry_path
                    .extension()
                    .map(|e| e.to_string_lossy().to_string()),
            });
        }

        // Sort: directories first, then by name
        files.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.cmp(&b.name),
        });

        let count = files.len();
        info!(path = %canonical.display(), count, "Listed directory");

        Ok(FileOpsOutput {
            success: true,
            operation: "list".to_string(),
            message: format!("Listed {} items in {}", count, canonical.display()),
            files: Some(files),
            content: None,
            bytes_written: None,
            items_affected: Some(count),
        })
    }

    /// Execute a read operation
    async fn execute_read(&self, path: &Path) -> Result<FileOpsOutput, ToolError> {
        let canonical = self.check_path(path)?;

        if !canonical.exists() {
            return Err(ToolError::Execution(format!(
                "File not found: {}",
                path.display()
            )));
        }

        if !canonical.is_file() {
            return Err(ToolError::InvalidArgs(format!(
                "Not a file: {}",
                path.display()
            )));
        }

        let metadata = fs::metadata(&canonical)
            .map_err(|e| ToolError::Execution(format!("Failed to get metadata: {}", e)))?;

        if metadata.len() > self.max_read_size {
            return Err(ToolError::InvalidArgs(format!(
                "File too large: {} bytes (max {})",
                metadata.len(),
                self.max_read_size
            )));
        }

        let content = fs::read_to_string(&canonical)
            .map_err(|e| ToolError::Execution(format!("Failed to read file: {}", e)))?;

        info!(path = %canonical.display(), size = metadata.len(), "Read file");

        Ok(FileOpsOutput {
            success: true,
            operation: "read".to_string(),
            message: format!("Read {} bytes from {}", metadata.len(), canonical.display()),
            files: None,
            content: Some(content),
            bytes_written: None,
            items_affected: None,
        })
    }

    /// Execute a write operation
    async fn execute_write(
        &self,
        path: &Path,
        content: &str,
        create_parents: bool,
    ) -> Result<FileOpsOutput, ToolError> {
        let canonical = self.check_path(path)?;

        // Create parent directories if needed
        if create_parents {
            if let Some(parent) = canonical.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent).map_err(|e| {
                        ToolError::Execution(format!("Failed to create directories: {}", e))
                    })?;
                    debug!(path = %parent.display(), "Created parent directories");
                }
            }
        }

        let bytes = content.len() as u64;
        fs::write(&canonical, content)
            .map_err(|e| ToolError::Execution(format!("Failed to write file: {}", e)))?;

        info!(path = %canonical.display(), bytes, "Wrote file");

        Ok(FileOpsOutput {
            success: true,
            operation: "write".to_string(),
            message: format!("Wrote {} bytes to {}", bytes, canonical.display()),
            files: None,
            content: None,
            bytes_written: Some(bytes),
            items_affected: None,
        })
    }

    /// Execute a move operation
    async fn execute_move(
        &self,
        from: &Path,
        to: &Path,
        create_parents: bool,
    ) -> Result<FileOpsOutput, ToolError> {
        let from_canonical = self.check_path(from)?;
        let to_canonical = self.check_path(to)?;

        if !from_canonical.exists() {
            return Err(ToolError::Execution(format!(
                "Source not found: {}",
                from.display()
            )));
        }

        // Create parent directories if needed
        if create_parents {
            if let Some(parent) = to_canonical.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent).map_err(|e| {
                        ToolError::Execution(format!("Failed to create directories: {}", e))
                    })?;
                }
            }
        }

        fs::rename(&from_canonical, &to_canonical)
            .map_err(|e| ToolError::Execution(format!("Failed to move: {}", e)))?;

        info!(from = %from_canonical.display(), to = %to_canonical.display(), "Moved");

        Ok(FileOpsOutput {
            success: true,
            operation: "move".to_string(),
            message: format!(
                "Moved {} to {}",
                from_canonical.display(),
                to_canonical.display()
            ),
            files: None,
            content: None,
            bytes_written: None,
            items_affected: Some(1),
        })
    }

    /// Execute a copy operation
    async fn execute_copy(
        &self,
        from: &Path,
        to: &Path,
        create_parents: bool,
    ) -> Result<FileOpsOutput, ToolError> {
        let from_canonical = self.check_path(from)?;
        let to_canonical = self.check_path(to)?;

        if !from_canonical.exists() {
            return Err(ToolError::Execution(format!(
                "Source not found: {}",
                from.display()
            )));
        }

        // Create parent directories if needed
        if create_parents {
            if let Some(parent) = to_canonical.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent).map_err(|e| {
                        ToolError::Execution(format!("Failed to create directories: {}", e))
                    })?;
                }
            }
        }

        let bytes = if from_canonical.is_file() {
            fs::copy(&from_canonical, &to_canonical)
                .map_err(|e| ToolError::Execution(format!("Failed to copy: {}", e)))?
        } else {
            // Directory copy - recursive
            self.copy_dir_recursive(&from_canonical, &to_canonical)?
        };

        info!(from = %from_canonical.display(), to = %to_canonical.display(), bytes, "Copied");

        Ok(FileOpsOutput {
            success: true,
            operation: "copy".to_string(),
            message: format!(
                "Copied {} to {} ({} bytes)",
                from_canonical.display(),
                to_canonical.display(),
                bytes
            ),
            files: None,
            content: None,
            bytes_written: Some(bytes),
            items_affected: Some(1),
        })
    }

    /// Recursively copy a directory
    fn copy_dir_recursive(&self, from: &Path, to: &Path) -> Result<u64, ToolError> {
        fs::create_dir_all(to)
            .map_err(|e| ToolError::Execution(format!("Failed to create directory: {}", e)))?;

        let mut total_bytes = 0u64;

        for entry in fs::read_dir(from)
            .map_err(|e| ToolError::Execution(format!("Failed to read directory: {}", e)))?
        {
            let entry =
                entry.map_err(|e| ToolError::Execution(format!("Failed to read entry: {}", e)))?;

            let from_path = entry.path();
            let to_path = to.join(entry.file_name());

            if from_path.is_dir() {
                total_bytes += self.copy_dir_recursive(&from_path, &to_path)?;
            } else {
                total_bytes += fs::copy(&from_path, &to_path)
                    .map_err(|e| ToolError::Execution(format!("Failed to copy file: {}", e)))?;
            }
        }

        Ok(total_bytes)
    }

    /// Execute a delete operation
    async fn execute_delete(&self, path: &Path) -> Result<FileOpsOutput, ToolError> {
        let canonical = self.check_path(path)?;

        if !canonical.exists() {
            return Err(ToolError::Execution(format!(
                "Path not found: {}",
                path.display()
            )));
        }

        let is_dir = canonical.is_dir();
        let items_deleted = if is_dir {
            let count = fs::read_dir(&canonical)
                .map(|entries| entries.count())
                .unwrap_or(0)
                + 1;
            fs::remove_dir_all(&canonical)
                .map_err(|e| ToolError::Execution(format!("Failed to delete directory: {}", e)))?;
            count
        } else {
            fs::remove_file(&canonical)
                .map_err(|e| ToolError::Execution(format!("Failed to delete file: {}", e)))?;
            1
        };

        info!(path = %canonical.display(), is_dir, items_deleted, "Deleted");

        Ok(FileOpsOutput {
            success: true,
            operation: "delete".to_string(),
            message: format!("Deleted {} ({} items)", canonical.display(), items_deleted),
            files: None,
            content: None,
            bytes_written: None,
            items_affected: Some(items_deleted),
        })
    }

    /// Execute a mkdir operation
    async fn execute_mkdir(
        &self,
        path: &Path,
        create_parents: bool,
    ) -> Result<FileOpsOutput, ToolError> {
        let canonical = self.check_path(path)?;

        if canonical.exists() {
            if canonical.is_dir() {
                return Ok(FileOpsOutput {
                    success: true,
                    operation: "mkdir".to_string(),
                    message: format!("Directory already exists: {}", canonical.display()),
                    files: None,
                    content: None,
                    bytes_written: None,
                    items_affected: Some(0),
                });
            } else {
                return Err(ToolError::InvalidArgs(format!(
                    "Path exists but is not a directory: {}",
                    path.display()
                )));
            }
        }

        if create_parents {
            fs::create_dir_all(&canonical).map_err(|e| {
                ToolError::Execution(format!("Failed to create directories: {}", e))
            })?;
        } else {
            fs::create_dir(&canonical)
                .map_err(|e| ToolError::Execution(format!("Failed to create directory: {}", e)))?;
        }

        info!(path = %canonical.display(), "Created directory");

        Ok(FileOpsOutput {
            success: true,
            operation: "mkdir".to_string(),
            message: format!("Created directory: {}", canonical.display()),
            files: None,
            content: None,
            bytes_written: None,
            items_affected: Some(1),
        })
    }

    /// Execute a search operation
    async fn execute_search(&self, dir: &Path, pattern: &str) -> Result<FileOpsOutput, ToolError> {
        let canonical = self.check_path(dir)?;

        if !canonical.exists() {
            return Err(ToolError::Execution(format!(
                "Directory not found: {}",
                dir.display()
            )));
        }

        if !canonical.is_dir() {
            return Err(ToolError::InvalidArgs(format!(
                "Not a directory: {}",
                dir.display()
            )));
        }

        let full_pattern = canonical.join(pattern);
        let pattern_str = full_pattern.to_string_lossy();

        let mut files = Vec::new();

        for entry in glob::glob(&pattern_str)
            .map_err(|e| ToolError::InvalidArgs(format!("Invalid glob pattern: {}", e)))?
        {
            match entry {
                Ok(path) => {
                    if let Ok(metadata) = fs::metadata(&path) {
                        files.push(FileInfo {
                            name: path
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_default(),
                            path: path.to_string_lossy().to_string(),
                            is_dir: metadata.is_dir(),
                            size: metadata.len(),
                            extension: path.extension().map(|e| e.to_string_lossy().to_string()),
                        });
                    }
                }
                Err(e) => {
                    debug!(error = %e, "Glob match error");
                }
            }
        }

        let count = files.len();
        info!(pattern, count, "Search completed");

        Ok(FileOpsOutput {
            success: true,
            operation: "search".to_string(),
            message: format!(
                "Found {} files matching '{}' in {}",
                count,
                pattern,
                canonical.display()
            ),
            files: Some(files),
            content: None,
            bytes_written: None,
            items_affected: Some(count),
        })
    }

    /// Execute a batch move operation
    ///
    /// Moves all files matching the pattern to the destination directory
    async fn execute_batch_move(
        &self,
        dir: &Path,
        pattern: &str,
        dest: &Path,
        create_parents: bool,
    ) -> Result<FileOpsOutput, ToolError> {
        let canonical = self.check_path(dir)?;
        let dest_canonical = if dest.exists() {
            self.check_path(dest)?
        } else if create_parents {
            // Create destination if needed
            fs::create_dir_all(dest).map_err(|e| {
                ToolError::Execution(format!("Failed to create destination: {}", e))
            })?;
            dest.to_path_buf()
        } else {
            return Err(ToolError::InvalidArgs(format!(
                "Destination does not exist: {}",
                dest.display()
            )));
        };

        if !canonical.is_dir() {
            return Err(ToolError::InvalidArgs(format!(
                "Source path is not a directory: {}",
                dir.display()
            )));
        }

        let full_pattern = canonical.join(pattern);
        let pattern_str = full_pattern.to_string_lossy();

        let mut moved_files = Vec::new();
        let mut errors = Vec::new();

        for entry in glob::glob(&pattern_str)
            .map_err(|e| ToolError::InvalidArgs(format!("Invalid glob pattern: {}", e)))?
        {
            match entry {
                Ok(path) => {
                    if path.is_file() {
                        let file_name = path.file_name().unwrap_or_default();
                        let dest_path = dest_canonical.join(file_name);

                        match fs::rename(&path, &dest_path) {
                            Ok(_) => {
                                moved_files.push(FileInfo {
                                    name: file_name.to_string_lossy().to_string(),
                                    path: dest_path.to_string_lossy().to_string(),
                                    is_dir: false,
                                    size: 0,
                                    extension: path
                                        .extension()
                                        .map(|e| e.to_string_lossy().to_string()),
                                });
                            }
                            Err(e) => {
                                errors.push(format!("{}: {}", path.display(), e));
                            }
                        }
                    }
                }
                Err(e) => {
                    debug!(error = %e, "Glob match error");
                }
            }
        }

        let count = moved_files.len();
        let message = if errors.is_empty() {
            format!(
                "Moved {} files matching '{}' to {}",
                count,
                pattern,
                dest_canonical.display()
            )
        } else {
            format!(
                "Moved {} files, {} errors: {}",
                count,
                errors.len(),
                errors.join("; ")
            )
        };

        info!(
            pattern,
            count,
            errors = errors.len(),
            "Batch move completed"
        );

        Ok(FileOpsOutput {
            success: errors.is_empty(),
            operation: "batch_move".to_string(),
            message,
            files: Some(moved_files),
            content: None,
            bytes_written: None,
            items_affected: Some(count),
        })
    }

    /// Execute an organize operation
    ///
    /// Automatically organizes files by type into categorized folders
    async fn execute_organize(
        &self,
        dir: &Path,
        create_parents: bool,
    ) -> Result<FileOpsOutput, ToolError> {
        let canonical = self.check_path(dir)?;

        if !canonical.is_dir() {
            return Err(ToolError::InvalidArgs(format!(
                "Not a directory: {}",
                dir.display()
            )));
        }

        // Define file type categories
        let categories: Vec<(&str, Vec<&str>)> = vec![
            (
                "Images",
                vec![
                    "jpg", "jpeg", "png", "gif", "webp", "svg", "bmp", "ico", "tiff", "heic",
                    "heif",
                ],
            ),
            (
                "Documents",
                vec![
                    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "rtf", "odt", "ods",
                    "odp", "pages", "numbers", "key", "md", "csv",
                ],
            ),
            (
                "Videos",
                vec![
                    "mp4", "avi", "mkv", "mov", "wmv", "flv", "webm", "m4v", "mpeg", "mpg", "3gp",
                ],
            ),
            (
                "Audio",
                vec![
                    "mp3", "wav", "flac", "aac", "ogg", "wma", "m4a", "aiff", "opus",
                ],
            ),
            (
                "Archives",
                vec!["zip", "rar", "7z", "tar", "gz", "bz2", "xz", "dmg", "iso"],
            ),
            (
                "Code",
                vec![
                    "rs", "py", "js", "ts", "jsx", "tsx", "java", "c", "cpp", "h", "hpp", "go",
                    "rb", "php", "swift", "kt", "scala", "html", "css", "scss", "json", "xml",
                    "yaml", "yml", "toml", "sh", "bash", "sql",
                ],
            ),
            (
                "Apps",
                vec!["app", "exe", "msi", "apk", "ipa", "deb", "rpm", "pkg"],
            ),
        ];

        let mut moved_files = Vec::new();
        let mut errors = Vec::new();
        let mut category_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();

        // Read directory entries
        let entries: Vec<_> = fs::read_dir(&canonical)
            .map_err(|e| ToolError::Execution(format!("Failed to read directory: {}", e)))?
            .filter_map(|e| e.ok())
            .collect();

        for entry in entries {
            let path = entry.path();

            // Skip directories
            if path.is_dir() {
                continue;
            }

            // Get file extension
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_lowercase())
                .unwrap_or_default();

            // Find matching category
            let category = categories
                .iter()
                .find(|(_, exts)| exts.contains(&ext.as_str()))
                .map(|(name, _)| *name)
                .unwrap_or("Others");

            // Create category directory if needed
            let category_dir = canonical.join(category);
            if !category_dir.exists() && create_parents {
                if let Err(e) = fs::create_dir(&category_dir) {
                    errors.push(format!("Failed to create {}: {}", category, e));
                    continue;
                }
            }

            // Move file to category directory
            let file_name = path.file_name().unwrap_or_default();
            let dest_path = category_dir.join(file_name);

            // Skip if already in category folder
            if path.parent() == Some(&category_dir) {
                continue;
            }

            match fs::rename(&path, &dest_path) {
                Ok(_) => {
                    *category_counts.entry(category.to_string()).or_insert(0) += 1;
                    moved_files.push(FileInfo {
                        name: file_name.to_string_lossy().to_string(),
                        path: dest_path.to_string_lossy().to_string(),
                        is_dir: false,
                        size: 0,
                        extension: Some(ext),
                    });
                }
                Err(e) => {
                    errors.push(format!("{}: {}", path.display(), e));
                }
            }
        }

        let count = moved_files.len();
        let summary: Vec<String> = category_counts
            .iter()
            .map(|(cat, cnt)| format!("{}: {}", cat, cnt))
            .collect();

        let message = if errors.is_empty() {
            format!(
                "Organized {} files into categories: {}",
                count,
                summary.join(", ")
            )
        } else {
            format!(
                "Organized {} files ({}), {} errors",
                count,
                summary.join(", "),
                errors.len()
            )
        };

        info!(count, categories = ?category_counts, errors = errors.len(), "Organize completed");

        Ok(FileOpsOutput {
            success: errors.is_empty(),
            operation: "organize".to_string(),
            message,
            files: Some(moved_files),
            content: None,
            bytes_written: None,
            items_affected: Some(count),
        })
    }

    /// Execute file operation based on args
    pub async fn call(&self, args: FileOpsArgs) -> Result<FileOpsOutput, ToolError> {
        info!(
            operation = ?args.operation,
            path = %args.path,
            destination = ?args.destination,
            "FileOpsTool::call invoked"
        );

        let path = Path::new(&args.path);

        match args.operation {
            FileOperation::List => self.execute_list(path).await,
            FileOperation::Read => self.execute_read(path).await,
            FileOperation::Write => {
                let content = args.content.ok_or_else(|| {
                    ToolError::InvalidArgs("Content required for write operation".to_string())
                })?;
                self.execute_write(path, &content, args.create_parents)
                    .await
            }
            FileOperation::Move => {
                let dest = args.destination.ok_or_else(|| {
                    ToolError::InvalidArgs("Destination required for move operation".to_string())
                })?;
                self.execute_move(path, Path::new(&dest), args.create_parents)
                    .await
            }
            FileOperation::Copy => {
                let dest = args.destination.ok_or_else(|| {
                    ToolError::InvalidArgs("Destination required for copy operation".to_string())
                })?;
                self.execute_copy(path, Path::new(&dest), args.create_parents)
                    .await
            }
            FileOperation::Delete => self.execute_delete(path).await,
            FileOperation::Mkdir => self.execute_mkdir(path, args.create_parents).await,
            FileOperation::Search => {
                let pattern = args.pattern.ok_or_else(|| {
                    ToolError::InvalidArgs("Pattern required for search operation".to_string())
                })?;
                self.execute_search(path, &pattern).await
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
                self.execute_batch_move(path, &pattern, Path::new(&dest), args.create_parents)
                    .await
            }
            FileOperation::Organize => self.execute_organize(path, args.create_parents).await,
        }
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
impl Tool for FileOpsTool {
    const NAME: &'static str = "file_ops";

    type Error = ToolError;
    type Args = FileOpsArgs;
    type Output = FileOpsOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: Self::DESCRIPTION.to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "operation": {
                        "type": "string",
                        "enum": ["list", "read", "write", "move", "copy", "delete", "mkdir", "search", "batch_move", "organize"],
                        "description": "The file operation to perform. Use 'organize' to auto-sort files by type, or 'batch_move' to move files matching a pattern."
                    },
                    "path": {
                        "type": "string",
                        "description": "Primary path (source directory for batch_move/organize, target for others)"
                    },
                    "destination": {
                        "type": "string",
                        "description": "Destination path (required for move/copy/batch_move operations)"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write (required for write operation)"
                    },
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern for search/batch_move (e.g., '*.pdf', '*.jpg', '**/*.png')"
                    },
                    "create_parents": {
                        "type": "boolean",
                        "description": "Create parent directories if needed (default: true)",
                        "default": true
                    }
                },
                "required": ["operation", "path"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        FileOpsTool::call(self, args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

        let result = tool.call(args).await.unwrap();
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

        let result = tool.call(write_args).await.unwrap();
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

        let result = tool.call(read_args).await.unwrap();
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

        let result = tool.call(args).await.unwrap();
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

        let result = tool.call(args).await.unwrap();
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

        let result = tool.call(args).await.unwrap();
        assert!(result.success);
        assert_eq!(result.items_affected, Some(2));
    }
}
