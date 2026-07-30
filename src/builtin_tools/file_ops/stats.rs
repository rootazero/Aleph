//! Recursive stats: per-file line/byte counts plus an aggregate summary.
//!
//! Replaces the "loop `file_read` N times then count lines" anti-pattern that
//! forced the LLM to make N round-trips just to answer "how many lines does
//! this directory contain?".

use std::path::Path;
use tokio::io::{AsyncBufReadExt, BufReader};
use tracing::{debug, info};

use super::path_utils::{check_and_resolve_path, reject_unsafe_glob_pattern};
use super::types::{
    is_skipped_dir_path, FileInfo, FileOpsOutput, StatsSummary, DEFAULT_ENTRY_LIMIT,
};
use crate::builtin_tools::error::ToolError;

/// Files larger than this are listed but not line-counted (treated as binary
/// or "too big to be worth scanning"). Mirrors the read tool's safety cap.
const MAX_LINE_COUNT_BYTES: u64 = 16 * 1024 * 1024; // 16 MB

/// Execute a recursive stats walk.
///
/// `pattern` is glob syntax relative to `dir` (e.g. `**/*.rs`); defaults to
/// `**/*` when not provided. Directories are skipped from line counting but
/// reported through `summary.total_files` only when they appear as concrete
/// files; directory entries themselves are filtered out of the result.
pub async fn execute_stats(
    dir: &Path,
    pattern: Option<&str>,
    denied_paths: &[String],
    output_dir_override: Option<&std::path::Path>,
    limit: Option<usize>,
) -> Result<FileOpsOutput, ToolError> {
    let canonical = check_and_resolve_path(dir, denied_paths, output_dir_override)?;

    if !canonical.exists() {
        return Err(ToolError::Execution(format!(
            "Path not found: {}",
            dir.display()
        )));
    }

    if !canonical.is_dir() {
        return Err(ToolError::InvalidArgs(format!(
            "Not a directory: {}",
            dir.display()
        )));
    }

    let glob_pattern = pattern.unwrap_or("**/*");
    // Same guard `search` applies: an absolute or `..`-climbing pattern would
    // replace/escape the deny-checked base via `join`, walking the whole
    // filesystem (and every denied credential path under it). Relative,
    // non-climbing patterns stay under `canonical`.
    reject_unsafe_glob_pattern(glob_pattern)?;
    let full_pattern = canonical.join(glob_pattern);
    let pattern_str = full_pattern.to_string_lossy();

    let cap = limit.unwrap_or(DEFAULT_ENTRY_LIMIT).max(1);
    let mut files: Vec<FileInfo> = Vec::new();
    let mut total_lines: u64 = 0;
    let mut total_bytes: u64 = 0;
    let mut skipped: usize = 0;
    let mut total_files: usize = 0;
    let mut skipped_generated: usize = 0;

    for entry in glob::glob(&pattern_str)
        .map_err(|e| ToolError::InvalidArgs(format!("Invalid glob pattern: {e}")))?
    {
        let path = match entry {
            Ok(p) => p,
            Err(e) => {
                debug!(error = %e, "Glob match error");
                continue;
            }
        };

        // Build/VCS directories are generated content the caller did not ask
        // about; walking them is what made `stats src` a 115k-token result whose
        // own aggregate answer then got replaced by a persist marker.
        if is_skipped_dir_path(&canonical, &path, glob_pattern) {
            skipped_generated += 1;
            continue;
        }

        // Defense in depth: even a relative pattern can match a symlink whose
        // target is a denied credential path (or escapes the base). Re-check
        // each match against the deny list; silently skip denied matches, just
        // as `search` does.
        if check_and_resolve_path(&path, denied_paths, output_dir_override).is_err() {
            continue;
        }

        let metadata = match tokio::fs::metadata(&path).await {
            Ok(m) => m,
            Err(_) => continue,
        };

        if metadata.is_dir() {
            continue;
        }

        let size = metadata.len();
        total_bytes = total_bytes.saturating_add(size);

        let lines = if size > MAX_LINE_COUNT_BYTES {
            skipped += 1;
            None
        } else {
            match count_lines(&path).await {
                Ok(n) => {
                    total_lines = total_lines.saturating_add(n);
                    Some(n)
                }
                Err(_) => {
                    skipped += 1;
                    None
                }
            }
        };

        // The aggregate always counts every file; only the per-file rows are
        // capped. Losing the four summary numbers to their own payload was the
        // failure mode — `stats` exists to answer "how many lines are in here",
        // and that answer must survive whatever the row budget does.
        total_files += 1;
        if files.len() < cap {
            files.push(FileInfo {
                name: path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
                path: path.to_string_lossy().to_string(),
                is_dir: false,
                size,
                extension: path.extension().map(|e| e.to_string_lossy().to_string()),
                lines,
            });
        }
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));

    let summary = StatsSummary {
        total_files,
        total_lines,
        total_bytes,
        skipped_files: skipped,
    };

    let message = format!(
        "Stats for {} (pattern={}): {} files, {} lines, {} bytes (skipped {}){}{}",
        canonical.display(),
        glob_pattern,
        total_files,
        total_lines,
        total_bytes,
        skipped,
        super::search::entry_cap_note(total_files, files.len(), cap),
        super::search::skipped_dirs_note(skipped_generated),
    );

    info!(
        path = %canonical.display(),
        pattern = glob_pattern,
        total_files,
        total_lines,
        total_bytes,
        skipped,
        "Stats completed"
    );

    Ok(FileOpsOutput {
        success: true,
        operation: "stats".to_string(),
        message,
        files: Some(files),
        bytes_written: None,
        items_affected: Some(total_files),
        summary: Some(summary),
    })
}

/// Count newline-terminated lines. Files without a trailing newline still
/// count their last line (matches `wc -l` semantics for non-empty files).
async fn count_lines(path: &Path) -> std::io::Result<u64> {
    let file = tokio::fs::File::open(path).await?;
    let reader = BufReader::new(file);
    let mut count: u64 = 0;
    let mut lines = reader.lines();
    while let Some(line) = lines.next_line().await? {
        // Surface I/O errors (e.g. invalid UTF-8 in a "text" file) so the
        // caller can mark the file as skipped instead of double-counting.
        let _ = line;
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn stats_counts_lines_recursively() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "one\ntwo\nthree\n").unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub/b.rs"), "hello\nworld\n").unwrap();
        fs::write(dir.path().join("sub/c.txt"), "ignored\n").unwrap();

        let out = execute_stats(dir.path(), Some("**/*.rs"), &[], None, None)
            .await
            .unwrap();

        assert!(out.success);
        let summary = out.summary.expect("summary populated");
        assert_eq!(summary.total_files, 2);
        assert_eq!(summary.total_lines, 5);
        assert_eq!(summary.skipped_files, 0);
    }

    #[tokio::test]
    async fn stats_rejects_escaping_pattern() {
        let dir = tempdir().unwrap();
        for bad in ["/etc/*", "../*", "../../**/*"] {
            let out = execute_stats(dir.path(), Some(bad), &[], None, None).await;
            assert!(
                matches!(out, Err(ToolError::InvalidArgs(_))),
                "escaping stats pattern {bad:?} must be rejected, got {out:?}"
            );
        }
    }

    #[tokio::test]
    async fn stats_default_pattern_includes_all_files() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a"), "x\ny\n").unwrap();
        fs::write(dir.path().join("b"), "z\n").unwrap();

        let out = execute_stats(dir.path(), None, &[], None, None)
            .await
            .unwrap();
        let summary = out.summary.expect("summary populated");
        assert_eq!(summary.total_files, 2);
        assert_eq!(summary.total_lines, 3);
    }

    /// The four aggregate numbers are the reason `stats` exists; they must be
    /// exact even when the per-file rows are capped. Losing them to their own
    /// payload (a 115k-token row array replaced wholesale by a persist marker)
    /// was the failure this cap prevents.
    #[tokio::test]
    async fn aggregate_is_exact_even_when_rows_are_capped() {
        let dir = tempdir().unwrap();
        for i in 0..30 {
            fs::write(dir.path().join(format!("f{i}.rs")), "a\nb\n").unwrap();
        }

        let out = execute_stats(dir.path(), Some("**/*.rs"), &[], None, Some(5))
            .await
            .unwrap();

        let summary = out.summary.expect("stats always reports an aggregate");
        assert_eq!(summary.total_files, 30, "every match is counted");
        assert_eq!(summary.total_lines, 60, "every match is line-counted");
        assert_eq!(
            out.files.as_ref().map(Vec::len),
            Some(5),
            "only the rows are capped"
        );
        assert!(
            out.message.contains("30 files") && out.message.contains("Showing 5 of 30"),
            "totals and the cap note must both be present; got: {}",
            out.message
        );
    }
}
