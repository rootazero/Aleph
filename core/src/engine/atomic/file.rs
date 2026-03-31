//! File Operations Handler
//!
//! Implements file I/O operations: Read, Write, Move

use crate::error::Result;
use crate::sync_primitives::Arc;
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tracing::debug;

use super::{AtomicResult, ExecutorContext, FileFilter, FileOps, LineRange, WriteMode};

/// File operations handler
///
/// Handles file I/O operations with size limits and security checks.
pub struct FileOpsHandler {
    /// Shared execution context
    context: Arc<ExecutorContext>,

    /// Maximum file size for read/write operations (bytes)
    max_file_size: u64,
}

impl FileOpsHandler {
    /// Create a new file operations handler
    ///
    /// # Arguments
    ///
    /// * `context` - Shared execution context
    /// * `max_file_size` - Maximum file size in bytes (default: 10MB)
    pub fn new(context: Arc<ExecutorContext>, max_file_size: u64) -> Self {
        Self {
            context,
            max_file_size,
        }
    }

    /// Convert file path to module name (for Rust files)
    fn path_to_module_name(&self, path: &Path) -> Option<String> {
        // Get relative path from working directory
        let rel_path = path.strip_prefix(&self.context.working_dir).ok()?;

        // Remove .rs extension
        let path_str = rel_path.to_str()?;
        let without_ext = path_str.strip_suffix(".rs")?;

        // Convert path separators to ::
        let module_name = without_ext.replace(['/', '\\'], "::");

        // Handle mod.rs files
        let module_name = module_name.strip_suffix("::mod").unwrap_or(&module_name);

        Some(module_name.to_string())
    }

    /// Update imports after moving a file
    async fn update_imports_after_move(
        &self,
        old_path: &Path,
        new_path: &Path,
    ) -> Result<Vec<PathBuf>> {
        let mut updated_files = Vec::new();

        // Only handle Rust files for now
        if let Some(ext) = old_path.extension() {
            if ext != "rs" {
                // Not a Rust file, skip import updates
                return Ok(updated_files);
            }
        } else {
            return Ok(updated_files);
        }

        // Extract module names from paths
        let old_module = self.path_to_module_name(old_path);
        let new_module = self.path_to_module_name(new_path);

        if old_module.is_none() || new_module.is_none() {
            return Ok(updated_files);
        }

        let old_mod = old_module.unwrap();
        let new_mod = new_module.unwrap();

        // Find all Rust files in the workspace
        let mut rust_files = Vec::new();
        self.context
            .collect_files_from_directory(
                &self.context.working_dir,
                true,
                &[FileFilter::Extension("rs".to_string())],
                &mut rust_files,
            )
            .await?;

        // Update imports in each file
        for file in &rust_files {
            if let Ok(content) = tokio::fs::read_to_string(file).await {
                let new_content = content.replace(&old_mod, &new_mod);
                if content != new_content {
                    tokio::fs::write(file, new_content).await?;
                    updated_files.push(file.clone());
                }
            }
        }

        Ok(updated_files)
    }
}

#[async_trait]
impl FileOps for FileOpsHandler {
    async fn read(&self, path: &str, range: Option<&LineRange>) -> Result<AtomicResult> {
        let resolved_path = self.context.resolve_path(path)?;

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

    async fn write(&self, path: &str, content: &str, mode: &WriteMode) -> Result<AtomicResult> {
        let resolved_path = self.context.resolve_path(path)?;

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
            output: format!(
                "Wrote {} bytes to {}",
                content.len(),
                resolved_path.display()
            ),
            error: None,
        })
    }

    async fn move_file(
        &self,
        source: &str,
        dest: &str,
        update_imports: bool,
        create_parent: bool,
    ) -> Result<AtomicResult> {
        // Parse paths
        let source_pathbuf = PathBuf::from(source);
        let dest_pathbuf = PathBuf::from(dest);

        debug!(source = ?source_pathbuf, destination = ?dest_pathbuf, "Executing move");

        // Resolve paths
        let source_path = if source_pathbuf.is_absolute() {
            source_pathbuf.clone()
        } else {
            self.context.working_dir.join(&source_pathbuf)
        };

        let dest_path = if dest_pathbuf.is_absolute() {
            dest_pathbuf.clone()
        } else {
            self.context.working_dir.join(&dest_pathbuf)
        };

        // Check source exists
        if !source_path.exists() {
            return Ok(AtomicResult {
                success: false,
                output: String::new(),
                error: Some(format!("Source does not exist: {}", source_path.display())),
            });
        }

        // Check destination doesn't exist
        if dest_path.exists() {
            return Ok(AtomicResult {
                success: false,
                output: String::new(),
                error: Some(format!(
                    "Destination already exists: {}",
                    dest_path.display()
                )),
            });
        }

        // Create parent directory if needed
        if create_parent {
            if let Some(parent) = dest_path.parent() {
                if !parent.exists() {
                    tokio::fs::create_dir_all(parent).await?;
                }
            }
        }

        // Check parent directory exists
        if let Some(parent) = dest_path.parent() {
            if !parent.exists() {
                return Ok(AtomicResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!(
                        "Parent directory does not exist: {}. Use create_parent option.",
                        parent.display()
                    )),
                });
            }
        }

        // Perform the move
        tokio::fs::rename(&source_path, &dest_path).await?;

        // Update imports if requested
        let mut import_updates = Vec::new();
        if update_imports {
            import_updates = self
                .update_imports_after_move(&source_path, &dest_path)
                .await?;
        }

        // Format output
        let output = if import_updates.is_empty() {
            format!("Moved {} to {}", source_path.display(), dest_path.display())
        } else {
            format!(
                "Moved {} to {}\nUpdated imports in {} files:\n{}",
                source_path.display(),
                dest_path.display(),
                import_updates.len(),
                import_updates
                    .iter()
                    .map(|f| format!("  - {}", f.display()))
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
}
