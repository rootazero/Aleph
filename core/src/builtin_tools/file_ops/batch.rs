//! Batch file operations: batch_move, organize

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tracing::{debug, info};

use crate::builtin_tools::error::ToolError;
use super::path_utils::check_and_resolve_path;
use super::types::{FileInfo, FileOpsOutput};

/// Execute a batch move operation
///
/// Moves all files matching the pattern to the destination directory
pub async fn execute_batch_move(
    dir: &Path,
    pattern: &str,
    dest: &Path,
    create_parents: bool,
    denied_paths: &[String],
) -> Result<FileOpsOutput, ToolError> {
    let canonical = check_and_resolve_path(dir, denied_paths)?;
    let dest_canonical = if dest.exists() {
        check_and_resolve_path(dest, denied_paths)?
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
pub async fn execute_organize(
    dir: &Path,
    create_parents: bool,
    denied_paths: &[String],
) -> Result<FileOpsOutput, ToolError> {
    let canonical = check_and_resolve_path(dir, denied_paths)?;

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
    let mut category_counts: HashMap<String, usize> = HashMap::new();

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
