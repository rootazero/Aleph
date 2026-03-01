//! Search Operations Handler
//!
//! Implements file search with pattern matching and filters

use std::path::{Path, PathBuf};
use crate::sync_primitives::Arc;
use async_trait::async_trait;
use regex::Regex;
use tracing::debug;
use crate::error::{AlephError, Result};

use super::{SearchOps, ExecutorContext, AtomicResult, SearchPattern, SearchScope, FileFilter};

/// Search match result
#[derive(Debug, Clone)]
struct SearchMatch {
    /// File where match was found
    file: PathBuf,
    /// Line number (1-indexed)
    line_number: usize,
    /// Content of the matching line
    line_content: String,
}

/// Search operations handler
///
/// Handles file search with regex, fuzzy, and AST-based pattern matching.
pub struct SearchOpsHandler {
    /// Shared execution context
    context: Arc<ExecutorContext>,

    /// Maximum file size for search operations (10MB)
    max_file_size: u64,
}

impl SearchOpsHandler {
    /// Create a new search operations handler
    ///
    /// # Arguments
    ///
    /// * `context` - Shared execution context
    pub fn new(context: Arc<ExecutorContext>) -> Self {
        Self {
            context,
            max_file_size: 10 * 1024 * 1024, // 10MB
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
                        });
                    }
                }
            }
        }

        Ok(matches)
    }
}

#[async_trait]
impl SearchOps for SearchOpsHandler {
    async fn search(
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
            SearchPattern::Ast { query: _, language } => {
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
}
