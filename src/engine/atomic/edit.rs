//! Edit Operations Handler
//!
//! Implements text editing and replacement operations

use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use async_trait::async_trait;
use regex::Regex;
use std::path::PathBuf;
use tracing::debug;

use super::{
    AtomicResult, EditOps, ExecutorContext, FileFilter, Patch, SearchPattern, SearchScope,
};
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
                if resolved.exists() && ExecutorContext::should_include_file(&resolved, filters) {
                    files.push(resolved);
                }
            }
            SearchScope::Directory { path, recursive } => {
                let resolved = self.context.resolve_path(path.to_str().unwrap_or(""))?;
                if resolved.exists() && resolved.is_dir() {
                    self.context
                        .collect_files_from_directory(&resolved, *recursive, filters, &mut files)
                        .await?;
                }
            }
            SearchScope::Workspace => {
                self.context
                    .collect_files_from_directory(
                        &self.context.working_dir,
                        true,
                        filters,
                        &mut files,
                    )
                    .await?;
            }
        }

        Ok(files)
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
        let new_content = applier
            .apply_all(&content)
            .map_err(|e| AlephError::tool(format!("Failed to apply patches: {}", e)))?;

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

        // Pre-compile regex once if needed
        let regex = match pattern {
            SearchPattern::Regex { pattern: regex_str } => Some(
                Regex::new(regex_str)
                    .map_err(|e| AlephError::tool(format!("Invalid regex pattern: {}", e)))?,
            ),
            _ => None,
        };

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
                SearchPattern::Regex { .. } => {
                    regex.as_ref().unwrap().replace_all(&content, replacement).to_string()
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
                    SearchPattern::Regex { .. } => regex.as_ref().unwrap().find_iter(&content).count(),
                    SearchPattern::Fuzzy { text, .. } => content.matches(text).count(),
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
                    .map(|r| format!(
                        "  {}: {} replacements",
                        r.file.display(),
                        r.replacement_count
                    ))
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
