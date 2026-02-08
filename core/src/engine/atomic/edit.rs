//! Edit Operations Handler
//!
//! Implements text editing and replacement operations

use std::path::{Path, PathBuf};
use std::sync::Arc;
use async_trait::async_trait;
use regex::Regex;
use tracing::debug;
use crate::error::{AlephError, Result};

use super::{EditOps, ExecutorContext, AtomicResult, Patch, SearchPattern, SearchScope, FileFilter};
use crate::engine::PatchApplier;

/// File replacement result
#[derive(Debug, Clone)]
struct FileReplacement {
    /// File that was modified
    file: PathBuf,
    /// Original content
    old_content: String,
    /// New content after replacement
    new_content: String,
    /// Number of replacements made
    replacement_count: usize,
}

/// Edit operations handler
///
/// Handles text editing via patches and batch replacement operations.
pub struct EditOpsHandler {
    /// Shared execution context
    context: Arc<ExecutorContext>,

    /// Maximum file size for edit operations (bytes)
    max_file_size: u64,
}

impl EditOpsHandler {
    /// Create a new edit operations handler
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

    /// Collect files for search based on scope and filters
    async fn collect_files_for_search(
        &self,
        scope: &SearchScope,
        filters: &[FileFilter],
    ) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();

        match scope {
            SearchScope::File { path } => {
                let resolved = self.context.resolve_path(path.to_str().unwrap_or(""))?;
                if resolved.exists() && self.should_include_file(&resolved, filters) {
                    files.push(resolved);
                }
            }
            SearchScope::Directory { path, recursive } => {
                let resolved = self.context.resolve_path(path.to_str().unwrap_or(""))?;
                if resolved.exists() && resolved.is_dir() {
                    self.collect_files_from_directory(&resolved, *recursive, filters, &mut files)
                        .await?;
                }
            }
            SearchScope::Workspace => {
                // Search in working directory recursively
                self.collect_files_from_directory(&self.context.working_dir, true, filters, &mut files)
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
}

#[async_trait]
impl EditOps for EditOpsHandler {
    async fn edit(&self, path: &str, patches: &[Patch]) -> Result<AtomicResult> {
        let resolved_path = self.context.resolve_path(path)?;

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

    async fn replace(
        &self,
        pattern: &SearchPattern,
        replacement: &str,
        scope: &SearchScope,
        preview: bool,
        dry_run: bool,
    ) -> Result<AtomicResult> {
        debug!(pattern = ?pattern, replacement = replacement, "Executing replace");

        // First, find all matches using search logic
        let files = self.collect_files_for_search(scope, &[]).await?;

        if files.is_empty() {
            return Ok(AtomicResult {
                success: true,
                output: "No files found matching the search scope".to_string(),
                error: None,
            });
        }

        // Perform replacement based on pattern type
        let mut replacements = Vec::new();
        let mut total_replacements = 0;

        for file in &files {
            // Skip files that are too large
            if let Ok(metadata) = tokio::fs::metadata(file).await {
                if metadata.len() > self.max_file_size {
                    continue;
                }
            }

            // Read file content
            let content = match tokio::fs::read_to_string(file).await {
                Ok(c) => c,
                Err(_) => continue,
            };

            // Perform replacement based on pattern type
            let new_content = match pattern {
                SearchPattern::Regex { pattern: regex_str } => {
                    let regex = Regex::new(regex_str)
                        .map_err(|e| AlephError::tool(format!("Invalid regex pattern: {}", e)))?;
                    regex.replace_all(&content, replacement).to_string()
                }
                SearchPattern::Fuzzy { text, .. } => {
                    // Simple case-insensitive replacement
                    content.replace(text, replacement)
                }
                SearchPattern::Ast { .. } => {
                    return Ok(AtomicResult {
                        success: false,
                        output: String::new(),
                        error: Some("AST-based replacement not yet implemented".to_string()),
                    });
                }
            };

            // Count replacements
            if content != new_content {
                let count = match pattern {
                    SearchPattern::Regex { pattern: regex_str } => {
                        let regex = Regex::new(regex_str).unwrap();
                        regex.find_iter(&content).count()
                    }
                    SearchPattern::Fuzzy { text, .. } => {
                        content.matches(text).count()
                    }
                    _ => 0,
                };

                total_replacements += count;

                replacements.push(FileReplacement {
                    file: file.clone(),
                    old_content: content.clone(),
                    new_content: new_content.clone(),
                    replacement_count: count,
                });

                // Write back if not dry_run
                if !dry_run {
                    tokio::fs::write(file, &new_content).await?;
                }
            }
        }

        // Format output
        let output = if replacements.is_empty() {
            "No replacements made".to_string()
        } else if preview {
            // Generate preview with diffs
            let mut preview_output = format!(
                "Preview: {} replacements in {} files\n\n",
                total_replacements,
                replacements.len()
            );

            for repl in &replacements {
                preview_output.push_str(&format!(
                    "File: {}\nReplacements: {}\n",
                    repl.file.display(),
                    repl.replacement_count
                ));

                // Show first few lines of diff
                let old_lines: Vec<&str> = repl.old_content.lines().collect();
                let new_lines: Vec<&str> = repl.new_content.lines().collect();

                for (i, (old, new)) in old_lines.iter().zip(new_lines.iter()).enumerate() {
                    if old != new {
                        preview_output.push_str(&format!("  Line {}:\n", i + 1));
                        preview_output.push_str(&format!("    - {}\n", old));
                        preview_output.push_str(&format!("    + {}\n", new));
                    }
                }
                preview_output.push('\n');
            }

            preview_output
        } else {
            // Summary output
            let mode = if dry_run { " (dry run)" } else { "" };
            format!(
                "Made {} replacements in {} files{}\n{}",
                total_replacements,
                replacements.len(),
                mode,
                replacements
                    .iter()
                    .map(|r| format!("  {}: {} replacements", r.file.display(), r.replacement_count))
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
