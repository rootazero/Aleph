//! Core file operations: list, move, copy, delete, mkdir, search, stats,
//! batch-move, organize. Reading and writing are deliberately NOT here — they
//! are the separate `file_read` / `file_write` tools (so `file_read` stays the
//! single, cat_guard-covered read path; `file_ops` has no read/write arm).

use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

use super::path_utils::{check_and_resolve_path, path_is_denied, resolve_for_removal};
use super::types::{FileInfo, FileOpsOutput, DEFAULT_ENTRY_LIMIT};
use crate::builtin_tools::error::ToolError;
use crate::tools::path_locks::{lock_path, lock_path_pair};

/// Execute a list operation
pub async fn execute_list(
    path: &Path,
    denied_paths: &[String],
    output_dir_override: Option<&std::path::Path>,
    limit: Option<usize>,
) -> Result<FileOpsOutput, ToolError> {
    let canonical = check_and_resolve_path(path, denied_paths, output_dir_override)?;

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
        .map_err(|e| ToolError::Execution(format!("Failed to read directory: {e}")))?
    {
        let entry =
            entry.map_err(|e| ToolError::Execution(format!("Failed to read entry: {e}")))?;

        let metadata = entry
            .metadata()
            .map_err(|e| ToolError::Execution(format!("Failed to get metadata: {e}")))?;

        let entry_path = entry.path();
        files.push(FileInfo {
            name: entry.file_name().to_string_lossy().to_string(),
            path: entry_path.to_string_lossy().to_string(),
            is_dir: metadata.is_dir(),
            size: metadata.len(),
            extension: entry_path
                .extension()
                .map(|e| e.to_string_lossy().to_string()),
            lines: None,
        });
    }

    // Sort: directories first, then by name
    files.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });

    // Cap after sorting so the kept entries are the deterministic head of a
    // stable order, not whatever `read_dir` happened to yield first.
    let cap = limit.unwrap_or(DEFAULT_ENTRY_LIMIT).max(1);
    let count = files.len();
    files.truncate(cap);
    info!(path = %canonical.display(), count, shown = files.len(), "Listed directory");

    Ok(FileOpsOutput {
        success: true,
        operation: "list".to_string(),
        message: format!(
            "Listed {count} items in {}{}",
            canonical.display(),
            super::search::entry_cap_note_with(count, files.len(), cap, " to see the rest")
        ),
        files: Some(files),
        bytes_written: None,
        items_affected: Some(count),
        summary: None,
    })
}

/// Validate and read a file's raw bytes.
///
/// Returns the canonicalized path, the file's byte size, and its raw contents.
/// Higher-level concerns — binary detection, lossy UTF-8 decoding, and line
/// windowing — are deliberately left to the caller (`file_read`), keeping this
/// function a single, focused I/O boundary.
pub(super) async fn read_file_bytes(
    path: &Path,
    denied_paths: &[String],
    max_read_size: u64,
    output_dir_override: Option<&std::path::Path>,
) -> Result<(PathBuf, u64, Vec<u8>), ToolError> {
    let canonical = check_and_resolve_path(path, denied_paths, output_dir_override)?;

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
        .map_err(|e| ToolError::Execution(format!("Failed to get metadata: {e}")))?;
    let size = metadata.len();

    if size > max_read_size {
        return Err(ToolError::InvalidArgs(format!(
            "File too large: {size} bytes (max {max_read_size})"
        )));
    }

    let bytes = fs::read(&canonical)
        .map_err(|e| ToolError::Execution(format!("Failed to read file: {e}")))?;

    info!(path = %canonical.display(), size, "Read file");

    Ok((canonical, size, bytes))
}

/// Execute a write operation.
///
/// Returns the canonicalized path written and the number of bytes written, so
/// callers can report the real resolved path rather than reverse-engineering it
/// from a human-readable message string.
pub(super) async fn execute_write(
    path: &Path,
    content: &str,
    create_parents: bool,
    denied_paths: &[String],
    output_dir_override: Option<&std::path::Path>,
) -> Result<(PathBuf, u64), ToolError> {
    let canonical = check_and_resolve_path(path, denied_paths, output_dir_override)?;

    // Cross-agent write guard: serialize against any other harness (parent
    // agent, concurrent subagent, team member) mutating the same canonical
    // path — the in-batch concurrency claims gate cannot see across harness
    // instances.
    let _path_guard = crate::tools::path_locks::lock_path(&canonical).await;

    // Create parent directories if needed
    if create_parents {
        if let Some(parent) = canonical.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).map_err(|e| {
                    ToolError::Execution(format!("Failed to create directories: {e}"))
                })?;
                debug!(path = %parent.display(), "Created parent directories");
            }
        }
    }

    let bytes = content.len() as u64;
    // Atomic write: staged temp file + rename, so a crash never leaves a
    // half-written file. Parent directories are already created above.
    crate::utils::atomic_write::atomic_write_file(&canonical, content)
        .await
        .map_err(|e| ToolError::Execution(format!("Failed to write file: {e}")))?;

    info!(path = %canonical.display(), bytes, "Wrote file");

    Ok((canonical, bytes))
}

/// Execute a move operation
pub async fn execute_move(
    from: &Path,
    to: &Path,
    create_parents: bool,
    denied_paths: &[String],
    output_dir_override: Option<&std::path::Path>,
) -> Result<FileOpsOutput, ToolError> {
    // A symlink source must be renamed as the LINK, not its target (which
    // `check_and_resolve_path` would canonicalize to and move out from under
    // the link). `fs::rename` never follows a final symlink, so operating on
    // the link path is correct.
    let from_canonical = resolve_for_removal(from, denied_paths, output_dir_override)?;
    let to_canonical = check_and_resolve_path(to, denied_paths, output_dir_override)?;

    // Cross-agent write serialization (defense in depth alongside the batch
    // claim, which only sees ONE harness's batch): both endpoints, sorted, so
    // crossed concurrent moves cannot ABBA-deadlock. Held through the
    // exists-check → rename critical section — the same guard `file_write` /
    // `file_edit` / `apply_patch` already take; `file_ops` mutations were the
    // one family that skipped it.
    let _path_guards = lock_path_pair(&from_canonical, &to_canonical).await;

    // lstat, not `exists()`: a dangling symlink source must still be movable
    // (`exists()` follows the link and would wrongly report it missing).
    if from_canonical.symlink_metadata().is_err() {
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
                    ToolError::Execution(format!("Failed to create directories: {e}"))
                })?;
            }
        }
    }

    fs::rename(&from_canonical, &to_canonical)
        .map_err(|e| ToolError::Execution(format!("Failed to move: {e}")))?;

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
        bytes_written: None,
        items_affected: Some(1),
        summary: None,
    })
}

/// Execute a copy operation
pub async fn execute_copy(
    from: &Path,
    to: &Path,
    create_parents: bool,
    denied_paths: &[String],
    output_dir_override: Option<&std::path::Path>,
) -> Result<FileOpsOutput, ToolError> {
    let from_canonical = check_and_resolve_path(from, denied_paths, output_dir_override)?;
    let to_canonical = check_and_resolve_path(to, denied_paths, output_dir_override)?;

    // Cross-agent serialization: the destination is written and the source is
    // read mid-copy (a concurrent writer to either tears the copy). Sorted
    // pair — see `execute_move`.
    let _path_guards = lock_path_pair(&from_canonical, &to_canonical).await;

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
                    ToolError::Execution(format!("Failed to create directories: {e}"))
                })?;
            }
        }
    }

    let bytes = if from_canonical.is_file() {
        fs::copy(&from_canonical, &to_canonical)
            .map_err(|e| ToolError::Execution(format!("Failed to copy: {e}")))?
    } else {
        // Directory copy - recursive. `denied_paths` are threaded through so a
        // symlink whose canonical target is a protected credential store is
        // skipped rather than followed and copied out.
        let mut visited = std::collections::HashSet::new();
        copy_dir_recursive(&from_canonical, &to_canonical, denied_paths, &mut visited)?
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
        bytes_written: Some(bytes),
        items_affected: Some(1),
        summary: None,
    })
}

/// Recursively copy a directory with symlink-cycle + deny guards.
fn copy_dir_recursive(
    from: &Path,
    to: &Path,
    denied_paths: &[String],
    visited: &mut std::collections::HashSet<PathBuf>,
) -> Result<u64, ToolError> {
    fs::create_dir_all(to)
        .map_err(|e| ToolError::Execution(format!("Failed to create directory: {e}")))?;

    let mut total_bytes = 0u64;

    for entry in fs::read_dir(from)
        .map_err(|e| ToolError::Execution(format!("Failed to read directory: {e}")))?
    {
        let entry =
            entry.map_err(|e| ToolError::Execution(format!("Failed to read entry: {e}")))?;

        let from_path = entry.path();
        let to_path = to.join(entry.file_name());

        if from_path.is_symlink() {
            let canonical = fs::canonicalize(&from_path).map_err(|e| {
                ToolError::Execution(format!("Failed to canonicalize symlink: {e}"))
            })?;
            // Deny guard: a symlink whose target resolves under a protected path
            // would otherwise exfiltrate blacklisted content into the copy. Skip
            // it (recursive copy follows symlinks by design; the escape is the
            // hole). Checked before the cycle guard so a denied target does not
            // consume a `visited` slot.
            if path_is_denied(&canonical, denied_paths) {
                info!(
                    symlink = %from_path.display(),
                    target = %canonical.display(),
                    "copy: skipping symlink to protected target"
                );
                continue;
            }
            if !visited.insert(canonical.clone()) {
                return Err(ToolError::Execution(format!(
                    "Symlink cycle detected at {}",
                    from_path.display()
                )));
            }
        }

        if from_path.is_dir() {
            total_bytes += copy_dir_recursive(&from_path, &to_path, denied_paths, visited)?;
        } else {
            total_bytes += fs::copy(&from_path, &to_path)
                .map_err(|e| ToolError::Execution(format!("Failed to copy file: {e}")))?;
        }
    }

    Ok(total_bytes)
}

/// Recursively count a path plus every entry beneath it, so `delete` can report
/// a truthful item count — `remove_dir_all` itself returns none. Symlinks count
/// as a single entry and are not followed, matching `remove_dir_all`, which
/// unlinks a symlink rather than descending into its target.
fn count_path_entries(path: &Path) -> usize {
    let mut total = 1; // the path itself
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            match entry.file_type() {
                Ok(ft) if ft.is_dir() => total += count_path_entries(&entry.path()),
                _ => total += 1,
            }
        }
    }
    total
}

/// Execute a delete operation
pub async fn execute_delete(
    path: &Path,
    denied_paths: &[String],
    output_dir_override: Option<&std::path::Path>,
) -> Result<FileOpsOutput, ToolError> {
    // Deleting a symlink must unlink the LINK, not descend into its target
    // (`check_and_resolve_path` would canonicalize the link to its target and
    // `remove_dir_all` would then destroy the pointed-at tree). `resolve_for_removal`
    // returns the un-followed link path when the final component is a symlink.
    let canonical = resolve_for_removal(path, denied_paths, output_dir_override)?;

    // Cross-agent write guard: serialize the exists-check → remove critical
    // section against any other harness mutating the same path (a concurrent
    // `file_edit` read-modify-write on this path must not interleave with its
    // deletion). Same guard the writers take.
    let _guard = lock_path(&canonical).await;

    // `symlink_metadata` (lstat) never follows the final component, so a
    // dangling or dir-pointing symlink is still detected as present and as a
    // symlink.
    let lmeta = canonical
        .symlink_metadata()
        .map_err(|_| ToolError::Execution(format!("Path not found: {}", path.display())))?;
    let is_symlink = lmeta.file_type().is_symlink();
    let is_dir = !is_symlink && lmeta.is_dir();
    let items_deleted = if is_symlink {
        // Unlink the symlink itself — never touch its target.
        fs::remove_file(&canonical)
            .map_err(|e| ToolError::Execution(format!("Failed to delete symlink: {e}")))?;
        1
    } else if is_dir {
        // Count the whole tree before removal so the reported figure is the
        // true total, not just the directory's top-level entries.
        let count = count_path_entries(&canonical);
        fs::remove_dir_all(&canonical)
            .map_err(|e| ToolError::Execution(format!("Failed to delete directory: {e}")))?;
        count
    } else {
        fs::remove_file(&canonical)
            .map_err(|e| ToolError::Execution(format!("Failed to delete file: {e}")))?;
        1
    };

    info!(path = %canonical.display(), is_dir, is_symlink, items_deleted, "Deleted");

    Ok(FileOpsOutput {
        success: true,
        operation: "delete".to_string(),
        message: format!("Deleted {} ({} items)", canonical.display(), items_deleted),
        files: None,
        bytes_written: None,
        items_affected: Some(items_deleted),
        summary: None,
    })
}

/// Execute a mkdir operation
pub async fn execute_mkdir(
    path: &Path,
    create_parents: bool,
    denied_paths: &[String],
    output_dir_override: Option<&std::path::Path>,
) -> Result<FileOpsOutput, ToolError> {
    let canonical = check_and_resolve_path(path, denied_paths, output_dir_override)?;

    // Cross-agent write guard: serialize the exists-check → create critical
    // section against a concurrent create/delete of the same path (mirror of
    // `execute_delete`).
    let _guard = lock_path(&canonical).await;

    if canonical.exists() {
        if canonical.is_dir() {
            return Ok(FileOpsOutput {
                success: true,
                operation: "mkdir".to_string(),
                message: format!("Directory already exists: {}", canonical.display()),
                files: None,
                bytes_written: None,
                items_affected: Some(0),
                summary: None,
            });
        } else {
            return Err(ToolError::InvalidArgs(format!(
                "Path exists but is not a directory: {}",
                path.display()
            )));
        }
    }

    if create_parents {
        fs::create_dir_all(&canonical)
            .map_err(|e| ToolError::Execution(format!("Failed to create directories: {e}")))?;
    } else {
        fs::create_dir(&canonical)
            .map_err(|e| ToolError::Execution(format!("Failed to create directory: {e}")))?;
    }

    info!(path = %canonical.display(), "Created directory");

    Ok(FileOpsOutput {
        success: true,
        operation: "mkdir".to_string(),
        message: format!("Created directory: {}", canonical.display()),
        files: None,
        bytes_written: None,
        items_affected: Some(1),
        summary: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn delete_reports_full_recursive_item_count() {
        let root = tempdir().unwrap();
        let target = root.path().join("tree");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("a.txt"), b"a").unwrap();
        fs::create_dir(target.join("sub")).unwrap();
        fs::write(target.join("sub/b.txt"), b"b").unwrap();
        fs::write(target.join("sub/c.txt"), b"c").unwrap();
        // tree + a.txt + sub + sub/b.txt + sub/c.txt = 5 entries.

        let out = execute_delete(&target, &[], None).await.unwrap();

        assert!(out.success);
        assert_eq!(
            out.items_affected,
            Some(5),
            "must count nested entries, not only the top level"
        );
        assert!(!target.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn delete_symlink_unlinks_link_not_target_tree() {
        use std::os::unix::fs::symlink;
        let root = tempdir().unwrap();
        // A precious directory tree the symlink points at.
        let target = root.path().join("precious");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep.txt"), b"important").unwrap();
        // A symlink to it.
        let link = root.path().join("link");
        symlink(&target, &link).unwrap();

        let out = execute_delete(&link, &[], None).await.unwrap();
        assert!(out.success);
        assert_eq!(out.items_affected, Some(1), "only the link is removed");
        // The link is gone; the target tree survives intact.
        assert!(!link.exists());
        assert!(target.exists());
        assert!(target.join("keep.txt").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn move_symlink_renames_link_not_target() {
        use std::os::unix::fs::symlink;
        let root = tempdir().unwrap();
        let target = root.path().join("precious");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep.txt"), b"important").unwrap();
        let link = root.path().join("link");
        symlink(&target, &link).unwrap();
        let dest = root.path().join("renamed-link");

        let out = execute_move(&link, &dest, false, &[], None).await.unwrap();
        assert!(out.success);
        // The link moved; the target tree is untouched at its original location.
        assert!(!link.exists());
        assert!(dest.symlink_metadata().unwrap().file_type().is_symlink());
        assert!(target.join("keep.txt").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn copy_skips_symlink_to_denied_target() {
        use std::os::unix::fs::symlink;
        let root = tempdir().unwrap();
        // A "credential" store we mark denied.
        let secret_dir = root.path().join("secret");
        fs::create_dir(&secret_dir).unwrap();
        fs::write(secret_dir.join("id_rsa"), b"PRIVATE KEY").unwrap();
        let denied = vec![secret_dir.to_string_lossy().to_string()];

        // A source dir containing a symlink INTO the denied store.
        let src = root.path().join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("ok.txt"), b"fine").unwrap();
        symlink(&secret_dir, src.join("leak")).unwrap();

        let dst = root.path().join("dst");
        let out = execute_copy(&src, &dst, true, &denied, None).await.unwrap();
        assert!(out.success);
        // The benign file copied; the symlinked credential store did NOT.
        assert!(dst.join("ok.txt").exists());
        assert!(!dst.join("leak").exists(), "denied symlink must be skipped");
    }

    #[tokio::test]
    async fn write_then_read_back_roundtrips() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested/out.txt");
        let (canonical, bytes) = execute_write(&path, "payload", true, &[], None)
            .await
            .unwrap();
        assert_eq!(bytes, 7);
        assert_eq!(fs::read_to_string(&canonical).unwrap(), "payload");
    }
}
