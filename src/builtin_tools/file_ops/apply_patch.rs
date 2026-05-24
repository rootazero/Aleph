//! `apply_patch` — V4A-style multi-file structured patches.
//!
//! Aleph's port of codex's [`apply_patch` tool][codex-apply-patch]. The
//! envelope syntax is identical so models trained on codex's format work
//! out-of-the-box:
//!
//! ```text
//! *** Begin Patch
//! *** Add File: hello.txt
//! +Hello world
//! *** Update File: src/app.py
//! *** Move to: src/main.py
//! @@ def greet():
//! -print("Hi")
//! +print("Hello, world!")
//! *** Delete File: obsolete.txt
//! *** End Patch
//! ```
//!
//! Implementation notes:
//!
//! - **Lean parser.** ~200 lines of line-by-line state machine, not codex's
//!   4 600-line Lark-grammar parser. We don't need streaming hunk preview;
//!   the agent submits a complete patch.
//! - **Reuses [`super::edit_match`].** Each hunk is converted to one
//!   `(old_text, new_text)` pair and applied through the same fuzzy locator
//!   `file_edit` uses, so typographic/whitespace drift is handled
//!   identically.
//! - **Path safety.** All file refs are routed through
//!   [`super::path_utils::check_and_resolve_path`], which honours
//!   workspace-scoped output dirs, denied-path lists, and rejects symlink
//!   escapes — same as `file_edit`/`file_write`.
//! - **Best-effort atomicity.** We parse the whole envelope before doing
//!   any filesystem work, so syntax errors never half-apply. Per-op
//!   failures abort the remaining ops but already-applied changes stay on
//!   disk and are reported in the result; the model can retry the
//!   remainder.
//!
//! [codex-apply-patch]: https://github.com/openai/codex/tree/main/codex-rs/apply-patch

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use super::edit_match::{apply_ranges, locate, LocateResult};
use super::path_utils::{check_and_resolve_path, get_denied_paths};
use super::text::is_binary;
use crate::builtin_tools::error::ToolError;
use crate::error::Result;
use crate::tools::AlephTool;

// =============================================================================
// Public surface
// =============================================================================

/// Tool argument: a single `*** Begin Patch ... *** End Patch` envelope.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ApplyPatchArgs {
    /// The full patch envelope (V4A format).
    pub patch: String,
}

/// Per-file outcome surfaced back to the model so it can recover from
/// partial failures without re-reading the filesystem.
#[derive(Debug, Clone, Serialize)]
pub struct FileOutcome {
    pub path: String,
    pub op: &'static str,
    pub success: bool,
    pub message: String,
}

/// Aggregate output for one `apply_patch` invocation.
#[derive(Debug, Clone, Serialize)]
pub struct ApplyPatchOutput {
    pub success: bool,
    pub message: String,
    pub files_changed: usize,
    pub files: Vec<FileOutcome>,
}

/// The `apply_patch` builtin tool.
pub struct ApplyPatchTool {
    denied_paths: Vec<String>,
    tool_context_handle: Option<crate::tools::ToolContextHandle>,
}

impl ApplyPatchTool {
    pub const NAME: &'static str = "apply_patch";

    pub const DESCRIPTION: &'static str = r#"Apply a V4A-format multi-file structured patch to the workspace.

Envelope:

*** Begin Patch
[ one or more file sections ]
*** End Patch

File sections (each MUST start with a header):
- *** Add File: <path>           — create a new file. Following lines are `+<content>`.
- *** Delete File: <path>        — remove an existing file.
- *** Update File: <path>        — patch an existing file in place.
  Optional immediately-following: *** Move to: <new path>
  Then one or more hunks, each introduced by `@@ <optional header>`.

Hunk line prefixes (space / minus / plus):
  ` <context>`  unchanged context
  `-<old>`      line to delete
  `+<new>`      line to add

Trailing `*** End of File` on a hunk indicates EOF anchoring.

Paths MUST be relative — absolute paths are rejected. All paths are
resolved against the workspace's output directory if one is configured,
otherwise the session working directory. This tool prefers `apply_patch`
over multiple `file_edit` / `file_write` calls when the model needs to
make several coordinated edits at once."#;

    pub fn new() -> Self {
        Self {
            denied_paths: get_denied_paths(),
            tool_context_handle: None,
        }
    }

    pub fn with_tool_context(mut self, handle: crate::tools::ToolContextHandle) -> Self {
        self.tool_context_handle = Some(handle);
        self
    }

    async fn resolve_output_dir(&self) -> Option<PathBuf> {
        if let Some(ref handle) = self.tool_context_handle {
            let ctx = handle.read().await;
            Some(ctx.output_dir.join("documents"))
        } else {
            None
        }
    }

    async fn run(&self, args: ApplyPatchArgs) -> std::result::Result<ApplyPatchOutput, ToolError> {
        use crate::builtin_tools::{notify_tool_result, notify_tool_start};

        notify_tool_start(Self::NAME, "apply_patch");
        info!(patch_bytes = args.patch.len(), "ApplyPatchTool::call invoked");

        // Parse the whole envelope before touching the filesystem.
        let ops = parse_patch(&args.patch).map_err(ToolError::InvalidArgs)?;

        let output_dir = self.resolve_output_dir().await;
        let output_dir_ref = output_dir.as_deref();

        let mut outcomes: Vec<FileOutcome> = Vec::with_capacity(ops.len());
        let mut all_ok = true;
        let mut applied = 0usize;

        for op in ops {
            let outcome = self.apply_one(&op, output_dir_ref);
            if outcome.success {
                applied += 1;
            } else {
                all_ok = false;
            }
            outcomes.push(outcome);
            // Stop on first failure so we don't pile garbage onto disk.
            if !all_ok {
                break;
            }
        }

        let message = if all_ok {
            format!("applied {} file operation(s)", applied)
        } else {
            format!(
                "applied {}/{} operations; aborted on first failure",
                applied,
                outcomes.len()
            )
        };

        let out = ApplyPatchOutput {
            success: all_ok,
            message: message.clone(),
            files_changed: applied,
            files: outcomes,
        };
        notify_tool_result(Self::NAME, &message, all_ok);
        Ok(out)
    }

    fn apply_one(&self, op: &PatchOp, output_dir: Option<&Path>) -> FileOutcome {
        match op {
            PatchOp::Add { path, lines } => self.do_add(path, lines, output_dir),
            PatchOp::Delete { path } => self.do_delete(path, output_dir),
            PatchOp::Update {
                path,
                move_to,
                hunks,
            } => self.do_update(path, move_to.as_deref(), hunks, output_dir),
        }
    }

    fn do_add(
        &self,
        path: &str,
        lines: &[String],
        output_dir: Option<&Path>,
    ) -> FileOutcome {
        let resolved = match self.resolve(path, output_dir) {
            Ok(p) => p,
            Err(msg) => return fail("add", path, msg),
        };
        if resolved.exists() {
            return fail(
                "add",
                path,
                format!(
                    "{} already exists — use `*** Update File:` instead",
                    resolved.display()
                ),
            );
        }
        if let Some(parent) = resolved.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return fail(
                    "add",
                    path,
                    format!("failed to create parent {}: {}", parent.display(), e),
                );
            }
        }
        let body = if lines.is_empty() {
            String::new()
        } else {
            // V4A `+lines` already strip the leading `+`; join with newlines
            // and append a trailing newline so the file ends in a newline
            // (the codex format implies a final newline).
            let mut s = lines.join("\n");
            s.push('\n');
            s
        };
        match std::fs::write(&resolved, body) {
            Ok(()) => FileOutcome {
                path: resolved.display().to_string(),
                op: "add",
                success: true,
                message: format!("created ({} bytes)", lines.iter().map(|l| l.len() + 1).sum::<usize>()),
            },
            Err(e) => fail("add", path, format!("write failed: {}", e)),
        }
    }

    fn do_delete(&self, path: &str, output_dir: Option<&Path>) -> FileOutcome {
        let resolved = match self.resolve(path, output_dir) {
            Ok(p) => p,
            Err(msg) => return fail("delete", path, msg),
        };
        if !resolved.exists() {
            return fail("delete", path, format!("{} does not exist", resolved.display()));
        }
        match std::fs::remove_file(&resolved) {
            Ok(()) => FileOutcome {
                path: resolved.display().to_string(),
                op: "delete",
                success: true,
                message: "deleted".into(),
            },
            Err(e) => fail("delete", path, format!("delete failed: {}", e)),
        }
    }

    fn do_update(
        &self,
        path: &str,
        move_to: Option<&str>,
        hunks: &[Hunk],
        output_dir: Option<&Path>,
    ) -> FileOutcome {
        let resolved = match self.resolve(path, output_dir) {
            Ok(p) => p,
            Err(msg) => return fail("update", path, msg),
        };
        if !resolved.exists() {
            return fail("update", path, format!("{} does not exist", resolved.display()));
        }

        // Read + binary refusal mirrors `file_edit::read_text_file`.
        let bytes = match std::fs::read(&resolved) {
            Ok(b) => b,
            Err(e) => return fail("update", path, format!("read failed: {}", e)),
        };
        if is_binary(&bytes) {
            return fail(
                "update",
                path,
                format!("{} is binary — apply_patch only edits text", resolved.display()),
            );
        }
        let mut content = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => {
                return fail(
                    "update",
                    path,
                    format!("{} is not valid UTF-8 text", resolved.display()),
                );
            }
        };

        let mut hunks_applied = 0;
        for (i, hunk) in hunks.iter().enumerate() {
            let (old_text, new_text) = hunk_to_old_new(hunk);
            if old_text.is_empty() {
                // Pure addition with no context — append (rare). Skip
                // gracefully rather than producing a confusing "not found".
                continue;
            }
            match locate(&content, &old_text) {
                LocateResult::Exact(ranges) | LocateResult::Folded(ranges) => {
                    // V4A hunks identify exactly one location per hunk.
                    let first = ranges.first().copied().into_iter().collect::<Vec<_>>();
                    content = apply_ranges(&content, &first, &new_text);
                    hunks_applied += 1;
                }
                LocateResult::NotFound(why) => {
                    return fail(
                        "update",
                        path,
                        format!("hunk {}/{} did not match: {}", i + 1, hunks.len(), why),
                    );
                }
            }
        }

        if let Err(e) = std::fs::write(&resolved, &content) {
            return fail("update", path, format!("write-back failed: {}", e));
        }

        let final_path = if let Some(new_path) = move_to {
            let new_resolved = match self.resolve(new_path, output_dir) {
                Ok(p) => p,
                Err(msg) => {
                    return fail(
                        "update",
                        path,
                        format!("move-to path rejected: {}", msg),
                    );
                }
            };
            if let Some(parent) = new_resolved.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    return fail(
                        "update",
                        path,
                        format!("failed to create move-to parent {}: {}", parent.display(), e),
                    );
                }
            }
            if let Err(e) = std::fs::rename(&resolved, &new_resolved) {
                return fail(
                    "update",
                    path,
                    format!(
                        "rename {} → {} failed: {}",
                        resolved.display(),
                        new_resolved.display(),
                        e
                    ),
                );
            }
            new_resolved
        } else {
            resolved
        };

        FileOutcome {
            path: final_path.display().to_string(),
            op: "update",
            success: true,
            message: format!("applied {} hunk(s)", hunks_applied),
        }
    }

    fn resolve(&self, path: &str, output_dir: Option<&Path>) -> std::result::Result<PathBuf, String> {
        if Path::new(path).is_absolute() {
            return Err(format!(
                "absolute path `{}` is not allowed; apply_patch requires relative paths",
                path
            ));
        }
        check_and_resolve_path(Path::new(path), &self.denied_paths, output_dir)
            .map_err(|e| e.to_string())
    }
}

impl Default for ApplyPatchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for ApplyPatchTool {
    fn clone(&self) -> Self {
        Self {
            denied_paths: self.denied_paths.clone(),
            tool_context_handle: self.tool_context_handle.clone(),
        }
    }
}

#[async_trait]
impl AlephTool for ApplyPatchTool {
    const NAME: &'static str = "apply_patch";
    const DESCRIPTION: &'static str = ApplyPatchTool::DESCRIPTION;
    type Args = ApplyPatchArgs;
    type Output = ApplyPatchOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.run(args).await.map_err(Into::into)
    }
}

// =============================================================================
// Patch model
// =============================================================================

#[derive(Debug, Clone, PartialEq)]
enum PatchOp {
    Add { path: String, lines: Vec<String> },
    Delete { path: String },
    Update {
        path: String,
        move_to: Option<String>,
        hunks: Vec<Hunk>,
    },
}

#[derive(Debug, Clone, PartialEq)]
struct Hunk {
    /// Optional `@@ <header>` line minus the `@@ ` prefix.
    header: Option<String>,
    /// Lines including the leading ` `, `-`, or `+` marker (stripped at apply time).
    lines: Vec<HunkLine>,
    /// `true` if a `*** End of File` marker followed this hunk.
    eof_anchor: bool,
}

#[derive(Debug, Clone, PartialEq)]
enum HunkLine {
    Context(String),
    Remove(String),
    Add(String),
}

fn hunk_to_old_new(hunk: &Hunk) -> (String, String) {
    let mut old = String::new();
    let mut new = String::new();
    let mut first_old = true;
    let mut first_new = true;
    for line in &hunk.lines {
        match line {
            HunkLine::Context(s) => {
                if !first_old {
                    old.push('\n');
                }
                if !first_new {
                    new.push('\n');
                }
                old.push_str(s);
                new.push_str(s);
                first_old = false;
                first_new = false;
            }
            HunkLine::Remove(s) => {
                if !first_old {
                    old.push('\n');
                }
                old.push_str(s);
                first_old = false;
            }
            HunkLine::Add(s) => {
                if !first_new {
                    new.push('\n');
                }
                new.push_str(s);
                first_new = false;
            }
        }
    }
    let _ = hunk.eof_anchor; // reserved for a future end-of-file anchor pass
    (old, new)
}

// =============================================================================
// Parser
// =============================================================================

fn parse_patch(input: &str) -> std::result::Result<Vec<PatchOp>, String> {
    // Strip an optional leading BOM and trim trailing whitespace per line is
    // intentionally NOT done — V4A is whitespace-significant.
    let mut lines = input.split('\n').peekable();

    let first = lines
        .next()
        .ok_or_else(|| "patch is empty".to_string())?
        .trim_end_matches('\r');
    if first.trim() != "*** Begin Patch" {
        return Err(format!(
            "patch must start with `*** Begin Patch` (got `{}`)",
            first
        ));
    }

    let mut ops: Vec<PatchOp> = Vec::new();
    loop {
        let line = match lines.next() {
            Some(l) => l.trim_end_matches('\r'),
            None => return Err("patch ended before `*** End Patch`".into()),
        };
        if line.trim() == "*** End Patch" {
            break;
        }
        if line.is_empty() {
            continue;
        }
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            let mut buf = Vec::new();
            loop {
                match lines.peek() {
                    Some(next) if next.starts_with("***") => break,
                    Some(_) => {
                        let next = lines.next().unwrap().trim_end_matches('\r');
                        if let Some(stripped) = next.strip_prefix('+') {
                            buf.push(stripped.to_string());
                        } else if next.is_empty() {
                            // Empty lines inside Add File map to blank file lines.
                            buf.push(String::new());
                        } else {
                            return Err(format!(
                                "Add File `{}`: expected `+...` line, got `{}`",
                                path, next
                            ));
                        }
                    }
                    None => return Err(format!("Add File `{}`: unterminated", path)),
                }
            }
            ops.push(PatchOp::Add {
                path: path.to_string(),
                lines: buf,
            });
        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
            ops.push(PatchOp::Delete {
                path: path.to_string(),
            });
        } else if let Some(path) = line.strip_prefix("*** Update File: ") {
            // Optional Move to:
            let mut move_to: Option<String> = None;
            if let Some(next) = lines.peek() {
                if let Some(dest) = next.trim_end_matches('\r').strip_prefix("*** Move to: ") {
                    move_to = Some(dest.to_string());
                    lines.next();
                }
            }
            // One or more hunks until the next `*** ...` line.
            let mut hunks: Vec<Hunk> = Vec::new();
            let mut current: Option<Hunk> = None;
            loop {
                match lines.peek() {
                    Some(next) if next.starts_with("*** ") && !next.starts_with("*** End of File") => break,
                    Some(_) => {
                        let raw = lines.next().unwrap().trim_end_matches('\r');
                        if let Some(rest) = raw.strip_prefix("@@") {
                            if let Some(h) = current.take() {
                                hunks.push(h);
                            }
                            let header = rest.trim();
                            current = Some(Hunk {
                                header: if header.is_empty() {
                                    None
                                } else {
                                    Some(header.to_string())
                                },
                                lines: Vec::new(),
                                eof_anchor: false,
                            });
                        } else if raw.trim() == "*** End of File" {
                            if let Some(h) = current.as_mut() {
                                h.eof_anchor = true;
                            }
                        } else if let Some(c) = raw.strip_prefix(' ') {
                            current_mut(&mut current).lines.push(HunkLine::Context(c.to_string()));
                        } else if let Some(c) = raw.strip_prefix('-') {
                            current_mut(&mut current).lines.push(HunkLine::Remove(c.to_string()));
                        } else if let Some(c) = raw.strip_prefix('+') {
                            current_mut(&mut current).lines.push(HunkLine::Add(c.to_string()));
                        } else if raw.is_empty() {
                            // Blank lines inside hunks are valid context.
                            current_mut(&mut current).lines.push(HunkLine::Context(String::new()));
                        } else {
                            return Err(format!(
                                "Update File `{}`: unexpected line `{}` (each hunk line must start with ` `, `+`, or `-`)",
                                path, raw
                            ));
                        }
                    }
                    None => return Err(format!("Update File `{}`: unterminated", path)),
                }
            }
            if let Some(h) = current.take() {
                hunks.push(h);
            }
            if hunks.is_empty() {
                return Err(format!(
                    "Update File `{}`: at least one `@@`-introduced hunk is required",
                    path
                ));
            }
            ops.push(PatchOp::Update {
                path: path.to_string(),
                move_to,
                hunks,
            });
        } else {
            return Err(format!(
                "expected file header (`*** Add File:`, `*** Update File:`, `*** Delete File:`) or `*** End Patch`, got `{}`",
                line
            ));
        }
    }

    if ops.is_empty() {
        return Err("patch contained no file operations".into());
    }
    Ok(ops)
}

/// Helper: a hunk body line arrived before any `@@` header. Auto-open an
/// anonymous hunk so V4A patches that omit the header for a single change
/// still apply.
fn current_mut(slot: &mut Option<Hunk>) -> &mut Hunk {
    if slot.is_none() {
        *slot = Some(Hunk {
            header: None,
            lines: Vec::new(),
            eof_anchor: false,
        });
    }
    slot.as_mut().expect("just initialised")
}

// =============================================================================
// Helpers
// =============================================================================

fn fail(op: &'static str, path: &str, message: impl Into<String>) -> FileOutcome {
    FileOutcome {
        path: path.to_string(),
        op,
        success: false,
        message: message.into(),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_handles_add_update_delete_envelope() {
        let patch = "*** Begin Patch\n\
*** Add File: hello.txt\n\
+Hello\n\
+World\n\
*** Update File: src/app.py\n\
*** Move to: src/main.py\n\
@@ def greet():\n\
-print(\"Hi\")\n\
+print(\"Hello\")\n\
*** Delete File: old.txt\n\
*** End Patch\n";
        let ops = parse_patch(patch).expect("parse");
        assert_eq!(ops.len(), 3);
        match &ops[0] {
            PatchOp::Add { path, lines } => {
                assert_eq!(path, "hello.txt");
                assert_eq!(lines, &vec!["Hello".to_string(), "World".to_string()]);
            }
            other => panic!("expected Add, got {:?}", other),
        }
        match &ops[1] {
            PatchOp::Update { path, move_to, hunks } => {
                assert_eq!(path, "src/app.py");
                assert_eq!(move_to.as_deref(), Some("src/main.py"));
                assert_eq!(hunks.len(), 1);
                assert_eq!(hunks[0].header.as_deref(), Some("def greet():"));
                assert_eq!(hunks[0].lines.len(), 2);
            }
            other => panic!("expected Update, got {:?}", other),
        }
        match &ops[2] {
            PatchOp::Delete { path } => assert_eq!(path, "old.txt"),
            other => panic!("expected Delete, got {:?}", other),
        }
    }

    #[test]
    fn parser_rejects_missing_envelope() {
        assert!(parse_patch("").is_err());
        assert!(parse_patch("Hello world\n*** End Patch\n").is_err());
        assert!(parse_patch("*** Begin Patch\n").is_err()); // unterminated
    }

    #[test]
    fn parser_rejects_unknown_op() {
        let patch = "*** Begin Patch\n*** Rename File: a b\n*** End Patch\n";
        assert!(parse_patch(patch).is_err());
    }

    #[test]
    fn hunk_to_old_new_assembles_text() {
        let hunk = Hunk {
            header: None,
            lines: vec![
                HunkLine::Context("a".into()),
                HunkLine::Remove("b".into()),
                HunkLine::Add("B".into()),
                HunkLine::Context("c".into()),
            ],
            eof_anchor: false,
        };
        let (old, new) = hunk_to_old_new(&hunk);
        assert_eq!(old, "a\nb\nc");
        assert_eq!(new, "a\nB\nc");
    }

    #[test]
    fn end_to_end_add_and_update_in_tempdir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let app_py = dir.path().join("app.py");
        std::fs::write(&app_py, "x = 1\ny = 2\nz = 3\n").unwrap();

        // Run the add+update in the temp directory using absolute prefix in
        // the resolved paths. We bypass the tool's path-resolution layer
        // here by constructing PatchOps directly.
        let tool = ApplyPatchTool::new();
        let hunk = Hunk {
            header: None,
            lines: vec![
                HunkLine::Context("x = 1".into()),
                HunkLine::Remove("y = 2".into()),
                HunkLine::Add("y = 20".into()),
                HunkLine::Context("z = 3".into()),
            ],
            eof_anchor: false,
        };
        let outcome = tool.do_update(
            app_py.to_str().unwrap(),
            None,
            &[hunk],
            Some(dir.path()),
        );
        // do_update will reject the absolute path; rerun with relative.
        assert!(!outcome.success);
        let outcome2 = tool.do_update("app.py", None, &[Hunk {
            header: None,
            lines: vec![
                HunkLine::Context("x = 1".into()),
                HunkLine::Remove("y = 2".into()),
                HunkLine::Add("y = 20".into()),
                HunkLine::Context("z = 3".into()),
            ],
            eof_anchor: false,
        }], Some(dir.path()));
        assert!(outcome2.success, "{:?}", outcome2);

        let updated = std::fs::read_to_string(&app_py).unwrap();
        assert_eq!(updated, "x = 1\ny = 20\nz = 3\n");
    }

    #[test]
    fn update_reports_unmatched_hunk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let app_py = dir.path().join("app.py");
        std::fs::write(&app_py, "a\nb\nc\n").unwrap();
        let tool = ApplyPatchTool::new();
        let hunk = Hunk {
            header: None,
            lines: vec![
                HunkLine::Context("zzz".into()),
                HunkLine::Remove("yyy".into()),
                HunkLine::Add("YYY".into()),
            ],
            eof_anchor: false,
        };
        let outcome = tool.do_update("app.py", None, &[hunk], Some(dir.path()));
        assert!(!outcome.success);
        assert!(outcome.message.contains("did not match"), "{:?}", outcome);
    }
}
