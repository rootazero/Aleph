//! Core file operations: list, move, copy, delete, mkdir, search, stats,
//! batch-move, organize. Reading and writing are deliberately NOT here — they
//! are the separate `file_read` / `file_write` tools (so `file_read` stays the
//! single, cat_guard-covered read path; `file_ops` has no read/write arm).

use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

use super::path_utils::{
    check_and_resolve_path, contains_denied_descendant, path_is_denied, resolve_for_removal,
};
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
    // One un-stat-able entry (EACCES, or an entry removed mid-walk) used to fail
    // the entire listing through `?`. `search` and `stats` already skip such
    // entries and keep walking; the count is reported below so a short listing
    // is never silent.
    let mut skipped = 0usize;
    for entry in fs::read_dir(&canonical)
        .map_err(|e| ToolError::Execution(format!("Failed to read directory: {e}")))?
    {
        let Ok(entry) = entry else {
            skipped += 1;
            continue;
        };

        let Ok(metadata) = entry.metadata() else {
            skipped += 1;
            continue;
        };

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
    info!(path = %canonical.display(), count, shown = files.len(), skipped, "Listed directory");

    let skipped_note = if skipped > 0 {
        format!(". Skipped {skipped} unreadable entries")
    } else {
        String::new()
    };

    Ok(FileOpsOutput {
        success: true,
        operation: "list".to_string(),
        message: format!(
            "Listed {count} items in {}{}{skipped_note}",
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
    let Ok(from_meta) = from_canonical.symlink_metadata() else {
        return Err(ToolError::Execution(format!(
            "Source not found: {}",
            from.display()
        )));
    };

    // `rename` relocates an entire tree in one syscall, so moving the PARENT of
    // a protected entry carries `secrets.vault` / the `data/` auth databases out
    // to a location an ordinary `file_read` will happily read — the
    // non-destructive twin of the `delete` hole. `is_dir()` is false for a
    // symlink here (lstat), and moving a link moves only the link.
    if from_meta.is_dir() {
        if let Some(protected) = contains_denied_descendant(&from_canonical, denied_paths) {
            return Err(ToolError::InvalidArgs(format!(
                "Access denied: {} contains a protected location ({})",
                from.display(),
                protected.display()
            )));
        }
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

        let is_symlink = from_path.is_symlink();

        let canonical = match fs::canonicalize(&from_path) {
            Ok(canonical) => canonical,
            Err(e) if is_symlink => {
                return Err(ToolError::Execution(format!(
                    "Failed to canonicalize symlink: {e}"
                )))
            }
            // A plain entry that cannot be resolved (removed mid-walk, EACCES)
            // cannot be shown to be outside the denylist, and it is only
            // reachable at all because the top-level gate never saw it. Skipping
            // is the conservative half of that pair; copying it blind is what
            // this guard exists to stop.
            Err(e) => {
                info!(
                    entry = %from_path.display(),
                    error = %e,
                    "copy: skipping entry that cannot be resolved"
                );
                continue;
            }
        };

        // Deny guard on EVERY entry, not just symlinks: a plain file or
        // directory whose own path is protected (`<config_dir>/data`, `~/.ssh`)
        // is reached whenever the caller copies its PARENT — which the denylist
        // does not name, so `check_and_resolve_path` waved the copy through and
        // this is the only point that sees the descendant. A symlink is the same
        // hole reached through its target. Checked before the cycle guard so a
        // denied target does not consume a `visited` slot.
        if path_is_denied(&canonical, denied_paths) {
            info!(
                entry = %from_path.display(),
                target = %canonical.display(),
                "copy: skipping protected entry"
            );
            continue;
        }
        if is_symlink && !visited.insert(canonical.clone()) {
            return Err(ToolError::Execution(format!(
                "Symlink cycle detected at {}",
                from_path.display()
            )));
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

    // `remove_dir_all` on the PARENT of a protected entry destroys what deleting
    // that entry directly is correctly refused: nothing on the denylist names
    // `<config_dir>` itself, so the downward check waved this through and
    // `secrets.vault` + the `data/` auth databases went with the tree.
    if is_dir {
        if let Some(protected) = contains_denied_descendant(&canonical, denied_paths) {
            return Err(ToolError::InvalidArgs(format!(
                "Access denied: {} contains a protected location ({})",
                path.display(),
                protected.display()
            )));
        }
    }

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

    /// A protected entry reached by copying its PARENT. Nothing on the denylist
    /// names the parent, so the top-level gate passes and only the per-entry
    /// re-check can stop `<config_dir>/data` from being copied out to a location
    /// a later `file_read` is happy to read.
    #[tokio::test]
    async fn copy_skips_denied_plain_directory() {
        let root = tempdir().unwrap();
        let src = root.path().join("aleph");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("ok.txt"), b"fine").unwrap();
        // A PLAIN (non-symlink) protected directory inside the copied tree.
        let secret_dir = src.join("data");
        fs::create_dir(&secret_dir).unwrap();
        fs::write(secret_dir.join("pairing.db"), b"DB").unwrap();
        let denied = vec![secret_dir.to_string_lossy().to_string()];

        let dst = root.path().join("backup");
        let out = execute_copy(&src, &dst, true, &denied, None).await.unwrap();
        assert!(out.success);
        assert!(dst.join("ok.txt").exists(), "benign file must still copy");
        assert!(
            !dst.join("data").exists(),
            "protected plain directory must not be copied out"
        );
    }

    #[tokio::test]
    async fn delete_refuses_parent_holding_protected_entry() {
        let root = tempdir().unwrap();
        let config = root.path().join("aleph");
        fs::create_dir(&config).unwrap();
        let vault = config.join("secrets.vault");
        fs::write(&vault, b"ENCRYPTED").unwrap();
        let denied = vec![vault.to_string_lossy().to_string()];

        let err = execute_delete(&config, &denied, None)
            .await
            .expect_err("deleting the parent of a protected entry must be refused");
        assert!(
            err.to_string().contains("secrets.vault"),
            "refusal must name the protected location, got: {err}"
        );
        assert!(vault.exists(), "the protected leaf must survive");
    }

    #[tokio::test]
    async fn move_refuses_parent_holding_protected_entry() {
        let root = tempdir().unwrap();
        let config = root.path().join("aleph");
        fs::create_dir(&config).unwrap();
        let vault = config.join("secrets.vault");
        fs::write(&vault, b"ENCRYPTED").unwrap();
        let denied = vec![vault.to_string_lossy().to_string()];

        let dest = root.path().join("relocated");
        let err = execute_move(&config, &dest, false, &denied, None)
            .await
            .expect_err("relocating the parent of a protected entry must be refused");
        assert!(
            err.to_string().contains("secrets.vault"),
            "refusal must name the protected location, got: {err}"
        );
        assert!(vault.exists(), "the protected leaf must stay put");
        assert!(!dest.exists(), "the tree must not be relocated");
    }

    /// One un-stat-able entry must not fail the whole listing — and the omission
    /// must be reported, never silent.
    ///
    /// A dangling symlink does NOT produce this shape: `DirEntry::metadata` is an
    /// `lstat` on Unix and succeeds on a dangling link. Dropping the directory's
    /// search bit while keeping its read bit does — `read_dir` still yields the
    /// names, `stat`ing any child returns EACCES.
    #[cfg(unix)]
    #[tokio::test]
    async fn list_skips_unstatable_entries_and_reports_the_skip() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempdir().unwrap();
        let dir = root.path().join("d");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("a.txt"), b"a").unwrap();
        fs::write(dir.join("b.txt"), b"b").unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o400)).unwrap();

        let restricted = fs::symlink_metadata(dir.join("a.txt")).is_err();
        if !restricted {
            // Running as root (or on a filesystem ignoring mode bits): an
            // un-stat-able entry cannot be constructed here.
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
            return;
        }

        let out = execute_list(&dir, &[], None, None).await;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();

        let out = out.expect("an un-stat-able entry must not fail the whole listing");
        assert!(out.success);
        assert!(
            out.message.contains("Skipped 2 unreadable entries"),
            "the listing must report what it dropped, got: {}",
            out.message
        );
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
