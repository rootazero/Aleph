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

use super::edit_match::{apply_ranges, locate, locate_lines, LocateResult};
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

    #[must_use]
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
        info!(
            patch_bytes = args.patch.len(),
            "ApplyPatchTool::call invoked"
        );

        // Parse the whole envelope before touching the filesystem.
        let ops = parse_patch(&args.patch).map_err(ToolError::InvalidArgs)?;

        let output_dir = self.resolve_output_dir().await;
        let output_dir_ref = output_dir.as_deref();

        let mut outcomes: Vec<FileOutcome> = Vec::with_capacity(ops.len());
        let mut all_ok = true;
        let mut applied = 0usize;

        for op in ops {
            let outcome = self.apply_one(&op, output_dir_ref).await;
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
            format!("applied {applied} file operation(s)")
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

    async fn apply_one(&self, op: &PatchOp, output_dir: Option<&Path>) -> FileOutcome {
        match op {
            PatchOp::Add { path, lines } => self.do_add(path, lines, output_dir).await,
            PatchOp::Delete { path } => self.do_delete(path, output_dir).await,
            PatchOp::Update {
                path,
                move_to,
                hunks,
            } => {
                self.do_update(path, move_to.as_deref(), hunks, output_dir)
                    .await
            }
        }
    }

    async fn do_add(&self, path: &str, lines: &[String], output_dir: Option<&Path>) -> FileOutcome {
        let resolved = match self.resolve(path, output_dir) {
            Ok(p) => p,
            Err(msg) => return fail("add", path, msg),
        };
        // Cross-agent write guard (same lock file_write / file_edit hold).
        let _path_guard = crate::tools::path_locks::lock_path(&resolved).await;
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
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
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
        // Atomic write-back (stage to temp + fsync + rename), matching the
        // durability guarantee of file_edit/file_write — a crash mid-write must
        // never leave the file truncated.
        let byte_count = body.len();
        match crate::utils::atomic_write::atomic_write_file(&resolved, &body).await {
            Ok(()) => FileOutcome {
                path: resolved.display().to_string(),
                op: "add",
                success: true,
                message: format!("created ({byte_count} bytes)"),
            },
            Err(e) => fail("add", path, format!("write failed: {e}")),
        }
    }

    async fn do_delete(&self, path: &str, output_dir: Option<&Path>) -> FileOutcome {
        let resolved = match self.resolve(path, output_dir) {
            Ok(p) => p,
            Err(msg) => return fail("delete", path, msg),
        };
        // Cross-agent write guard (same lock file_write / file_edit hold).
        let _path_guard = crate::tools::path_locks::lock_path(&resolved).await;
        if !resolved.exists() {
            return fail(
                "delete",
                path,
                format!("{} does not exist", resolved.display()),
            );
        }
        match tokio::fs::remove_file(&resolved).await {
            Ok(()) => FileOutcome {
                path: resolved.display().to_string(),
                op: "delete",
                success: true,
                message: "deleted".into(),
            },
            Err(e) => fail("delete", path, format!("delete failed: {e}")),
        }
    }

    async fn do_update(
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
        // Resolve the move-to destination BEFORE taking any lock so both
        // locks can be acquired in a canonical (sorted) order — two
        // concurrent updates with crossed `move_to` targets could otherwise
        // deadlock (A holds src wants dst while B holds dst wants src).
        // Failing a bad destination here, before hunks are applied, also
        // avoids a half-done update followed by a rejected rename.
        let move_dest: Option<PathBuf> = match move_to {
            Some(new_path) => match self.resolve(new_path, output_dir) {
                Ok(p) => Some(p),
                Err(msg) => {
                    return fail("update", path, format!("move-to path rejected: {msg}"));
                }
            },
            None => None,
        };
        // Cross-agent write guard: read → apply hunks → write below is a
        // lost-update window across concurrent harnesses; the rename target
        // (if any) is guarded too so a concurrent writer cannot be clobbered.
        let (_guard_a, _guard_b) = match move_dest.as_ref() {
            Some(dest) if dest != &resolved => {
                let (first, second) = if dest < &resolved {
                    (dest, &resolved)
                } else {
                    (&resolved, dest)
                };
                let ga = crate::tools::path_locks::lock_path(first).await;
                let gb = crate::tools::path_locks::lock_path(second).await;
                (ga, Some(gb))
            }
            _ => (crate::tools::path_locks::lock_path(&resolved).await, None),
        };
        if !resolved.exists() {
            return fail(
                "update",
                path,
                format!("{} does not exist", resolved.display()),
            );
        }

        // Read + binary refusal mirrors `file_edit::read_text_file`.
        let bytes = match tokio::fs::read(&resolved).await {
            Ok(b) => b,
            Err(e) => return fail("update", path, format!("read failed: {e}")),
        };
        if is_binary(&bytes) {
            return fail(
                "update",
                path,
                format!(
                    "{} is binary — apply_patch only edits text",
                    resolved.display()
                ),
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
        let mut skipped_context_less = 0usize;
        // Forward search cursor: each hunk is located only in the content AT OR
        // AFTER the end of the previous hunk's applied edit. V4A hunks are
        // ordered top-to-bottom, so a later hunk must never bind to an earlier
        // (already-processed) occurrence of an identical block — without this,
        // two hunks editing two identical blocks both matched the FIRST one,
        // silently corrupting the file. The cursor is a byte offset into the
        // (rewritten) `content`; it always lands on a char boundary because it
        // is set to the end of a just-spliced, valid-UTF-8 replacement.
        let mut search_from = 0usize;
        for (i, hunk) in hunks.iter().enumerate() {
            let (old_text, new_text) = hunk_to_old_new(hunk);
            if old_text.is_empty() {
                // Pure addition with no context has no anchor to place it. Count
                // it and fail after the loop (below) rather than silently
                // dropping it while reporting success — the model must re-emit
                // with surrounding context or an EOF-anchored hunk.
                skipped_context_less += 1;
                continue;
            }
            // Search only the un-consumed tail `content[search_from..]`, then
            // translate any hit back into whole-`content` coordinates. Substring
            // `locate` is the fast path and carries the rich "why" diagnostic on
            // a miss. `locate_lines` is the codex `seek_sequence` line-anchored
            // matcher — a fallback when the substring search misses (whitespace /
            // indentation / CRLF drift), and the *primary* matcher for
            // EOF-anchored hunks, which must bind to the tail rather than a
            // head-first substring hit on an identical earlier block.
            let tail = &content[search_from..];
            let substring = locate(tail, &old_text);
            let rel_range = if hunk.eof_anchor {
                locate_lines(tail, &old_text, true).or_else(|| first_range(&substring))
            } else {
                match first_range(&substring) {
                    Some(r) => Some(r),
                    None => locate_lines(tail, &old_text, false),
                }
            };
            match rel_range {
                Some((s, e)) => {
                    let abs = (s + search_from, e + search_from);
                    content = apply_ranges(&content, &[abs], &new_text);
                    // Advance the cursor past the just-spliced replacement so the
                    // next hunk cannot rebind to it or anything before it.
                    search_from = abs.0 + new_text.len();
                    hunks_applied += 1;
                }
                None => {
                    let why = match &substring {
                        LocateResult::NotFound(w) => w.as_str(),
                        _ => "no matching location in file",
                    };
                    return fail(
                        "update",
                        path,
                        format!("hunk {}/{} did not match: {}", i + 1, hunks.len(), why),
                    );
                }
            }
        }

        // Any context-less pure addition could not be anchored. Reporting
        // success would tell the model its additions landed when they were
        // dropped — fail explicitly (whether or not other hunks applied) so it
        // re-emits with surrounding context or an EOF anchor.
        if skipped_context_less > 0 {
            return fail(
                "update",
                path,
                format!(
                    "{skipped_context_less} context-less addition hunk(s) could not be placed — \
                     include surrounding context lines or an EOF-anchored hunk so each addition \
                     has an anchor"
                ),
            );
        }
        if hunks_applied == 0 && !hunks.is_empty() {
            return fail(
                "update",
                path,
                "no hunks applied: context-less additions are not supported — \
                 include surrounding context lines or an EOF-anchored hunk"
                    .to_string(),
            );
        }

        // Atomic write-back: a crash mid-write must never truncate the file.
        if let Err(e) = crate::utils::atomic_write::atomic_write_file(&resolved, &content).await {
            return fail("update", path, format!("write-back failed: {e}"));
        }

        let final_path = if let Some(new_resolved) = move_dest {
            if let Some(parent) = new_resolved.parent() {
                if let Err(e) = tokio::fs::create_dir_all(parent).await {
                    return fail(
                        "update",
                        path,
                        format!(
                            "failed to create move-to parent {}: {}",
                            parent.display(),
                            e
                        ),
                    );
                }
            }
            if let Err(e) = tokio::fs::rename(&resolved, &new_resolved).await {
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
            message: format!("applied {hunks_applied} hunk(s)"),
        }
    }

    fn resolve(
        &self,
        path: &str,
        output_dir: Option<&Path>,
    ) -> std::result::Result<PathBuf, String> {
        if Path::new(path).is_absolute() {
            return Err(format!(
                "absolute path `{path}` is not allowed; apply_patch requires relative paths"
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
    const DESCRIPTION: &'static str = Self::DESCRIPTION;
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
    Add {
        path: String,
        lines: Vec<String>,
    },
    Delete {
        path: String,
    },
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
    // `eof_anchor` is consumed by `do_update`'s line-anchored fallback locator.
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
            "patch must start with `*** Begin Patch` (got `{first}`)"
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
                        let next = match lines.next() {
                            Some(v) => v,
                            None => return Err("Add File `{path}`: missing next line".to_string()),
                        };
                        let next = next.trim_end_matches('\r');
                        if let Some(stripped) = next.strip_prefix('+') {
                            buf.push(stripped.to_string());
                        } else if next.is_empty() {
                            // Empty lines inside Add File map to blank file lines.
                            buf.push(String::new());
                        } else {
                            return Err(format!(
                                "Add File `{path}`: expected `+...` line, got `{next}`"
                            ));
                        }
                    }
                    None => return Err(format!("Add File `{path}`: unterminated")),
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
                    Some(next)
                        if next.starts_with("*** ") && !next.starts_with("*** End of File") =>
                    {
                        break
                    }
                    Some(_) => {
                        let raw = match lines.next() {
                            Some(v) => v,
                            None => {
                                return Err("Update File `{path}`: missing hunk line".to_string())
                            }
                        };
                        let raw = raw.trim_end_matches('\r');
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
                            current_mut(&mut current)
                                .lines
                                .push(HunkLine::Context(c.to_string()));
                        } else if let Some(c) = raw.strip_prefix('-') {
                            current_mut(&mut current)
                                .lines
                                .push(HunkLine::Remove(c.to_string()));
                        } else if let Some(c) = raw.strip_prefix('+') {
                            current_mut(&mut current)
                                .lines
                                .push(HunkLine::Add(c.to_string()));
                        } else if raw.is_empty() {
                            // Blank lines inside hunks are valid context.
                            current_mut(&mut current)
                                .lines
                                .push(HunkLine::Context(String::new()));
                        } else {
                            return Err(format!(
                                "Update File `{path}`: unexpected line `{raw}` (each hunk line must start with ` `, `+`, or `-`)"
                            ));
                        }
                    }
                    None => return Err(format!("Update File `{path}`: unterminated")),
                }
            }
            if let Some(h) = current.take() {
                hunks.push(h);
            }
            if hunks.is_empty() {
                return Err(format!(
                    "Update File `{path}`: at least one `@@`-introduced hunk is required"
                ));
            }
            ops.push(PatchOp::Update {
                path: path.to_string(),
                move_to,
                hunks,
            });
        } else {
            return Err(format!(
                "expected file header (`*** Add File:`, `*** Update File:`, `*** Delete File:`) or `*** End Patch`, got `{line}`"
            ));
        }
    }

    if ops.is_empty() {
        return Err("patch contained no file operations".into());
    }
    Ok(ops)
}

/// The set of workspace paths a V4A patch envelope will touch — every
/// `Add` / `Delete` / `Update` target plus any `*** Move to:` destination.
///
/// This is the *blast radius* the concurrency scheduler needs to scope an
/// `apply_patch` call: unlike `file_write` / `file_edit` (one path argument),
/// `apply_patch` carries a multi-file patch body and *no* path field, so its
/// footprint can only be recovered by parsing the envelope. Reuses the same
/// [`parse_patch`] the executor runs, so the scheduled footprint can never
/// drift from the files actually mutated.
///
/// Returns an empty vec when the envelope does not parse. The caller
/// ([`crate::tools::adapters::registry_adapter`]) feeds that into
/// `ConcurrencyClaim::paths`, which degrades an empty set to a whole-world
/// (`Global`) claim — so a malformed patch serializes against everything
/// rather than inventing a wrong, narrow footprint.
pub(crate) fn patch_target_paths(patch: &str) -> Vec<String> {
    let Ok(ops) = parse_patch(patch) else {
        return Vec::new();
    };
    let mut paths = Vec::with_capacity(ops.len());
    for op in ops {
        match op {
            PatchOp::Add { path, .. } | PatchOp::Delete { path } => paths.push(path),
            PatchOp::Update { path, move_to, .. } => {
                paths.push(path);
                if let Some(dest) = move_to {
                    paths.push(dest);
                }
            }
        }
    }
    paths
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
    slot.as_mut()
        .unwrap_or_else(|| unreachable!("just initialised"))
}

// =============================================================================
// Helpers
// =============================================================================

/// First matched byte range of a substring [`LocateResult`], or `None` for a
/// miss. V4A hunks identify exactly one location per hunk, so only the first
/// occurrence is ever applied.
fn first_range(result: &LocateResult) -> Option<(usize, usize)> {
    match result {
        // Crlf is accepted as-is: the spliced hunk text keeps its LF newlines,
        // matching the long-standing behaviour of the `locate_lines` rstrip
        // fallback on CRLF files (see `edit_match::locate_lines`).
        LocateResult::Exact(ranges) | LocateResult::Folded(ranges) | LocateResult::Crlf(ranges) => {
            ranges.first().copied()
        }
        LocateResult::NotFound(_) => None,
    }
}

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
            PatchOp::Update {
                path,
                move_to,
                hunks,
            } => {
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
    fn target_paths_covers_every_op_and_move_destination() {
        // Add + Update-with-Move + Delete: the scheduler must see all four
        // touched paths (the Update contributes both its source and its
        // `*** Move to:` destination) so an overlapping concurrent patch or
        // file_edit is correctly serialized.
        let patch = "*** Begin Patch\n\
*** Add File: hello.txt\n\
+Hello\n\
*** Update File: src/app.py\n\
*** Move to: src/main.py\n\
@@ def greet():\n\
-print(\"Hi\")\n\
+print(\"Hello\")\n\
*** Delete File: old.txt\n\
*** End Patch\n";
        let paths = patch_target_paths(patch);
        assert_eq!(
            paths,
            vec![
                "hello.txt".to_string(),
                "src/app.py".to_string(),
                "src/main.py".to_string(),
                "old.txt".to_string(),
            ]
        );
    }

    #[test]
    fn target_paths_empty_for_unparseable_patch() {
        // An unparseable envelope yields no paths, so the caller degrades to a
        // whole-world (`Global`) claim rather than a wrong narrow footprint.
        assert!(patch_target_paths("not a patch").is_empty());
        assert!(patch_target_paths("").is_empty());
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

    #[tokio::test]
    async fn end_to_end_add_and_update_in_tempdir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let app_py = dir.path().join("app.py");
        tokio::fs::write(&app_py, "x = 1\ny = 2\nz = 3\n")
            .await
            .unwrap();

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
        let outcome = tool
            .do_update(app_py.to_str().unwrap(), None, &[hunk], Some(dir.path()))
            .await;
        // do_update will reject the absolute path; rerun with relative.
        assert!(!outcome.success);
        let outcome2 = tool
            .do_update(
                "app.py",
                None,
                &[Hunk {
                    header: None,
                    lines: vec![
                        HunkLine::Context("x = 1".into()),
                        HunkLine::Remove("y = 2".into()),
                        HunkLine::Add("y = 20".into()),
                        HunkLine::Context("z = 3".into()),
                    ],
                    eof_anchor: false,
                }],
                Some(dir.path()),
            )
            .await;
        assert!(outcome2.success, "{:?}", outcome2);

        let updated = tokio::fs::read_to_string(&app_py).await.unwrap();
        assert_eq!(updated, "x = 1\ny = 20\nz = 3\n");
    }

    #[tokio::test]
    async fn update_reports_unmatched_hunk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let app_py = dir.path().join("app.py");
        tokio::fs::write(&app_py, "a\nb\nc\n").await.unwrap();
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
        let outcome = tool
            .do_update("app.py", None, &[hunk], Some(dir.path()))
            .await;
        assert!(!outcome.success);
        assert!(outcome.message.contains("did not match"), "{:?}", outcome);
    }

    #[tokio::test]
    async fn update_applies_hunk_despite_trailing_whitespace_drift() {
        // The file's context lines carry trailing whitespace that the model's
        // patch omits. The contiguous-substring locator misses (the block is
        // broken by the `\n`-adjacent spaces); the codex-style line-anchored
        // fallback recovers the location and the edit lands.
        let dir = tempfile::tempdir().expect("tempdir");
        let app_py = dir.path().join("app.py");
        tokio::fs::write(&app_py, "x = 1   \ny = 2\nz = 3  \n")
            .await
            .unwrap();

        let tool = ApplyPatchTool::new();
        let outcome = tool
            .do_update(
                "app.py",
                None,
                &[Hunk {
                    header: None,
                    lines: vec![
                        HunkLine::Context("x = 1".into()),
                        HunkLine::Remove("y = 2".into()),
                        HunkLine::Add("y = 20".into()),
                        HunkLine::Context("z = 3".into()),
                    ],
                    eof_anchor: false,
                }],
                Some(dir.path()),
            )
            .await;
        assert!(outcome.success, "{:?}", outcome);
        let updated = tokio::fs::read_to_string(&app_py).await.unwrap();
        assert_eq!(updated, "x = 1\ny = 20\nz = 3\n");
    }

    #[tokio::test]
    async fn update_multi_hunk_forward_cursor_edits_distinct_occurrences() {
        // Two identical lines; two hunks whose replacement STILL contains the
        // old text. Without a forward cursor the second hunk rebinds to the
        // first (already-edited) occurrence and the file is corrupted; with the
        // cursor each hunk edits its own occurrence in order.
        let dir = tempfile::tempdir().expect("tempdir");
        let f = dir.path().join("f.txt");
        tokio::fs::write(&f, "a = a\na = a\n").await.unwrap();

        let tool = ApplyPatchTool::new();
        let hunk = |suffix: &str| Hunk {
            header: None,
            lines: vec![
                HunkLine::Remove("a = a".into()),
                HunkLine::Add(format!("a = a  # {suffix}")),
            ],
            eof_anchor: false,
        };
        let outcome = tool
            .do_update("f.txt", None, &[hunk("one"), hunk("two")], Some(dir.path()))
            .await;
        assert!(outcome.success, "{:?}", outcome);
        let updated = tokio::fs::read_to_string(&f).await.unwrap();
        assert_eq!(
            updated, "a = a  # one\na = a  # two\n",
            "each hunk must edit a distinct occurrence, in order"
        );
    }

    #[tokio::test]
    async fn update_fails_on_unplaceable_context_less_addition() {
        // A normal hunk plus a context-less pure-add hunk: the addition has no
        // anchor, so the whole update must fail explicitly (and leave the file
        // untouched) rather than report success while dropping the addition.
        let dir = tempfile::tempdir().expect("tempdir");
        let f = dir.path().join("f.txt");
        tokio::fs::write(&f, "keep\n").await.unwrap();

        let tool = ApplyPatchTool::new();
        let normal = Hunk {
            header: None,
            lines: vec![
                HunkLine::Remove("keep".into()),
                HunkLine::Add("kept".into()),
            ],
            eof_anchor: false,
        };
        let pure_add = Hunk {
            header: None,
            lines: vec![HunkLine::Add("appended".into())],
            eof_anchor: false,
        };
        let outcome = tool
            .do_update("f.txt", None, &[normal, pure_add], Some(dir.path()))
            .await;
        assert!(!outcome.success, "{:?}", outcome);
        assert!(
            outcome.message.contains("context-less"),
            "message must explain the unplaceable addition: {}",
            outcome.message
        );
        // No partial write — the file is byte-for-byte unchanged.
        let after = tokio::fs::read_to_string(&f).await.unwrap();
        assert_eq!(after, "keep\n", "a failed update must not mutate the file");
    }

    #[tokio::test]
    async fn update_eof_anchor_targets_file_tail() {
        // Identical sentinel blocks at head and tail; the EOF anchor must make
        // the fallback locator edit the trailing one. (Whitespace drift on the
        // context line forces the line-anchored path.)
        let dir = tempfile::tempdir().expect("tempdir");
        let f = dir.path().join("log.txt");
        tokio::fs::write(&f, "end \nmiddle\nend \n").await.unwrap();

        let tool = ApplyPatchTool::new();
        let outcome = tool
            .do_update(
                "log.txt",
                None,
                &[Hunk {
                    header: None,
                    lines: vec![HunkLine::Remove("end".into()), HunkLine::Add("FIN".into())],
                    eof_anchor: true,
                }],
                Some(dir.path()),
            )
            .await;
        assert!(outcome.success, "{:?}", outcome);
        let updated = tokio::fs::read_to_string(&f).await.unwrap();
        assert_eq!(updated, "end \nmiddle\nFIN\n");
    }
}
