//! Search operations for file system

use std::fs;
use std::path::Path;
use tracing::{debug, info};

use super::path_utils::{check_and_resolve_path, reject_unsafe_glob_pattern};
use super::types::{FileInfo, FileOpsOutput};
use crate::builtin_tools::error::ToolError;

/// Execute a search operation
pub async fn execute_search(
    dir: &Path,
    pattern: &str,
    denied_paths: &[String],
    output_dir_override: Option<&std::path::Path>,
) -> Result<FileOpsOutput, ToolError> {
    let canonical = check_and_resolve_path(dir, denied_paths, output_dir_override)?;

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

    reject_unsafe_glob_pattern(pattern)?;

    let full_pattern = canonical.join(pattern);
    let pattern_str = full_pattern.to_string_lossy();

    let mut files = Vec::new();

    for entry in glob::glob(&pattern_str)
        .map_err(|e| ToolError::InvalidArgs(format!("Invalid glob pattern: {e}")))?
    {
        match entry {
            Ok(path) => {
                // Defense in depth: even a relative pattern can match a symlink
                // pointing outside the base. Re-check each match against the
                // deny list; silently skip denied matches.
                if check_and_resolve_path(&path, denied_paths, output_dir_override).is_err() {
                    continue;
                }
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
                        lines: None,
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
        bytes_written: None,
        items_affected: Some(count),
        summary: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn absolute_pattern_is_rejected() {
        let dir = std::env::temp_dir();
        let out = execute_search(&dir, "/etc/*", &[], None).await;
        assert!(
            matches!(out, Err(ToolError::InvalidArgs(_))),
            "absolute glob pattern must be rejected, got {out:?}"
        );
    }

    #[tokio::test]
    async fn parent_escape_pattern_is_rejected() {
        let dir = std::env::temp_dir();
        let out = execute_search(&dir, "../*", &[], None).await;
        assert!(
            matches!(out, Err(ToolError::InvalidArgs(_))),
            "`..` glob pattern must be rejected, got {out:?}"
        );
    }
}
