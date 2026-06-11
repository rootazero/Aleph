//! Batch file operations: batch_move, organize

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tracing::{debug, info};

use super::path_utils::check_and_resolve_path;
use super::types::{FileInfo, FileOpsOutput};
use crate::builtin_tools::error::ToolError;

/// Execute a batch move operation
///
/// Moves all files matching the pattern to the destination directory
pub async fn execute_batch_move(
    dir: &Path,
    pattern: &str,
    dest: &Path,
    create_parents: bool,
    denied_paths: &[String],
    output_dir_override: Option<&std::path::Path>,
) -> Result<FileOpsOutput, ToolError> {
    let canonical = check_and_resolve_path(dir, denied_paths, output_dir_override)?;
    let dest_canonical = if dest.exists() {
        check_and_resolve_path(dest, denied_paths, output_dir_override)?
    } else if create_parents {
        // Security: check destination path BEFORE creating it, to prevent
        // writing into denied directories (e.g., ~/.ssh with create_parents=true).
        let checked_dest = check_and_resolve_path(dest, denied_paths, output_dir_override)?;
        // Create destination if needed
        fs::create_dir_all(&checked_dest)
            .map_err(|e| ToolError::Execution(format!("Failed to create destination: {e}")))?;
        checked_dest
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
        .map_err(|e| ToolError::InvalidArgs(format!("Invalid glob pattern: {e}")))?
    {
        match entry {
            Ok(path) => {
                if path.is_file() {
                    let file_name = path.file_name().unwrap_or_default();
                    // Sanitize filename to prevent path traversal via "../" in names
                    let safe_name = file_name.to_string_lossy().replace(['/', '\\'], "_");
                    let dest_path = dest_canonical.join(&safe_name);

                    match fs::rename(&path, &dest_path) {
                        Ok(_) => {
                            let size = fs::metadata(&dest_path).map(|m| m.len()).unwrap_or(0);
                            moved_files.push(FileInfo {
                                name: file_name.to_string_lossy().to_string(),
                                path: dest_path.to_string_lossy().to_string(),
                                is_dir: false,
                                size,
                                extension: path
                                    .extension()
                                    .map(|e| e.to_string_lossy().to_string()),
                                lines: None,
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
        bytes_written: None,
        items_affected: Some(count),
        summary: None,
    })
}

/// Execute an organize operation
///
/// Automatically organizes files by type into categorized folders
pub async fn execute_organize(
    dir: &Path,
    denied_paths: &[String],
    output_dir_override: Option<&std::path::Path>,
) -> Result<FileOpsOutput, ToolError> {
    let canonical = check_and_resolve_path(dir, denied_paths, output_dir_override)?;

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
                "jpg", "jpeg", "png", "gif", "webp", "svg", "bmp", "ico", "tiff", "heic", "heif",
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
                "rs", "py", "js", "ts", "jsx", "tsx", "java", "c", "cpp", "h", "hpp", "go", "rb",
                "php", "swift", "kt", "scala", "html", "css", "scss", "json", "xml", "yaml", "yml",
                "toml", "sh", "bash", "sql",
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
        .map_err(|e| ToolError::Execution(format!("Failed to read directory: {e}")))?
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

        // Create the category directory. Sorting files into category folders
        // is `organize`'s whole purpose, so it always creates them — each is a
        // one-level child of an already-existing directory.
        let category_dir = canonical.join(category);
        if !category_dir.exists() {
            if let Err(e) = fs::create_dir(&category_dir) {
                errors.push(format!("Failed to create {category}: {e}"));
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
                let size = fs::metadata(&dest_path).map(|m| m.len()).unwrap_or(0);
                moved_files.push(FileInfo {
                    name: file_name.to_string_lossy().to_string(),
                    path: dest_path.to_string_lossy().to_string(),
                    is_dir: false,
                    size,
                    extension: Some(ext),
                    lines: None,
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
        .map(|(cat, cnt)| format!("{cat}: {cnt}"))
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
        bytes_written: None,
        items_affected: Some(count),
        summary: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn organize_sorts_files_into_category_folders() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("photo.png"), b"image-bytes").unwrap();
        fs::write(dir.path().join("notes.md"), b"# notes").unwrap();
        fs::write(dir.path().join("main.rs"), b"fn main() {}").unwrap();

        // `organize` must always create its category folders — historically it
        // only did so when an unrelated `create_parents` flag was set.
        let out = execute_organize(dir.path(), &[], None).await.unwrap();

        assert!(out.success, "message: {}", out.message);
        assert_eq!(out.items_affected, Some(3));
        assert!(dir.path().join("Images/photo.png").is_file());
        assert!(dir.path().join("Documents/notes.md").is_file());
        assert!(dir.path().join("Code/main.rs").is_file());

        // FileInfo.size must report the real moved-file size, not a 0 placeholder.
        let files = out.files.unwrap();
        assert!(files.iter().all(|f| f.size > 0), "sizes: {files:?}");
    }

    #[tokio::test]
    async fn batch_move_reports_real_file_sizes() {
        let src = tempdir().unwrap();
        let dst = tempdir().unwrap();
        fs::write(src.path().join("a.log"), b"hello-log").unwrap();

        let out = execute_batch_move(src.path(), "*.log", dst.path(), false, &[], None)
            .await
            .unwrap();

        assert!(out.success, "message: {}", out.message);
        let files = out.files.unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].size, 9, "size must be the real byte count");
    }
}
