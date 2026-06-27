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
        out.push_str(&format!("{lineno:>width$}\t{}\n", clamp_line(lines[idx])));
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

// =============================================================================
// Args & Output
// =============================================================================

/// Arguments for the `file_edit` tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct FileEditArgs {
    /// Absolute or relative path to the file to edit
    pub file_path: String,
    /// The exact string to find in the file
    pub old_string: String,
    /// The replacement string
    pub new_string: String,
    /// Replace all occurrences (default: false — single match only)
    #[serde(default)]
    pub replace_all: bool,
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

        // Validate: old_string must be non-empty and differ from new_string.
        if args.old_string.is_empty() {
            let err = ToolError::InvalidArgs("old_string must not be empty".to_string());
            notify_tool_result("file_edit", &err.to_string(), false);
            return Err(err);
        }
        if args.old_string == args.new_string {
            let err = ToolError::InvalidArgs(
                "old_string and new_string are identical; nothing to change".to_string(),
            );
            notify_tool_result("file_edit", &err.to_string(), false);
            return Err(err);
        }

        // Resolve & validate path
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

        // Locate the edit target: exact match, then CRLF / typographic-folding
        // fallbacks, then an actionable diagnostic.
        let (ranges, fuzzy, crlf) = match locate(&content, &args.old_string) {
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
        let replacement = if crlf && !args.new_string.contains('\r') {
            std::borrow::Cow::Owned(args.new_string.replace('\n', "\r\n"))
        } else {
            std::borrow::Cow::Borrowed(args.new_string.as_str())
        };

        // `ranges` is non-empty here; apply all under `replace_all`, else the first.
        let applied = if args.replace_all {
            &ranges[..]
        } else {
            &ranges[..1]
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
        let snippet = render_edit_snippet(&new_content, applied[0].0, &replacement);

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
            },
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
            },
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
            },
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
            },
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
            },
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
            },
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
            },
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
            },
        )
        .await
        .unwrap();

        assert!(result.success);
        assert_eq!(fs::read_to_string(&file).unwrap(), "echo two");
        // The atomic temp-file-and-rename write must not drop the file's mode.
        let mode = fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755, "executable bit must survive the edit");
    }
}
