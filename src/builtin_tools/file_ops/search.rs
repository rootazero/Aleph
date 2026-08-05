//! Search operations for file system

use std::fs;
use std::path::Path;
use tracing::{debug, info};

use super::path_utils::{check_and_resolve_path, reject_unsafe_glob_pattern};
use super::types::{
    is_skipped_dir_path, FileInfo, FileOpsOutput, DEFAULT_ENTRY_LIMIT, SKIPPED_DIRS,
};
use crate::builtin_tools::error::ToolError;

/// Execute a search operation
pub async fn execute_search(
    dir: &Path,
    pattern: &str,
    denied_paths: &[String],
    output_dir_override: Option<&std::path::Path>,
    limit: Option<usize>,
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

    let cap = limit.unwrap_or(DEFAULT_ENTRY_LIMIT).max(1);
    let mut files = Vec::new();
    let mut matched = 0usize;
    let mut skipped_generated = 0usize;

    for entry in glob::glob(&pattern_str)
        .map_err(|e| ToolError::InvalidArgs(format!("Invalid glob pattern: {e}")))?
    {
        match entry {
            Ok(path) => {
                // Build/VCS directories would otherwise consume the whole entry
                // budget with generated files the caller did not ask about.
                if is_skipped_dir_path(&canonical, &path, pattern) {
                    skipped_generated += 1;
                    continue;
                }
                // Defense in depth: even a relative pattern can match a symlink
                // pointing outside the base. Re-check each match against the
                // deny list; silently skip denied matches.
                if check_and_resolve_path(&path, denied_paths, output_dir_override).is_err() {
                    continue;
                }
                if let Ok(metadata) = fs::metadata(&path) {
                    matched += 1;
                    if files.len() < cap {
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
                            mtime: None,
                        });
                    }
                }
            }
            Err(e) => {
                debug!(error = %e, "Glob match error");
            }
        }
    }

    let count = files.len();
    info!(pattern, count, matched, "Search completed");

    Ok(FileOpsOutput {
        success: true,
        operation: "search".to_string(),
        message: format!(
            "Found {matched} files matching '{pattern}' in {}{}{}",
            canonical.display(),
            entry_cap_note(matched, count, cap),
            skipped_dirs_note(skipped_generated),
        ),
        files: Some(files),
        bytes_written: None,
        items_affected: Some(matched),
        summary: None,
    })
}

/// Actionable note appended when the entry cap bound the result. Mirrors pi's
/// phrasing (`find.ts`) — telling the model the exact lever beats telling it
/// only that something was dropped.
pub(super) fn entry_cap_note(matched: usize, shown: usize, cap: usize) -> String {
    entry_cap_note_with(matched, shown, cap, ", or narrow the pattern")
}

/// [`entry_cap_note`] with caller-supplied follow-up advice.
///
/// `list` takes no `pattern`, so telling its caller to narrow one named a lever
/// that does not exist for that operation — the sort of instruction a model will
/// dutifully try and get an argument error for.
pub(super) fn entry_cap_note_with(
    matched: usize,
    shown: usize,
    cap: usize,
    advice: &str,
) -> String {
    if matched <= shown {
        return String::new();
    }
    format!(
        ". Showing {shown} of {matched} — {cap}-entry limit reached; pass limit={}{advice}",
        cap.saturating_mul(2)
    )
}

/// Note appended when generated/VCS directories were skipped, so the omission is
/// never silent.
pub(super) fn skipped_dirs_note(skipped: usize) -> String {
    if skipped == 0 {
        return String::new();
    }
    format!(
        ". Skipped {skipped} entries under build/VCS directories ({}); \
         name one in `pattern` to include it",
        SKIPPED_DIRS.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn absolute_pattern_is_rejected() {
        let dir = std::env::temp_dir();
        let out = execute_search(&dir, "/etc/*", &[], None, None).await;
        assert!(
            matches!(out, Err(ToolError::InvalidArgs(_))),
            "absolute glob pattern must be rejected, got {out:?}"
        );
    }

    #[tokio::test]
    async fn parent_escape_pattern_is_rejected() {
        let dir = std::env::temp_dir();
        let out = execute_search(&dir, "../*", &[], None, None).await;
        assert!(
            matches!(out, Err(ToolError::InvalidArgs(_))),
            "`..` glob pattern must be rejected, got {out:?}"
        );
    }

    /// The cap bounds what is *returned*, never what is *counted*, and it always
    /// says so — a silently short list reads as a complete answer.
    #[tokio::test]
    async fn entry_cap_bounds_rows_but_not_the_count() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..40 {
            std::fs::write(dir.path().join(format!("f{i}.txt")), "x").unwrap();
        }

        let out = execute_search(dir.path(), "*.txt", &[], None, Some(10))
            .await
            .unwrap();
        assert_eq!(
            out.files.as_ref().map(Vec::len),
            Some(10),
            "rows are capped at the requested limit"
        );
        assert_eq!(
            out.items_affected,
            Some(40),
            "the total must stay exact, capped rows or not"
        );
        assert!(
            out.message.contains("Showing 10 of 40") && out.message.contains("limit=20"),
            "the cap must be announced with the lever to raise it; got: {}",
            out.message
        );

        // Uncapped: no note at all, so the common case reads clean.
        let all = execute_search(dir.path(), "*.txt", &[], None, None)
            .await
            .unwrap();
        assert_eq!(all.files.as_ref().map(Vec::len), Some(40));
        assert!(
            !all.message.contains("limit reached"),
            "no cap note when nothing was dropped; got: {}",
            all.message
        );
    }

    /// Generated directories are skipped by default, opt back in by naming them.
    #[tokio::test]
    async fn build_dirs_are_skipped_unless_the_pattern_names_them() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("target/debug")).unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.path().join("target/debug/gen.rs"), "// generated").unwrap();

        let out = execute_search(dir.path(), "**/*.rs", &[], None, None)
            .await
            .unwrap();
        let paths: Vec<&str> = out
            .files
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|f| f.path.as_str())
            .collect();
        assert!(
            paths.iter().any(|p| p.ends_with("main.rs")),
            "source must be found: {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.contains("target")),
            "target/ must be skipped: {paths:?}"
        );
        assert!(
            out.message.contains("Skipped 1 entries under build/VCS"),
            "the skip must be announced; got: {}",
            out.message
        );

        // Naming the directory in the pattern opts back in.
        let opted = execute_search(dir.path(), "target/**/*.rs", &[], None, None)
            .await
            .unwrap();
        assert_eq!(
            opted.items_affected,
            Some(1),
            "naming target/ in the pattern must reach it: {}",
            opted.message
        );
    }
}
