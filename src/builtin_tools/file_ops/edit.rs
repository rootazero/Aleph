//! `FileEditTool` — string replacement editing tool
//!
//! Performs string replacements in files, aligned with claude-code's
//! `FileEditTool`. Matching is exact first; on a miss it falls back to folding
//! typographic punctuation (see [`super::edit_match`]) and, failing that,
//! produces a diagnostic that tells the model exactly how to fix its input.
//! Binary and non-UTF-8 files are refused outright — editing them would corrupt
//! their non-text bytes on write-back.

use std::path::Path;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use super::edit_match::{apply_ranges, locate, LocateResult};
use super::path_utils::{check_and_resolve_path, get_denied_paths};
use super::text::{clamp_line, is_binary};
use crate::builtin_tools::error::ToolError;
use crate::error::Result;
use crate::tools::AlephTool;

/// Read a file as UTF-8 text, refusing binary or non-UTF-8 content.
///
/// `file_edit` must round-trip the file faithfully, so — unlike `file_read` —
/// it cannot use lossy decoding: a lossy decode followed by write-back would
/// permanently replace every non-UTF-8 byte with U+FFFD.
async fn read_text_file(path: &Path) -> std::result::Result<String, ToolError> {
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| ToolError::Execution(format!("Failed to read {}: {}", path.display(), e)))?;
    if is_binary(&bytes) {
        return Err(ToolError::InvalidArgs(format!(
            "Cannot edit {}: file appears to be binary.",
            path.display()
        )));
    }
    String::from_utf8(bytes).map_err(|_| {
        ToolError::InvalidArgs(format!(
            "Cannot edit {}: file is not valid UTF-8 text.",
            path.display()
        ))
    })
}

/// Context lines shown on each side of the replacement in the result snippet.
const SNIPPET_CONTEXT_LINES: usize = 2;
/// Upper bound on snippet length; a larger window is elided in the middle.
const SNIPPET_MAX_LINES: usize = 20;

/// Render a `cat -n`-style excerpt of `new_content` around the first
/// replacement, mirroring `file_read`'s format so line numbers line up with a
/// later read.
///
/// `first_start` is the byte offset of the first applied range in the
/// *pre-edit* content; the splice leaves everything before it unchanged, so it
/// is also a valid offset (and char boundary) in `new_content`, and the line
/// index it implies is the first edited line.
fn render_edit_snippet(new_content: &str, first_start: usize, replacement: &str) -> String {
    let lines: Vec<&str> = new_content.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    let edit_line = new_content[..first_start].matches('\n').count();
    let span = replacement.matches('\n').count() + 1;
    let start = edit_line.saturating_sub(SNIPPET_CONTEXT_LINES);
    let end = (edit_line + span + SNIPPET_CONTEXT_LINES).min(lines.len());
    let width = end.to_string().len();

    let push = |out: &mut String, idx: usize| {
        let lineno = idx + 1;
        let line = lines
            .get(idx)
            .expect("invariant: idx is within the rendered snippet range");
        out.push_str(&format!("{lineno:>width$}\t{}\n", clamp_line(line)));
    };

    let mut out = String::new();
    if end - start > SNIPPET_MAX_LINES {
        // Huge replacement: show the head and tail of the window, elide the middle.
        let head_end = start + SNIPPET_MAX_LINES / 2;
        let tail_start = end - SNIPPET_MAX_LINES / 2;
        for idx in start..head_end {
            push(&mut out, idx);
        }
        out.push_str(&format!(
            "{:>width$}\t… [{} lines elided] …\n",
            "",
            tail_start - head_end
        ));
        for idx in tail_start..end {
            push(&mut out, idx);
        }
    } else {
        for idx in start..end {
            push(&mut out, idx);
        }
    }
    out
}

/// Apply a set of **non-overlapping** replacements in `content`, each at the
/// given byte range with its own replacement string. Ranges are spliced
/// back-to-front so earlier edits never shift the offsets of later ones, and
/// every range lies on a UTF-8 char boundary (we are only splicing ASCII
/// substitutions back into the file, so the chars-touched assertion holds by
/// construction — multi-byte characters either stay inside the un-spliced
/// regions or are replaced wholesale with whatever the model supplied).
///
/// Caller's contract: ranges are non-overlapping and ascending by `start`.
/// That is checked by the multi-edit gate (overlap detector) before this
/// function is reached.
fn apply_distinct_replacements(
    content: &str,
    resolved: &[(usize, usize, std::borrow::Cow<'_, str>, bool, bool)],
) -> String {
    // Index slice in descending start order. The `fuzzy`/`crlf` flags are
    // informational (already used in the message) and intentionally not
    // consumed here.
    let mut order: Vec<usize> = (0..resolved.len()).collect();
    order.sort_by(|&a, &b| resolved[b].0.cmp(&resolved[a].0));

    let mut result = content.to_string();
    for i in order {
        let (start, end, ref replacement, _, _) = resolved[i];
        result.replace_range(start..end, replacement.as_ref());
    }
    result
}

// =============================================================================
// Args & Output
// =============================================================================

/// One targeted replacement inside a `file_edit` call.
///
/// Used in the `edits: [...]` array form (multi-edit) and as the per-element
/// shape of the array. Fields mirror the legacy single-edit `old_string` /
/// `new_string` so a model that learned the old shape can still produce a
/// correct `edits` array without translation.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct EditOp {
    /// The exact text to find. Must be unique within the file unless the
    /// call's top-level `replace_all` is also set.
    pub old_string: String,
    /// The replacement text.
    pub new_string: String,
}

/// Arguments for the `file_edit` tool.
///
/// The schema accepts two shapes:
///
/// 1. **Legacy single edit** (kept for backwards-compat): `old_string` /
///    `new_string` / `replace_all`. A model that sends only the legacy fields
///    gets a single-target edit; this is the form most prompts/skills learned.
/// 2. **Multi edit**: a non-empty `edits: [{old_string, new_string}, ...]`
///    array. All edits are applied atomically against the *original* file
///    (each `old_string` is matched against the pre-edit content, not against
///    an incrementally-mutated file), and the whole call is rejected if any
///    individual edit's `old_string` matches in more than one place, or if
///    two edits' ranges overlap.
///
/// When both shapes are present, `edits` wins (the legacy fields are ignored).
/// `replace_all` only applies to the legacy single-edit shape.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FileEditArgs {
    /// Absolute or relative path to the file to edit
    pub file_path: String,
    /// The exact string to find in the file. Ignored when `edits` is set.
    #[serde(default)]
    pub old_string: String,
    /// The replacement string. Ignored when `edits` is set.
    #[serde(default)]
    pub new_string: String,
    /// Replace all occurrences (default: false — single match only). Ignored
    /// when `edits` is set; multi-edit requires each `old_string` to be unique.
    #[serde(default)]
    pub replace_all: bool,
    /// One or more targeted replacements, each matched against the original
    /// file. Overlapping or non-unique entries are rejected. Takes precedence
    /// over the legacy `old_string` / `new_string` fields when non-empty.
    #[serde(default)]
    pub edits: Vec<EditOp>,
}

/// Output from the `file_edit` tool
#[derive(Debug, Clone, Serialize)]
pub struct FileEditOutput {
    /// Whether the edit succeeded
    pub success: bool,
    /// Resolved canonical path of the edited file
    pub path: String,
    /// Number of replacements performed
    pub replacements: usize,
    /// Human-readable result message
    pub message: String,
    /// Line-numbered excerpt of the file around the first replacement, as it
    /// reads *after* the edit. Lets the model verify the result in place
    /// instead of spending a follow-up `file_read` on it.
    pub snippet: String,
}

// =============================================================================
// Tool struct
// =============================================================================

/// String-replacement file editing tool
pub struct FileEditTool {
    /// Denied path patterns (security)
    denied_paths: Vec<String>,
    /// Optional `ToolContext` handle for workspace-scoped output path resolution
    tool_context_handle: Option<crate::tools::ToolContextHandle>,
}

impl FileEditTool {
    /// Create a new `FileEditTool` with default denied paths
    pub fn new() -> Self {
        let denied_paths = get_denied_paths();
        info!(
            denied_paths_count = denied_paths.len(),
            "FileEditTool: initialized"
        );
        Self {
            denied_paths,
            tool_context_handle: None,
        }
    }

    /// Configure the tool to use a `ToolContext` handle for workspace-scoped output paths
    pub fn with_tool_context(mut self, handle: crate::tools::ToolContextHandle) -> Self {
        self.tool_context_handle = Some(handle);
        self
    }

    /// Resolve the output directory from the `ToolContext` handle (if available).
    async fn resolve_output_dir(&self) -> Option<std::path::PathBuf> {
        if let Some(ref handle) = self.tool_context_handle {
            let ctx = handle.read().await;
            Some(ctx.output_dir.join("documents"))
        } else {
            None
        }
    }

    /// Internal implementation
    async fn call_impl(
        &self,
        args: FileEditArgs,
    ) -> std::result::Result<FileEditOutput, ToolError> {
        use crate::builtin_tools::{notify_tool_result, notify_tool_start};

        // Notify start
        let summary = format!("edit: {}", &args.file_path);
        notify_tool_start("file_edit", &summary);

        // Resolve & validate path (cheap, do it up front so a denied path is
        // refused before we start reading content).
        let output_dir = self.resolve_output_dir().await;
        let output_dir_ref = output_dir.as_deref();
        let canonical = check_and_resolve_path(
            Path::new(&args.file_path),
            &self.denied_paths,
            output_dir_ref,
        )?;

        info!(path = %canonical.display(), "FileEditTool: reading file");

        // Cross-agent write guard: the read → locate → apply → write sequence
        // below is a lost-update window when another harness (parent agent,
        // concurrent subagent, team member sharing the workspace) edits the
        // same file — the atomic rename only prevents torn writes. Hold the
        // process-wide per-path lock for the whole critical section.
        let _path_guard = crate::tools::path_locks::lock_path(&canonical).await;

        // Read current content — binary / non-UTF-8 files are refused.
        let content = read_text_file(&canonical).await.inspect_err(|e| {
            notify_tool_result("file_edit", &e.to_string(), false);
        })?;

        // Normalise the request: the `edits: [...]` array (if non-empty) wins
        // over the legacy single-edit fields. Each normalised op carries the
        // resolved (old, new) pair exactly as it should be applied.
        let ops: Vec<EditOp> = if !args.edits.is_empty() {
            args.edits.clone()
        } else {
            vec![EditOp {
                old_string: args.old_string.clone(),
                new_string: args.new_string.clone(),
            }]
        };

        // Validate the normalised op set. Both shapes feed through the same
        // gate so the model gets the same error contract regardless of which
        // form it sent.
        for (i, op) in ops.iter().enumerate() {
            if op.old_string.is_empty() {
                let err = ToolError::InvalidArgs(format!(
                    "edits[{i}].old_string must not be empty"
                ));
                notify_tool_result("file_edit", &err.to_string(), false);
                return Err(err);
            }
            if op.old_string == op.new_string {
                let err = ToolError::InvalidArgs(format!(
                    "edits[{i}]: old_string and new_string are identical; nothing to change"
                ));
                notify_tool_result("file_edit", &err.to_string(), false);
                return Err(err);
            }
        }

        // Multi-edit branch: locate every op in one pass, refuse overlapping
        // or non-unique matches, then apply the splices in descending order so
        // earlier ops never shift the offsets of later ones. Each op is
        // matched against the ORIGINAL file (the pre-edit content), so a
        // later splice can never silently make an earlier one disappear.
        if ops.len() > 1 {
            return self.apply_multi_edit(&canonical, &content, &ops).await;
        }

        // Single-edit branch (the legacy fast path).
        let op = &ops[0];
        let (ranges, fuzzy, crlf) = match locate(&content, &op.old_string) {
            LocateResult::Exact(r) => (r, false, false),
            LocateResult::Folded(r) => (r, true, false),
            LocateResult::Crlf(r) => (r, false, true),
            LocateResult::NotFound(diagnostic) => {
                let err = ToolError::Execution(diagnostic);
                notify_tool_result("file_edit", &err.to_string(), false);
                return Err(err);
            }
        };

        if ranges.len() > 1 && !args.replace_all {
            let err = ToolError::Execution(format!(
                "Found {} matches of old_string; provide more context to make it unique or set replace_all=true",
                ranges.len()
            ));
            notify_tool_result("file_edit", &err.to_string(), false);
            return Err(err);
        }

        // When the match was bridged by CRLF expansion, the replacement's LF
        // newlines must be expanded the same way, or the edit would splice
        // LF lines into a CRLF file (mixed line endings).
        let replacement = if crlf && !op.new_string.contains('\r') {
            std::borrow::Cow::Owned(op.new_string.replace('\n', "\r\n"))
        } else {
            std::borrow::Cow::Borrowed(op.new_string.as_str())
        };

        // `ranges` is non-empty here; apply all under `replace_all`, else the first.
        let applied = if args.replace_all {
            &ranges[..]
        } else {
            ranges
                .get(..1)
                .expect("invariant: ranges is non-empty when replace_all is false")
        };
        let replacements = applied.len();
        let new_content = apply_ranges(&content, applied, &replacement);

        // Write back atomically: stage to a temp file in the same directory,
        // fsync, then rename. A crash mid-write must never leave the user's
        // existing file truncated. File permissions are preserved.
        crate::utils::atomic_write::atomic_write_file(&canonical, &new_content)
            .await
            .map_err(|e| {
                ToolError::Execution(format!("Failed to write {}: {}", canonical.display(), e))
            })?;

        let path_str = canonical.to_string_lossy().to_string();
        let message = format!(
            "Replaced {} occurrence{} in {}{}",
            replacements,
            if replacements == 1 { "" } else { "s" },
            path_str,
            if fuzzy {
                " (matched after normalizing typographic punctuation)"
            } else if crlf {
                " (matched after normalizing line endings; replacement written with CRLF)"
            } else {
                ""
            },
        );
        let snippet = render_edit_snippet(
            &new_content,
            applied
                .first()
                .expect("invariant: at least one range was applied")
                .0,
            &replacement,
        );

        info!(replacements, fuzzy, crlf, path = %path_str, "FileEditTool: edit complete");
        notify_tool_result("file_edit", &message, true);

        Ok(FileEditOutput {
            success: true,
            path: path_str,
            replacements,
            message,
            snippet,
        })
    }

    /// Apply a multi-edit request in one atomic write. All `ops` are matched
    /// against the *original* `content`; if any op's `old_string` cannot be
    /// found uniquely the whole call is refused (no partial write), and if
    /// two ops' resolved ranges overlap the call is refused with the index of
    /// the offender so the model can fix the pair. Splices are applied
    /// back-to-front so earlier ops never shift the offsets of later ones.
    async fn apply_multi_edit(
        &self,
        canonical: &Path,
        content: &str,
        ops: &[EditOp],
    ) -> std::result::Result<FileEditOutput, ToolError> {
        use crate::builtin_tools::notify_tool_result;

        // Step 1 — locate every op against the original content. Each entry
        // is `(byte_start, byte_end, replacement_text, was_fuzzy, was_crlf)`
        // — the range is over the *original* content; the replacement has
        // been CRLF-expanded if a CRLF-bridged match was used.
        let mut resolved: Vec<(usize, usize, std::borrow::Cow<'_, str>, bool, bool)> =
            Vec::with_capacity(ops.len());
        for (i, op) in ops.iter().enumerate() {
            let (ranges, fuzzy, crlf) = match locate(content, &op.old_string) {
                LocateResult::Exact(r) => (r, false, false),
                LocateResult::Folded(r) => (r, true, false),
                LocateResult::Crlf(r) => (r, false, true),
                LocateResult::NotFound(diagnostic) => {
                    let err = ToolError::Execution(format!(
                        "edits[{i}] did not match: {diagnostic}"
                    ));
                    notify_tool_result("file_edit", &err.to_string(), false);
                    return Err(err);
                }
            };
            if ranges.len() > 1 {
                let err = ToolError::Execution(format!(
                    "edits[{i}] matched {} places in the file; multi-edit requires \
                     each old_string to be unique. Add more surrounding context \
                     to make this one different from its sibling matches.",
                    ranges.len()
                ));
                notify_tool_result("file_edit", &err.to_string(), false);
                return Err(err);
            }
            let (start, end) = ranges[0];
            let replacement = if crlf && !op.new_string.contains('\r') {
                std::borrow::Cow::Owned(op.new_string.replace('\n', "\r\n"))
            } else {
                std::borrow::Cow::Borrowed(op.new_string.as_str())
            };
            resolved.push((start, end, replacement, fuzzy, crlf));
        }

        // Step 2 — refuse overlapping ranges. Two edits to the same byte
        // range (or to nested ranges) is a request the model should resolve
        // upstream by either merging them into one edit or by giving them
        // disjoint context; silently keeping one and dropping the other is
        // the kind of "edit worked but the file is now wrong" failure this
        // tool is explicitly trying to prevent.
        let mut by_start: Vec<usize> = (0..resolved.len()).collect();
        by_start.sort_by_key(|&i| resolved[i].0);
        for win in by_start.windows(2) {
            let prev = win[0];
            let next = win[1];
            // `resolved[i]` is `(start, end, replacement, fuzzy, crlf)`; the
            // naming below is what makes the overlap test actually work — the
            // previous version destructured `prev_end` from position 0 (i.e.
            // the START), which made the comparison degenerate to
            // `next_start < prev_start` and never fired on start ties.
            let (prev_start, prev_end, _, _, _) = resolved[prev];
            let (next_start, _, _, _, _) = resolved[next];
            if next_start < prev_end {
                let err = ToolError::Execution(format!(
                    "edits[{prev}] and edits[{next}] overlap in the file (edits[{prev}] \
                     spans {prev_start}..{prev_end}, edits[{next}] starts at {next_start}); \
                     multi-edit requires every edit to target a non-overlapping region. \
                     Either merge them into one edit or expand the context of each so the \
                     regions become disjoint."
                ));
                notify_tool_result("file_edit", &err.to_string(), false);
                return Err(err);
            }
        }

        // Step 3 — apply in descending start order. `apply_ranges` already
        // takes a slice of `(start, end)` and splices back-to-front, so
        // collecting just the ranges is enough; the replacement text is
        // spliced per-range by a small inline loop below because the
        // existing `apply_ranges` only handles a single replacement string
        // for every range.
        let new_content = apply_distinct_replacements(content, &resolved);

        // Step 4 — atomic write.
        crate::utils::atomic_write::atomic_write_file(canonical, &new_content)
            .await
            .map_err(|e| {
                ToolError::Execution(format!("Failed to write {}: {}", canonical.display(), e))
            })?;

        let path_str = canonical.to_string_lossy().to_string();
        let replacements = resolved.len();
        let had_fuzzy = resolved.iter().any(|r| r.3);
        let had_crlf = resolved.iter().any(|r| r.4);
        let suffix = if had_fuzzy && had_crlf {
            " (some edits matched after normalizing typographic punctuation and/or line endings)"
        } else if had_fuzzy {
            " (some edits matched after normalizing typographic punctuation)"
        } else if had_crlf {
            " (some edits matched after normalizing line endings; replacements written with CRLF)"
        } else {
            ""
        };
        let message = format!(
            "Applied {replacements} edits in {path_str}{suffix}"
        );
        // Render the snippet around the *first* applied edit by start order —
        // it is the one most likely to be the "lead" edit the model cares
        // about verifying.
        let first_idx = by_start[0];
        let (first_start, _, ref first_replacement, _, _) = resolved[first_idx];
        let snippet = render_edit_snippet(&new_content, first_start, first_replacement);

        info!(replacements, had_fuzzy, had_crlf, path = %path_str, "FileEditTool: multi-edit complete");
        notify_tool_result("file_edit", &message, true);

        Ok(FileEditOutput {
            success: true,
            path: path_str,
            replacements,
            message,
            snippet,
        })
    }
}

impl Default for FileEditTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for FileEditTool {
    fn clone(&self) -> Self {
        Self {
            denied_paths: self.denied_paths.clone(),
            tool_context_handle: self.tool_context_handle.clone(),
        }
    }
}

// =============================================================================
// AlephTool impl
// =============================================================================

#[async_trait]
impl AlephTool for FileEditTool {
    const NAME: &'static str = "file_edit";
    const DESCRIPTION: &'static str = r#"Perform a string replacement in a file.

Finds `old_string` in the file and replaces it with `new_string`.
- By default, `old_string` must match exactly once; if multiple matches exist the call fails.
- Set `replace_all=true` to replace every occurrence.
- `old_string` must be the raw file text — do NOT include the line-number prefixes shown by file_read.
- Matching is exact; typographic punctuation (curly quotes, em-dashes) and CRLF/LF line-ending drift are tolerated, and on a miss the error explains how to fix your input.
- The result includes a line-numbered `snippet` of the file around the edit — verify from it instead of re-reading the file.

Use this tool for surgical edits — it only changes what you specify, leaving the rest of the file intact."#;

    type Args = FileEditArgs;
    type Output = FileEditOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.call_impl(args).await.map_err(Into::into)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_single_replacement() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, "Hello World").unwrap();

        let tool = FileEditTool::new();
        let args = FileEditArgs {
            file_path: file.to_string_lossy().to_string(),
            old_string: "World".to_string(),
            new_string: "Rust".to_string(),
            replace_all: false,
            edits: vec![],
        };

        let result = AlephTool::call(&tool, args).await.unwrap();
        assert!(result.success);
        assert_eq!(result.replacements, 1);
        assert_eq!(fs::read_to_string(&file).unwrap(), "Hello Rust");
    }

    #[tokio::test]
    async fn test_replace_all() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, "aaa bbb aaa").unwrap();

        let tool = FileEditTool::new();
        let args = FileEditArgs {
            file_path: file.to_string_lossy().to_string(),
            old_string: "aaa".to_string(),
            new_string: "ccc".to_string(),
            replace_all: true,
            edits: vec![],
        };

        let result = AlephTool::call(&tool, args).await.unwrap();
        assert!(result.success);
        assert_eq!(result.replacements, 2);
        assert_eq!(fs::read_to_string(&file).unwrap(), "ccc bbb ccc");
    }

    #[tokio::test]
    async fn test_multiple_matches_without_replace_all_fails() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, "foo bar foo").unwrap();

        let tool = FileEditTool::new();
        let args = FileEditArgs {
            file_path: file.to_string_lossy().to_string(),
            old_string: "foo".to_string(),
            new_string: "baz".to_string(),
            replace_all: false,
            edits: vec![],
        };

        let result = AlephTool::call(&tool, args).await;
        assert!(result.is_err());
        // File should be unchanged
        assert_eq!(fs::read_to_string(&file).unwrap(), "foo bar foo");
    }

    #[tokio::test]
    async fn test_old_string_not_found() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, "Hello World").unwrap();

        let tool = FileEditTool::new();
        let args = FileEditArgs {
            file_path: file.to_string_lossy().to_string(),
            old_string: "NotHere".to_string(),
            new_string: "Replaced".to_string(),
            replace_all: false,
            edits: vec![],
        };

        let result = AlephTool::call(&tool, args).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_identical_strings_rejected() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, "Hello").unwrap();

        let tool = FileEditTool::new();
        let args = FileEditArgs {
            file_path: file.to_string_lossy().to_string(),
            old_string: "Hello".to_string(),
            new_string: "Hello".to_string(),
            replace_all: false,
            edits: vec![],
        };

        let result = AlephTool::call(&tool, args).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fuzzy_typographic_punctuation_match() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("t.txt");
        // File has a curly apostrophe; the model types an ASCII one.
        fs::write(&file, "it\u{2019}s here").unwrap();

        let result = AlephTool::call(
            &FileEditTool::new(),
            FileEditArgs {
                file_path: file.to_string_lossy().to_string(),
                old_string: "it's here".to_string(),
                new_string: "it is here".to_string(),
                replace_all: false,
            edits: vec![],
            }
        )
        .await
        .unwrap();
        assert!(result.success);
        assert!(
            result.message.contains("normalizing"),
            "msg: {}",
            result.message
        );
        assert_eq!(fs::read_to_string(&file).unwrap(), "it is here");
    }

    #[tokio::test]
    async fn crlf_file_multiline_edit_succeeds() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("win.txt");
        fs::write(&file, "alpha\r\nbeta\r\ngamma\r\n").unwrap();

        // The model copies from file_read output, which strips '\r'.
        let result = AlephTool::call(
            &FileEditTool::new(),
            FileEditArgs {
                file_path: file.to_string_lossy().to_string(),
                old_string: "alpha\nbeta".to_string(),
                new_string: "alpha\nBETA\nbeta".to_string(),
                replace_all: false,
            edits: vec![],
            }
        )
        .await
        .unwrap();
        assert!(result.success);
        assert!(
            result.message.contains("line endings"),
            "msg: {}",
            result.message
        );
        // The replacement's LF newlines must be written as CRLF — no mixing.
        let written = fs::read_to_string(&file).unwrap();
        assert_eq!(written, "alpha\r\nBETA\r\nbeta\r\ngamma\r\n");
    }

    #[tokio::test]
    async fn edit_result_carries_verification_snippet() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("s.txt");
        fs::write(&file, "l1\nl2\nl3\nl4\nl5\nl6\n").unwrap();

        let result = AlephTool::call(
            &FileEditTool::new(),
            FileEditArgs {
                file_path: file.to_string_lossy().to_string(),
                old_string: "l4".to_string(),
                new_string: "L4-EDITED".to_string(),
                replace_all: false,
            edits: vec![],
            }
        )
        .await
        .unwrap();
        // Snippet shows the edited line with context, file_read-style numbering.
        assert!(
            result.snippet.contains("4\tL4-EDITED"),
            "snippet: {}",
            result.snippet
        );
        assert!(
            result.snippet.contains("2\tl2"),
            "snippet: {}",
            result.snippet
        );
        assert!(
            result.snippet.contains("6\tl6"),
            "snippet: {}",
            result.snippet
        );
        assert!(
            !result.snippet.contains("l1\n"),
            "snippet: {}",
            result.snippet
        );
    }

    #[tokio::test]
    async fn binary_file_is_refused() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("blob.bin");
        fs::write(&file, [0x00, 0x01, 0x02, 0x03]).unwrap();

        let result = AlephTool::call(
            &FileEditTool::new(),
            FileEditArgs {
                file_path: file.to_string_lossy().to_string(),
                old_string: "anything".to_string(),
                new_string: "else".to_string(),
                replace_all: false,
            edits: vec![],
            }
        )
        .await;
        assert!(result.unwrap_err().to_string().contains("binary"));
    }

    #[tokio::test]
    async fn non_utf8_file_is_refused() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("latin1.txt");
        // 0xFF is invalid UTF-8 yet contains no NUL byte.
        fs::write(&file, [b'h', b'i', 0xFF]).unwrap();

        let result = AlephTool::call(
            &FileEditTool::new(),
            FileEditArgs {
                file_path: file.to_string_lossy().to_string(),
                old_string: "hi".to_string(),
                new_string: "yo".to_string(),
                replace_all: false,
            edits: vec![],
            }
        )
        .await;
        assert!(result.unwrap_err().to_string().contains("UTF-8"));
    }

    #[tokio::test]
    async fn empty_old_string_rejected() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("t.txt");
        fs::write(&file, "content").unwrap();

        let result = AlephTool::call(
            &FileEditTool::new(),
            FileEditArgs {
                file_path: file.to_string_lossy().to_string(),
                old_string: String::new(),
                new_string: "x".to_string(),
                replace_all: false,
            edits: vec![],
            }
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn whitespace_drift_produces_diagnostic() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("code.rs");
        let original = "fn main() {\n    let x = 1;\n}\n";
        fs::write(&file, original).unwrap();

        // Over-indented old_string — not even a substring of the file.
        let result = AlephTool::call(
            &FileEditTool::new(),
            FileEditArgs {
                file_path: file.to_string_lossy().to_string(),
                old_string: "            let x = 1;".to_string(),
                new_string: "    let x = 2;".to_string(),
                replace_all: false,
            edits: vec![],
            }
        )
        .await;
        let err = result.unwrap_err().to_string();
        assert!(err.contains("whitespace"), "err was: {err}");
        // The file must be untouched when the match fails.
        assert_eq!(fs::read_to_string(&file).unwrap(), original);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn edit_preserves_file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let file = dir.path().join("run.sh");
        fs::write(&file, "echo one").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();

        let result = AlephTool::call(
            &FileEditTool::new(),
            FileEditArgs {
                file_path: file.to_string_lossy().to_string(),
                old_string: "one".to_string(),
                new_string: "two".to_string(),
                replace_all: false,
            edits: vec![],
            }
        )
        .await
        .unwrap();

        assert!(result.success);
        assert_eq!(fs::read_to_string(&file).unwrap(), "echo two");
        // The atomic temp-file-and-rename write must not drop the file's mode.
        let mode = fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755, "executable bit must survive the edit");
    }

    // ========================================================================
    // Multi-edit (edits: []) — applies every replacement in a single call
    // against the ORIGINAL file, refusing overlaps and non-unique matches.
    // ========================================================================

    /// Two disjoint edits in a single call. Each is matched against the
    /// pre-edit content, and the result on disk is the original file with
    /// both splices applied in their original positions.
    #[tokio::test]
    async fn multi_edit_applies_two_disjoint_edits_atomically() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "alpha=1\nbeta=2\ngamma=3\n").unwrap();

        let result = AlephTool::call(
            &FileEditTool::new(),
            FileEditArgs {
                file_path: file.to_string_lossy().to_string(),
                old_string: String::new(),
                new_string: String::new(),
                replace_all: false,
                edits: vec![
                    EditOp {
                        old_string: "alpha=1".to_string(),
                        new_string: "alpha=10".to_string(),
                    },
                    EditOp {
                        old_string: "gamma=3".to_string(),
                        new_string: "gamma=30".to_string(),
                    },
                ],
            },
        )
        .await
        .unwrap();

        assert!(result.success);
        assert_eq!(result.replacements, 2);
        // `beta=` is between the two edits and must survive untouched.
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "alpha=10\nbeta=2\ngamma=30\n"
        );
    }

    /// Multi-edit applies later splices against the ORIGINAL content, not the
    /// incrementally-mutated buffer. A second edit whose `old_string` was
    /// produced by the first edit therefore must NOT be able to silently
    /// match in the half-written file.
    #[tokio::test]
    async fn multi_edit_matches_each_op_against_the_original_file() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "FOO\nbar\n").unwrap();

        // The second edit's old_string would only exist if the first edit
        // were applied first. Matching against the original file means the
        // first op succeeds, the second op does NOT match, and the whole
        // call is refused with a clean diagnostic — the file stays intact.
        let result = AlephTool::call(
            &FileEditTool::new(),
            FileEditArgs {
                file_path: file.to_string_lossy().to_string(),
                old_string: String::new(),
                new_string: String::new(),
                replace_all: false,
                edits: vec![
                    EditOp {
                        old_string: "FOO".to_string(),
                        new_string: "BAZ".to_string(),
                    },
                    EditOp {
                        old_string: "BAZ".to_string(),
                        new_string: "QUUX".to_string(),
                    },
                ],
            },
        )
        .await;
        let err = result.unwrap_err().to_string();
        assert!(err.contains("edits[1]"), "err: {err}");
        // Original file untouched — the all-or-nothing contract.
        assert_eq!(fs::read_to_string(&file).unwrap(), "FOO\nbar\n");
    }

    /// Overlapping byte ranges between two ops are refused, naming the
    /// offending pair. This is the guard that prevents "edit succeeded but
    /// the file is wrong" — when two ops target the same region, only one
    /// of them can win on disk, and silently picking a winner corrupts the
    /// model's intent.
    #[tokio::test]
    async fn multi_edit_refuses_overlapping_ranges() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "aaaa bbbb cccc\n").unwrap();

        // The two old_strings are unique on their own but the ranges they
        // resolve to overlap: "aa aa" sits inside "aaaa bbbb".
        let result = AlephTool::call(
            &FileEditTool::new(),
            FileEditArgs {
                file_path: file.to_string_lossy().to_string(),
                old_string: String::new(),
                new_string: String::new(),
                replace_all: false,
                edits: vec![
                    EditOp {
                        old_string: "aaaa".to_string(),
                        new_string: "xxxx".to_string(),
                    },
                    EditOp {
                        old_string: "aaaa bbbb".to_string(),
                        new_string: "yyyy".to_string(),
                    },
                ],
            },
        )
        .await;
        let err = result.unwrap_err().to_string();
        assert!(err.contains("overlap"), "err: {err}");
        // File untouched on refusal.
        assert_eq!(fs::read_to_string(&file).unwrap(), "aaaa bbbb cccc\n");
    }

    /// A single `old_string` that matches the file in more than one place is
    /// refused for a multi-edit call, just like the legacy single-edit
    /// branch's uniqueness gate. The error names the offending op index so
    /// the model can disambiguate by adding context.
    #[tokio::test]
    async fn multi_edit_refuses_non_unique_op() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "foo bar foo\n").unwrap();

        let result = AlephTool::call(
            &FileEditTool::new(),
            FileEditArgs {
                file_path: file.to_string_lossy().to_string(),
                old_string: String::new(),
                new_string: String::new(),
                replace_all: false,
                edits: vec![
                    EditOp {
                        old_string: "foo bar foo".to_string(),
                        new_string: "X".to_string(),
                    },
                    EditOp {
                        old_string: "foo".to_string(),
                        new_string: "Y".to_string(),
                    },
                ],
            },
        )
        .await;
        let err = result.unwrap_err().to_string();
        assert!(err.contains("edits[1]") && err.contains("2 places"), "err: {err}");
        // File untouched on refusal.
        assert_eq!(fs::read_to_string(&file).unwrap(), "foo bar foo\n");
    }

    /// Empty `old_string` inside `edits[]` is rejected at the same gate as
    /// the legacy single-edit form. The error names the array index so the
    /// model can target the right entry.
    #[tokio::test]
    async fn multi_edit_empty_op_is_rejected() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "x\n").unwrap();

        let result = AlephTool::call(
            &FileEditTool::new(),
            FileEditArgs {
                file_path: file.to_string_lossy().to_string(),
                old_string: String::new(),
                new_string: String::new(),
                replace_all: false,
                edits: vec![EditOp {
                    old_string: "x".to_string(),
                    new_string: "y".to_string(),
                }, EditOp {
                    old_string: String::new(),
                    new_string: "z".to_string(),
                }],
            },
        )
        .await;
        assert!(result.unwrap_err().to_string().contains("edits[1]"));
    }

    /// `edits` wins when both shapes are present — the legacy fields are
    /// ignored. This matches the documented contract and keeps the
    /// multi-edit form strictly a superset.
    #[tokio::test]
    async fn multi_edit_takes_precedence_over_legacy_fields() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("a.txt");
        fs::write(&file, "alpha=1\nbeta=2\n").unwrap();

        // Legacy fields would edit `alpha=1` -> `WRONG`. The `edits` array
        // edits the same `alpha=1` -> `alpha=10`. The legacy fields must be
        // ignored — the file on disk must reflect the array's intent.
        let result = AlephTool::call(
            &FileEditTool::new(),
            FileEditArgs {
                file_path: file.to_string_lossy().to_string(),
                old_string: "alpha=1".to_string(),
                new_string: "WRONG".to_string(),
                replace_all: false,
                edits: vec![EditOp {
                    old_string: "alpha=1".to_string(),
                    new_string: "alpha=10".to_string(),
                }],
            },
        )
        .await
        .unwrap();
        assert!(result.success);
        assert_eq!(fs::read_to_string(&file).unwrap(), "alpha=10\nbeta=2\n");
    }
}
