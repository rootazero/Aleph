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
    //
    // Walk via `tokio::fs` so a directory with many entries (e.g.
    // `node_modules/`, a build cache) does not stall a tokio worker — the
    // previous `std::fs::read_dir` + `entry.metadata()` pair blocked the
    // executor thread for the duration of every syscall.
    let mut skipped = 0usize;
    let mut rd = tokio::fs::read_dir(&canonical)
        .await
        .map_err(|e| ToolError::Execution(format!("Failed to read directory: {e}")))?;
    loop {
        let entry = match rd.next_entry().await {
            Ok(Some(entry)) => entry,
            Ok(None) => break,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };

        let metadata = match entry.metadata().await {
            Ok(m) => m,
            Err(_) => {
                skipped += 1;
                continue;
            }
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
            mtime: None,
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

    // `tokio::fs::metadata` so a multi-second stat on a slow / network
    // filesystem does not stall the executor thread; the previous
    // `canonical.exists()` + `is_file()` pair both made a blocking stat.
    let metadata = tokio::fs::metadata(&canonical)
        .await
        .map_err(|e| ToolError::Execution(format!("File not found: {}: {e}", path.display())))?;
    if !metadata.is_file() {
        return Err(ToolError::InvalidArgs(format!(
            "Not a file: {}",
            path.display()
        )));
    }
    let size = metadata.len();

    if size > max_read_size {
        return Err(ToolError::InvalidArgs(format!(
            "File too large: {size} bytes (max {max_read_size})"
        )));
    }

    let bytes = tokio::fs::read(&canonical)
        .await
        .map_err(|e| ToolError::Execution(format!("Failed to read file: {e}")))?;

    info!(path = %canonical.display(), size, "Read file");

    Ok((canonical, size, bytes))
}

/// What [`execute_write`] did to disk. The `bytes` figure is the full content
/// length either way (what would have been written) so callers can report a
/// uniform figure regardless of whether the call was a no-op.
pub(super) struct WriteOutcome {
    pub canonical: PathBuf,
    pub bytes: u64,
    /// `true` when the destination already had identical bytes and the atomic
    /// rename was therefore skipped. The file's `mtime` is preserved on
    /// `unchanged == true`; on `false` the rename overwrote it.
    pub unchanged: bool,
}

/// Execute a write operation.
///
/// Returns the canonicalized path written and the number of bytes written, so
/// callers can report the real resolved path rather than reverse-engineering it
/// from a human-readable message string.
///
/// **No-op short-circuit**: when the destination already exists and its
/// contents are byte-for-byte equal to `content`, the atomic rename is
/// skipped so the file's `mtime` is preserved. This matters for build
/// systems, file watchers, and incremental test runners that key on `mtime`:
/// a no-op write must not look like a new commit. The byte count returned
/// is still the full size of `content` (what would have been written) so
/// callers can report a uniform figure.
pub(super) async fn execute_write(
    path: &Path,
    content: &str,
    create_parents: bool,
    denied_paths: &[String],
    output_dir_override: Option<&std::path::Path>,
) -> Result<WriteOutcome, ToolError> {
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

    // No-op short-circuit: if the file already exists with the exact same
    // bytes, skip the atomic write entirely so mtime (and the file's
    // identity in any mtime-keyed downstream) is preserved. The compare
    // happens UNDER the path lock so a concurrent writer cannot change the
    // file between the read and the decision to skip.
    if is_byte_equal_existing(&canonical, content.as_bytes()).await {
        info!(path = %canonical.display(), bytes, "Wrote file (no-op, mtime preserved)");
        return Ok(WriteOutcome {
            canonical,
            bytes,
            unchanged: true,
        });
    }

    // Atomic write: staged temp file + rename, so a crash never leaves a
    // half-written file. Parent directories are already created above.
    crate::utils::atomic_write::atomic_write_file(&canonical, content)
        .await
        .map_err(|e| ToolError::Execution(format!("Failed to write file: {e}")))?;

    info!(path = %canonical.display(), bytes, "Wrote file");

    Ok(WriteOutcome {
        canonical,
        bytes,
        unchanged: false,
    })
}

/// Whether `canonical` exists and is byte-for-byte equal to `wanted`. Returns
/// `false` (i.e. "not a no-op, please write") when the file is absent or its
/// size differs — a cheap short-circuit that avoids loading the file when the
/// lengths already disagree.
async fn is_byte_equal_existing(canonical: &Path, wanted: &[u8]) -> bool {
    // BT-A-R4-06: open the file once and read exactly `wanted.len()`
    // bytes rather than two-syscall metadata() + read(). The previous
    // shape had a TOCTOU window: a concurrent writer could change the
    // file between the metadata() returning the right length and the
    // read() returning the actual bytes, and a small replace would
    // still byte-equal-length-match the wanted content. Reading
    // `wanted.len()` bytes from a single `File` handle closes the
    // window to a single syscall pair (open + read-of-N).
    use tokio::io::AsyncReadExt;
    let mut f = match tokio::fs::File::open(canonical).await {
        Ok(f) => f,
        Err(_) => return false,
    };
    let meta = match f.metadata().await {
        Ok(m) if m.is_file() => m,
        _ => return false,
    };
    if meta.len() as usize != wanted.len() {
        return false;
    }
    let mut existing = vec![0u8; wanted.len()];
    if f.read_exact(&mut existing).await.is_err() {
        return false;
    }
    existing == wanted
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

    // Create parent directories if needed. `tokio::fs::try_exists` + async
    // `create_dir_all` so the parent-mkdir does not stall a worker on a slow
    // filesystem; the previous `std::fs` pair blocked the executor for the
    // duration of every syscall.
    if create_parents {
        if let Some(parent) = to_canonical.parent() {
            match tokio::fs::try_exists(parent).await {
                Ok(false) => {
                    tokio::fs::create_dir_all(parent).await.map_err(|e| {
                        ToolError::Execution(format!("Failed to create directories: {e}"))
                    })?;
                }
                Ok(true) => {}
                Err(e) => {
                    return Err(ToolError::Execution(format!(
                        "Failed to stat destination parent: {e}"
                    )));
                }
            }
        }
    }

    tokio::fs::rename(&from_canonical, &to_canonical)
        .await
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

    // Create parent directories if needed. `tokio::fs::try_exists` +
    // async `create_dir_all` so the parent-mkdir does not stall a worker on a
    // slow filesystem.
    if create_parents {
        if let Some(parent) = to_canonical.parent() {
            match tokio::fs::try_exists(parent).await {
                Ok(false) => {
                    tokio::fs::create_dir_all(parent).await.map_err(|e| {
                        ToolError::Execution(format!("Failed to create directories: {e}"))
                    })?;
                }
                Ok(true) => {}
                Err(e) => {
                    return Err(ToolError::Execution(format!(
                        "Failed to stat destination parent: {e}"
                    )));
                }
            }
        }
    }

    let mut tally = CopyTally::default();
    if tokio::fs::metadata(&from_canonical)
        .await
        .map_err(|e| ToolError::Execution(format!("Failed to stat source: {e}")))?
        .is_file()
    {
        // File copy is small enough to do inline with `tokio::fs::copy`.
        tally.bytes = tokio::fs::copy(&from_canonical, &to_canonical)
            .await
            .map_err(|e| ToolError::Execution(format!("Failed to copy: {e}")))?;
    } else {
        // Directory copy is recursive — the tree walk calls into many blocking
        // syscalls (`read_dir`, `canonicalize`, `copy`, `create_dir_all`) on a
        // tree whose depth and breadth are caller-controlled. Run the whole
        // walk on the blocking pool so a multi-thousand-entry tree does not
        // monopolize a tokio worker.
        let from_canonical = from_canonical.clone();
        let to_canonical = to_canonical.clone();
        let denied_paths = denied_paths.to_vec();
        let mut visited = std::collections::HashSet::new();
        let tally_local: CopyTally = tokio::task::spawn_blocking(move || {
            copy_dir_recursive(
                &from_canonical,
                &to_canonical,
                &denied_paths,
                &mut visited,
            )
        })
        .await
        .map_err(|e| ToolError::Execution(format!("Copy task join failed: {e}")))??;
        tally = tally_local;
    }
    let bytes = tally.bytes;

    info!(
        from = %from_canonical.display(),
        to = %to_canonical.display(),
        bytes,
        protected_skipped = tally.protected,
        unresolvable_skipped = tally.unresolvable,
        "Copied"
    );

    // `success` stays true on a partial copy, and the message says PARTIAL.
    // Rationale for keeping the skip (rather than refusing outright the way
    // `move` and `delete` do for the same hazard): those two are destructive to
    // the SOURCE — a half-done move leaves the tree torn in two and a refused
    // delete is the only way to keep a protected file alive — whereas copy
    // leaves the source untouched, so the useful part of the work is worth
    // keeping as long as the omission is disclosed rather than silent. What was
    // wrong before was not the skipping; it was reporting an unconditional
    // "Copied X to Y (N bytes)" that a reader can only take as complete.
    Ok(FileOpsOutput {
        success: true,
        operation: "copy".to_string(),
        message: format!(
            "Copied {} to {} ({} bytes){}",
            from_canonical.display(),
            to_canonical.display(),
            bytes,
            tally.disclosure()
        ),
        files: tally.skipped_file_infos(),
        bytes_written: Some(bytes),
        items_affected: Some(1),
        summary: None,
    })
}

/// How many skipped paths a recursive copy names individually before it falls
/// back to counting. Bounded for the same reason the listing operations are:
/// an unbounded list of names is an unbounded tool result.
const MAX_NAMED_COPY_SKIPS: usize = 20;

/// What a recursive copy actually did.
///
/// The skip counts are separate because the two reasons are **not the same
/// claim**: `protected` means "the denylist says no", `unresolvable` means "I
/// could not find out" — a crash-boundary unknown, not a policy decision.
/// Folding them into one number would let a result assert something it does not
/// know.
#[derive(Default)]
struct CopyTally {
    /// Bytes actually copied.
    bytes: u64,
    /// Entries skipped because their canonical path is on the denylist.
    protected: usize,
    /// Entries skipped because they could not be resolved at all (removed
    /// mid-walk, EACCES): we do not know whether they were protected.
    unresolvable: usize,
    /// Source paths of the first [`MAX_NAMED_COPY_SKIPS`] skipped entries, so
    /// the result can name them and not only count them.
    named: Vec<PathBuf>,
}

impl CopyTally {
    fn note_protected(&mut self, path: &Path) {
        self.protected += 1;
        self.remember(path);
    }

    fn note_unresolvable(&mut self, path: &Path) {
        self.unresolvable += 1;
        self.remember(path);
    }

    fn remember(&mut self, path: &Path) {
        if self.named.len() < MAX_NAMED_COPY_SKIPS {
            self.named.push(path.to_path_buf());
        }
    }

    const fn skipped(&self) -> usize {
        self.protected + self.unresolvable
    }

    /// The clause appended to the tool message. Empty when nothing was skipped,
    /// so a complete copy reads exactly as it did before.
    fn disclosure(&self) -> String {
        let skipped = self.skipped();
        if skipped == 0 {
            return String::new();
        }
        let names = self
            .named
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let more = if skipped > self.named.len() {
            format!(" (first {} of {skipped})", self.named.len())
        } else {
            String::new()
        };
        format!(
            ". PARTIAL COPY: skipped {skipped} entries — {} protected by policy, \
             {} unresolvable (could not be checked, so neither copied nor cleared){more}: {names}. \
             The source is unchanged; `files` lists the skipped source paths.",
            self.protected, self.unresolvable
        )
    }

    /// The skipped source paths as structured rows. `FileOpsOutput` has no
    /// field of its own for this (adding one would break the `FileOpsOutput`
    /// literals in `batch.rs` / `search.rs` / `stats.rs`), so `files` carries
    /// them and the message says so. Metadata is best-effort `lstat`: an
    /// unresolvable entry has none by definition, and this must not read a
    /// protected entry's contents to describe it.
    fn skipped_file_infos(&self) -> Option<Vec<FileInfo>> {
        if self.named.is_empty() {
            return None;
        }
        Some(
            self.named
                .iter()
                .map(|path| {
                    let meta = fs::symlink_metadata(path).ok();
                    FileInfo {
                        name: path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default(),
                        path: path.to_string_lossy().to_string(),
                        is_dir: meta.as_ref().is_some_and(std::fs::Metadata::is_dir),
                        size: meta.as_ref().map_or(0, std::fs::Metadata::len),
                        extension: path.extension().map(|e| e.to_string_lossy().to_string()),
                        lines: None,
                        mtime: None,
                    }
                })
                .collect(),
        )
    }
}

/// Recursively copy a directory with symlink-cycle + deny guards.
///
/// Skips are accumulated into `tally` rather than swallowed: whatever this
/// function declines to copy, [`execute_copy`] names in its result.
fn copy_dir_recursive(
    from: &Path,
    to: &Path,
    denied_paths: &[String],
    visited: &mut std::collections::HashSet<PathBuf>,
) -> Result<CopyTally, ToolError> {
    let mut tally = CopyTally::default();
    fs::create_dir_all(to)
        .map_err(|e| ToolError::Execution(format!("Failed to create directory: {e}")))?;

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
            // this guard exists to stop. Recorded as UNRESOLVABLE, not
            // protected: "I could not check" is not "policy said no".
            Err(e) => {
                info!(
                    entry = %from_path.display(),
                    error = %e,
                    "copy: skipping entry that cannot be resolved"
                );
                tally.note_unresolvable(&from_path);
                continue;
            }
        };

        // Deny guard on EVERY entry, not just symlinks: a plain file or
        // directory whose own path is protected (`<config_dir>/data`, `~/.ssh`,
        // or a `[sandbox] deny_read_globs` match) is reached whenever the caller
        // copies its PARENT — which the denylist does not name, so
        // `check_and_resolve_path` waved the copy through and this is the only
        // point that sees the descendant. A symlink is the same hole reached
        // through its target. Checked before the cycle guard so a denied target
        // does not consume a `visited` slot.
        if path_is_denied(&canonical, denied_paths) {
            info!(
                entry = %from_path.display(),
                target = %canonical.display(),
                "copy: skipping protected entry"
            );
            tally.note_protected(&from_path);
            continue;
        }
        if is_symlink && !visited.insert(canonical.clone()) {
            return Err(ToolError::Execution(format!(
                "Symlink cycle detected at {}",
                from_path.display()
            )));
        }

        if from_path.is_dir() {
            // Reborrow `visited` for the recursive call so the cycle-check
            // borrow above is released before we hand the reference down.
            let sub_tally =
                copy_dir_recursive(&from_path, &to_path, denied_paths, &mut *visited)?;
            tally.bytes += sub_tally.bytes;
            tally.protected += sub_tally.protected;
            tally.unresolvable += sub_tally.unresolvable;
        } else {
            tally.bytes += fs::copy(&from_path, &to_path)
                .map_err(|e| ToolError::Execution(format!("Failed to copy file: {e}")))?;
        }
    }

    Ok(tally)
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
        let canonical = canonical.clone();
        tokio::task::spawn_blocking(move || {
            std::fs::remove_file(&canonical)
                .map_err(|e| ToolError::Execution(format!("Failed to delete symlink: {e}")))
        })
        .await
        .map_err(|e| ToolError::Execution(format!("Delete task join failed: {e}")))??;
        1
    } else if is_dir {
        // Count the whole tree before removal so the reported figure is the
        // true total, not just the directory's top-level entries. Both the
        // recursive count and `remove_dir_all` are blocking syscalls — run
        // them on the blocking pool so a deep tree does not monopolize a
        // tokio worker.
        let canonical_for_blocking = canonical.clone();
        let (count, ()) = tokio::task::spawn_blocking(move || {
            let count = count_path_entries(&canonical_for_blocking);
            std::fs::remove_dir_all(&canonical_for_blocking)
                .map_err(|e| ToolError::Execution(format!("Failed to delete directory: {e}")))?;
            Ok::<(usize, ()), ToolError>((count, ()))
        })
        .await
        .map_err(|e| ToolError::Execution(format!("Delete task join failed: {e}")))??;
        count
    } else {
        let canonical = canonical.clone();
        tokio::task::spawn_blocking(move || {
            std::fs::remove_file(&canonical)
                .map_err(|e| ToolError::Execution(format!("Failed to delete file: {e}")))
        })
        .await
        .map_err(|e| ToolError::Execution(format!("Delete task join failed: {e}")))??;
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

    // Async stat so a slow filesystem does not stall the executor. The two
    // branches (exists-dir / exists-not-dir / missing) match the prior sync
    // shape exactly.
    match tokio::fs::metadata(&canonical).await {
        Ok(md) => {
            if md.is_dir() {
                return Ok(FileOpsOutput {
                    success: true,
                    operation: "mkdir".to_string(),
                    message: format!("Directory already exists: {}", canonical.display()),
                    files: None,
                    bytes_written: None,
                    items_affected: Some(0),
                    summary: None,
                });
            }
            return Err(ToolError::InvalidArgs(format!(
                "Path exists but is not a directory: {}",
                path.display()
            )));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(ToolError::Execution(format!(
                "Failed to stat destination: {e}"
            )));
        }
    }

    if create_parents {
        tokio::fs::create_dir_all(&canonical)
            .await
            .map_err(|e| ToolError::Execution(format!("Failed to create directories: {e}")))?;
    } else {
        tokio::fs::create_dir(&canonical)
            .await
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

    /// RED before the fix: a recursive copy that skipped a protected entry
    /// still returned `success: true` with "Copied X to Y (N bytes)" and
    /// `files: None` — a caller could only read that as a complete copy. The
    /// skip has to be in the result the model reads, not only in a `tracing`
    /// line the model never sees.
    #[tokio::test]
    async fn copy_discloses_skipped_protected_entries() {
        let root = tempdir().unwrap();
        let src = root.path().join("aleph");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("ok.txt"), b"fine").unwrap();
        let secret_dir = src.join("data");
        fs::create_dir(&secret_dir).unwrap();
        fs::write(secret_dir.join("pairing.db"), b"DB").unwrap();
        let denied = vec![secret_dir.to_string_lossy().to_string()];

        let dst = root.path().join("backup");
        let out = execute_copy(&src, &dst, true, &denied, None).await.unwrap();

        assert!(out.success, "a partial copy still succeeds");
        assert!(
            out.message.contains("PARTIAL COPY"),
            "the omission must be named, got: {}",
            out.message
        );
        assert!(
            out.message.contains("1 protected"),
            "the protected count must be reported, got: {}",
            out.message
        );
        assert!(
            out.message.contains("0 unresolvable"),
            "the two skip reasons are separate claims, got: {}",
            out.message
        );
        assert!(
            out.message.contains("data"),
            "the skipped path must be named, got: {}",
            out.message
        );
        let skipped = out.files.expect("skipped entries must be structured too");
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].name, "data");
        assert!(dst.join("ok.txt").exists(), "the benign file still copies");
        assert!(!dst.join("data").exists());
    }

    /// A copy with nothing to skip must read exactly as it did before — the
    /// disclosure clause is empty and `files` stays absent.
    #[tokio::test]
    async fn copy_without_skips_says_nothing_extra() {
        let root = tempdir().unwrap();
        let src = root.path().join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("a.txt"), b"aaa").unwrap();
        let dst = root.path().join("dst");

        let out = execute_copy(&src, &dst, true, &[], None).await.unwrap();
        assert!(out.success);
        assert!(
            !out.message.contains("PARTIAL"),
            "a complete copy must not cry partial: {}",
            out.message
        );
        assert!(out.files.is_none(), "no skips, no skip list");
        assert_eq!(out.bytes_written, Some(3));
    }

    /// The second skip reason. An entry that cannot be resolved is NOT
    /// "protected" — we never found out — so it is counted and disclosed under
    /// its own name. Built by dropping the source directory's *search* bit
    /// while keeping its read bit: `read_dir` still yields the child names,
    /// while `canonicalize` on any child returns EACCES.
    #[cfg(unix)]
    #[tokio::test]
    async fn copy_discloses_unresolvable_entries_separately() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempdir().unwrap();
        let src = root.path().join("src");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("a.txt"), b"a").unwrap();
        fs::write(src.join("b.txt"), b"b").unwrap();
        let src_canonical = src.canonicalize().unwrap();
        fs::set_permissions(&src, fs::Permissions::from_mode(0o400)).unwrap();

        if fs::canonicalize(src.join("a.txt")).is_ok() {
            // Running as root (or a filesystem ignoring mode bits): the shape
            // cannot be constructed here.
            fs::set_permissions(&src, fs::Permissions::from_mode(0o700)).unwrap();
            return;
        }

        let dst = root.path().join("dst");
        // `src` is already canonical for the copy itself; only its children are
        // unreachable.
        let out = execute_copy(&src_canonical, &dst, true, &[], None).await;
        fs::set_permissions(&src, fs::Permissions::from_mode(0o700)).unwrap();

        let out = out.expect("an unresolvable entry must not fail the whole copy");
        assert!(out.success);
        assert!(
            out.message.contains("2 unresolvable"),
            "the unknown must be reported as unknown, not as protected: {}",
            out.message
        );
        assert!(
            out.message.contains("0 protected"),
            "an unresolvable entry must not be counted as a policy refusal: {}",
            out.message
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
        let outcome = execute_write(&path, "payload", true, &[], None)
            .await
            .unwrap();
        assert_eq!(outcome.bytes, 7);
        assert!(!outcome.unchanged, "first write must not be a no-op");
        assert_eq!(fs::read_to_string(&outcome.canonical).unwrap(), "payload");
    }

    /// No-op short-circuit: writing the same bytes to an existing file
    /// reports `unchanged: true` and leaves the file alone (its mtime is
    /// preserved so build systems and file watchers that key on mtime
    /// don't see a fresh change).
    #[tokio::test]
    async fn write_to_existing_file_with_same_bytes_is_a_noop() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("p.txt");
        fs::write(&path, "same").unwrap();
        let outcome = execute_write(&path, "same", false, &[], None)
            .await
            .unwrap();
        assert_eq!(outcome.bytes, 4);
        assert!(
            outcome.unchanged,
            "byte-equal rewrite must report unchanged"
        );
        assert_eq!(fs::read_to_string(&outcome.canonical).unwrap(), "same");
    }
}
