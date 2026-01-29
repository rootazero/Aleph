//! Core file operations: list, read, write, move, copy, delete, mkdir

use std::fs;
use std::path::Path;
use tracing::{debug, info};

use crate::builtin_tools::error::ToolError;
use super::path_utils::check_and_resolve_path;
use super::state::record_written_file;
use super::types::{FileInfo, FileOpsOutput};

/// Execute a list operation
pub async fn execute_list(
    path: &Path,
    denied_paths: &[String],
) -> Result<FileOpsOutput, ToolError> {
    let canonical = check_and_resolve_path(path, denied_paths)?;

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
pub async fn execute_read(
    path: &Path,
    denied_paths: &[String],
    max_read_size: u64,
) -> Result<FileOpsOutput, ToolError> {
    let canonical = check_and_resolve_path(path, denied_paths)?;

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

    if metadata.len() > max_read_size {
        return Err(ToolError::InvalidArgs(format!(
            "File too large: {} bytes (max {})",
            metadata.len(),
            max_read_size
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
pub async fn execute_write(
    path: &Path,
    content: &str,
    create_parents: bool,
    denied_paths: &[String],
) -> Result<FileOpsOutput, ToolError> {
    let canonical = check_and_resolve_path(path, denied_paths)?;

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

    // Record the written file for attachment tracking
    record_written_file(canonical.clone(), bytes, "write");

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
pub async fn execute_move(
    from: &Path,
    to: &Path,
    create_parents: bool,
    denied_paths: &[String],
) -> Result<FileOpsOutput, ToolError> {
    let from_canonical = check_and_resolve_path(from, denied_paths)?;
    let to_canonical = check_and_resolve_path(to, denied_paths)?;

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
pub async fn execute_copy(
    from: &Path,
    to: &Path,
    create_parents: bool,
    denied_paths: &[String],
) -> Result<FileOpsOutput, ToolError> {
    let from_canonical = check_and_resolve_path(from, denied_paths)?;
    let to_canonical = check_and_resolve_path(to, denied_paths)?;

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
        copy_dir_recursive(&from_canonical, &to_canonical)?
    };

    info!(from = %from_canonical.display(), to = %to_canonical.display(), bytes, "Copied");

    // Record the copied file for attachment tracking
    record_written_file(to_canonical.clone(), bytes, "copy");

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
fn copy_dir_recursive(from: &Path, to: &Path) -> Result<u64, ToolError> {
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
            total_bytes += copy_dir_recursive(&from_path, &to_path)?;
        } else {
            total_bytes += fs::copy(&from_path, &to_path)
                .map_err(|e| ToolError::Execution(format!("Failed to copy file: {}", e)))?;
        }
    }

    Ok(total_bytes)
}

/// Execute a delete operation
pub async fn execute_delete(
    path: &Path,
    denied_paths: &[String],
) -> Result<FileOpsOutput, ToolError> {
    let canonical = check_and_resolve_path(path, denied_paths)?;

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
pub async fn execute_mkdir(
    path: &Path,
    create_parents: bool,
    denied_paths: &[String],
) -> Result<FileOpsOutput, ToolError> {
    let canonical = check_and_resolve_path(path, denied_paths)?;

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
