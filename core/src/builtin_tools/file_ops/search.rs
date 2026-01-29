//! Search operations for file system

use std::fs;
use std::path::Path;
use tracing::{debug, info};

use crate::builtin_tools::error::ToolError;
use super::path_utils::check_and_resolve_path;
use super::types::{FileInfo, FileOpsOutput};

/// Execute a search operation
pub async fn execute_search(
    dir: &Path,
    pattern: &str,
    denied_paths: &[String],
) -> Result<FileOpsOutput, ToolError> {
    let canonical = check_and_resolve_path(dir, denied_paths)?;

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
