//! Batch file operations: `batch_move`, organize

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tracing::{debug, info};

use super::path_utils::{check_and_resolve_path, reject_unsafe_glob_pattern};
use super::types::{FileInfo, FileOpsOutput};
use crate::builtin_tools::error::ToolError;

/// A non-colliding destination path inside `dir` for `file_name`. Returns
/// `dir/file_name` when free, else inserts an incrementing ` (N)` suffix before
/// the extension (`report.txt` → `report (1).txt`) until an unused path is
/// found. Prevents `batch_move` from silently clobbering same-basename files
/// gathered from different subdirectories.
fn unique_dest(dir: &std::path::Path, file_name: &str) -> std::path::PathBuf {
    let candidate = dir.join(file_name);
    if !candidate.exists() {
        return candidate;
    }
    let p = std::path::Path::new(file_name);
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned());
    let ext = p.extension().map(|e| e.to_string_lossy().into_owned());
    // Bounded loop: filesystems cannot hold usize::MAX same-named files, and a
    // pathological directory falls back to the original (clobber) only after
    // exhausting the range — effectively never.
    for n in 1..usize::MAX {
        let name = match (&stem, &ext) {
            (Some(s), Some(e)) => format!("{s} ({n}).{e}"),
            (Some(s), None) => format!("{s} ({n})"),
            _ => format!("{file_name} ({n})"),
        };
        let candidate = dir.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(file_name)
}

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

    if !canonical.is_dir() {
        return Err(ToolError::InvalidArgs(format!(
            "Source path is not a directory: {}",
            dir.display()
        )));
    }

    reject_unsafe_glob_pattern(pattern)?;

    // Security: check destination path BEFORE creating it, to prevent writing
    // into denied directories (e.g., ~/.ssh with create_parents=true). Resolve
    // it up front and test THAT for existence: `dest.exists()` answered for the
    // literal argument, so a `~/...` (or run-relative) destination that was
    // right there read as missing and the whole call was refused. Both this and
    // the creation happen after the checks above, so a rejected call no longer
    // leaves a directory behind.
    let checked_dest = check_and_resolve_path(dest, denied_paths, output_dir_override)?;
    let dest_canonical = if checked_dest.exists() {
        checked_dest
    } else if create_parents {
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
                    let Ok(src_canonical) =
                        check_and_resolve_path(&path, denied_paths, output_dir_override)
                    else {
                        errors.push(format!("{}: denied by policy", path.display()));
                        continue;
                    };
                    let file_name = path.file_name().unwrap_or_default();
                    // Sanitize filename to prevent path traversal via "../" in names
                    let safe_name = file_name.to_string_lossy().replace(['/', '\\'], "_");
                    // A `**/*` pattern can match same-basename files in different
                    // subdirectories; a plain `join` would make later matches
                    // clobber earlier ones (silent data loss). De-duplicate the
                    // destination so every source file survives.
                    let dest_path = unique_dest(&dest_canonical, &safe_name);

                    // Cross-agent serialization per item (sorted pair — same
                    // guard `execute_move` takes; released each iteration).
                    let _path_guards =
                        crate::tools::path_locks::lock_path_pair(&src_canonical, &dest_path).await;

                    match fs::rename(&path, &dest_path) {
                        Ok(_) => {
                            let size = fs::metadata(&dest_path).map_or(0, |m| m.len());
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

        // Per-entry deny re-check (defense in depth, matching `search` /
        // `batch_move` / `stats`): a symlink whose target is a protected
        // credential path — or any entry resolving under the denylist — must not
        // be relocated out from under the blacklist. Silently skip denied entries.
        if check_and_resolve_path(&path, denied_paths, output_dir_override).is_err() {
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
            .map_or("Others", |(name, _)| *name);

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
        // A category folder accumulates across runs, so a `photo.png` arriving
        // a week after the first one would `fs::rename` straight over the file
        // an earlier organize had already filed there — silent data loss, still
        // reported as success. De-duplicate exactly as `batch_move` does.
        let dest_path = unique_dest(&category_dir, &file_name.to_string_lossy());

        // Cross-agent serialization per item (sorted pair — same guard
        // `execute_move` takes; released each iteration). `path` comes from
        // `read_dir` on the already-canonicalized directory, so it is a
        // stable lock key.
        let _path_guards = crate::tools::path_locks::lock_path_pair(&path, &dest_path).await;

        match fs::rename(&path, &dest_path) {
            Ok(_) => {
                *category_counts.entry(category.to_string()).or_insert(0) += 1;
                let size = fs::metadata(&dest_path).map_or(0, |m| m.len());
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

    #[tokio::test]
    async fn batch_move_dedups_same_basename_collisions() {
        // Two files with the SAME basename in different subdirectories must both
        // survive the move — historically the second silently clobbered the first.
        let src = tempdir().unwrap();
        let dst = tempdir().unwrap();
        fs::create_dir(src.path().join("a")).unwrap();
        fs::create_dir(src.path().join("b")).unwrap();
        fs::write(src.path().join("a/report.log"), b"AAAA").unwrap();
        fs::write(src.path().join("b/report.log"), b"BBBBBB").unwrap();

        let out = execute_batch_move(src.path(), "**/report.log", dst.path(), false, &[], None)
            .await
            .unwrap();
        assert!(out.success, "message: {}", out.message);
        assert_eq!(out.items_affected, Some(2), "both files must move");
        // Original name + one de-duplicated name, both present, no data lost.
        assert!(dst.path().join("report.log").exists());
        assert!(dst.path().join("report (1).log").exists());
    }

    #[tokio::test]
    async fn organize_dedups_against_an_already_filed_same_name_file() {
        // A category folder accumulates across runs: a `photo.png` that arrives
        // a week after the first one used to be renamed straight over the file
        // the earlier organize had already filed, and the result still said
        // `success: true`.
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("photo.png"), b"first").unwrap();
        execute_organize(dir.path(), &[], None).await.unwrap();

        fs::write(dir.path().join("photo.png"), b"second-arrival").unwrap();
        let out = execute_organize(dir.path(), &[], None).await.unwrap();

        assert!(out.success, "message: {}", out.message);
        assert_eq!(
            fs::read(dir.path().join("Images/photo.png")).unwrap(),
            b"first",
            "the already-filed file must survive"
        );
        assert_eq!(
            fs::read(dir.path().join("Images/photo (1).png")).unwrap(),
            b"second-arrival",
            "the newcomer must land beside it, not on top of it"
        );
    }

    #[tokio::test]
    async fn batch_move_rejected_pattern_leaves_no_destination_behind() {
        // The destination was created before the pattern was validated, so a
        // call that got refused still littered a directory on disk.
        let src = tempdir().unwrap();
        let holder = tempdir().unwrap();
        let dest = holder.path().join("made-by-a-refused-call");

        let out = execute_batch_move(src.path(), "/etc/*", &dest, true, &[], None).await;
        assert!(
            matches!(out, Err(ToolError::InvalidArgs(_))),
            "absolute glob pattern must be rejected, got {out:?}"
        );
        assert!(
            !dest.exists(),
            "a refused call must not leave its destination behind"
        );
    }

    #[tokio::test]
    async fn batch_move_tests_existence_of_the_expanded_destination() {
        // Existence was tested on the RAW argument, so a destination spelled
        // `~/...` — or relative to the run's base, as here — was reported
        // missing even when it was right there, and `create_parents: false`
        // refused the whole call.
        let src = tempdir().unwrap();
        let base = tempdir().unwrap();
        fs::write(src.path().join("a.log"), b"hello-log").unwrap();
        fs::create_dir(base.path().join("inbox")).unwrap();

        let out = execute_batch_move(
            src.path(),
            "*.log",
            Path::new("inbox"),
            false,
            &[],
            Some(base.path()),
        )
        .await
        .unwrap();

        assert!(out.success, "message: {}", out.message);
        assert!(base.path().join("inbox/a.log").is_file());
    }

    #[tokio::test]
    async fn batch_move_absolute_pattern_is_rejected() {
        let dir = std::env::temp_dir();
        let dest = std::env::temp_dir().join("aleph_batch_dest");
        let out = execute_batch_move(&dir, "/etc/*", &dest, true, &[], None).await;
        assert!(
            matches!(out, Err(ToolError::InvalidArgs(_))),
            "absolute glob pattern must be rejected, got {out:?}"
        );
    }
}
