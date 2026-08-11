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
//! - **Two-phase application.** The whole envelope is parsed, then every op
//!   is resolved and its final content computed in memory, and only then is
//!   anything written. A hunk that misses on the LAST file therefore leaves
//!   the EARLIER files untouched, so the model can resend the same envelope
//!   verbatim instead of working out which half already landed.
//!
//! [codex-apply-patch]: https://github.com/openai/codex/tree/main/codex-rs/apply-patch

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use super::edit_match::{apply_ranges, locate, locate_lines, LocateResult};
use super::path_utils::{check_and_resolve_path, get_denied_paths, resolve_for_removal};
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

/// Hard upper bound on the patch envelope (UTF-8 bytes). The whole envelope is
/// parsed and held in memory before any write, so an unbounded input is a
/// memory-exhaustion vector the model can be steered into. 4 MiB is well over
/// any legitimate V4A patch (the realistic ceiling is single-digit KB) while
/// still leaving room for large refactors. The error message points the model
/// at `file_write` / `file_edit` for bulk rewrites that genuinely need more.
const MAX_PATCH_BYTES: usize = 4 * 1024 * 1024;

/// Maximum number of file ops (Add/Update/Delete headers) in a single envelope.
/// Each op is materialised into a `PatchOp` + body before commit; an
/// unbounded count lets one envelope hold a million Add ops and balloon
/// resident memory. 500 is well past anything a legitimate refactor needs.
const MAX_PATCH_OPS: usize = 500;

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
        // Bound the input first: parse_patch builds a `Vec<PatchOp>` plus per-op
        // `lines: Vec<Line>` entirely in memory, so a runaway envelope is a
        // memory-exhaustion vector the model can be steered into.
        if args.patch.len() > MAX_PATCH_BYTES {
            return Err(ToolError::InvalidArgs(format!(
                "apply_patch envelope is {} bytes; the cap is {MAX_PATCH_BYTES} bytes. \
                 Split into multiple `apply_patch` calls or use `file_write`/`file_edit` for bulk rewrites.",
                args.patch.len()
            )));
        }
        let op_headers = args
            .patch
            .matches("\n*** Add File:")
            .count()
            + args.patch.matches("\n*** Update File:").count()
            + args.patch.matches("\n*** Delete File:").count();
        if op_headers > MAX_PATCH_OPS {
            return Err(ToolError::InvalidArgs(format!(
                "apply_patch envelope declares {op_headers} file ops; the cap is \
                 {MAX_PATCH_OPS}. Split the change into multiple envelopes."
            )));
        }
        let ops = parse_patch(&args.patch).map_err(ToolError::InvalidArgs)?;

        let output_dir = self.resolve_output_dir().await;
        let output_dir_ref = output_dir.as_deref();

        let total = ops.len();
        let (all_ok, outcomes) = self.execute(&ops, output_dir_ref).await;
        let applied = outcomes.iter().filter(|o| o.success).count();

        let message = if all_ok {
            format!("applied {applied} file operation(s)")
        } else if applied == 0 {
            // The envelope is resolved in full before anything is written, so a
            // rejected op leaves every file untouched — the model fixes the one
            // reported op and resends the same patch.
            format!("nothing written: 1 of {total} operation(s) could not be applied")
        } else {
            // Only an I/O fault no planning could predict gets this far.
            format!("applied {applied}/{total} operations; a write failed partway")
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

    /// Resolve every op and compute its final content, then write. Writing
    /// op-by-op left the EARLIER files already rewritten when a LATER hunk
    /// missed, and the model's natural whole-envelope retry then failed
    /// *differently* — hunk 1 no longer matched its own edit.
    async fn execute(
        &self,
        ops: &[PatchOp],
        output_dir: Option<&Path>,
    ) -> (bool, Vec<FileOutcome>) {
        // Held from the first read until the last write: the plan is computed
        // against content that must not change underneath it.
        let _guards = self.lock_all(ops, output_dir).await;

        let mut pending: Pending = HashMap::new();
        let mut plans: Vec<Planned> = Vec::with_capacity(ops.len());
        for op in ops {
            match self.plan_one(op, output_dir, &mut pending).await {
                Ok(plan) => plans.push(plan),
                // Nothing has been written, so every file is untouched — report
                // only the op that could not be applied. Claiming the earlier
                // ops succeeded would send the model retrying a remainder that
                // does not exist.
                Err(outcome) => return (false, vec![outcome]),
            }
        }

        let mut outcomes: Vec<FileOutcome> = Vec::with_capacity(plans.len());
        let mut all_ok = true;
        for plan in plans {
            let outcome = plan.commit().await;
            if !outcome.success {
                all_ok = false;
            }
            outcomes.push(outcome);
            // Only an I/O fault gets here; stop rather than pile more writes
            // onto a filesystem that just refused one.
            if !all_ok {
                break;
            }
        }
        (all_ok, outcomes)
    }

    /// Take the write lock for every path the envelope touches, in sorted
    /// order.
    ///
    /// The locks span planning *and* committing, so a concurrent `file_edit`
    /// cannot land between a hunk's read and its write-back. Sorting is what
    /// keeps two patches with crossed paths from ABBA-deadlocking (the
    /// discipline [`crate::tools::path_locks::lock_path_pair`] documents).
    /// Paths that fail to resolve are skipped — `plan_one` reports them, and it
    /// does so before any write.
    async fn lock_all(
        &self,
        ops: &[PatchOp],
        output_dir: Option<&Path>,
    ) -> Vec<tokio::sync::OwnedMutexGuard<()>> {
        let mut paths: Vec<PathBuf> = Vec::with_capacity(ops.len());
        let mut push = |resolved: std::result::Result<PathBuf, String>| {
            if let Ok(p) = resolved {
                paths.push(p);
            }
        };
        for op in ops {
            match op {
                PatchOp::Add { path, .. } => push(self.resolve(path, output_dir)),
                PatchOp::Delete { path } => {
                    push(self.resolve(path, output_dir));
                    push(self.resolve_unfollowed(path, output_dir));
                }
                PatchOp::Update {
                    path, move_to: mv, ..
                } => {
                    push(self.resolve(path, output_dir));
                    if let Some(dest) = mv {
                        push(self.resolve(dest, output_dir));
                        push(self.resolve_unfollowed(path, output_dir));
                    }
                }
            }
        }
        // Dedup matters for correctness, not just tidiness: re-acquiring the
        // same async mutex from one task deadlocks.
        paths.sort();
        paths.dedup();
        let mut guards = Vec::with_capacity(paths.len());
        for path in &paths {
            guards.push(crate::tools::path_locks::lock_path(path).await);
        }
        guards
    }

    async fn plan_one(
        &self,
        op: &PatchOp,
        output_dir: Option<&Path>,
        pending: &mut Pending,
    ) -> std::result::Result<Planned, FileOutcome> {
        match op {
            PatchOp::Add { path, lines } => self.plan_add(path, lines, output_dir, pending),
            PatchOp::Delete { path } => self.plan_delete(path, output_dir, pending),
            PatchOp::Update {
                path,
                move_to,
                hunks,
            } => {
                self.plan_update(path, move_to.as_deref(), hunks, output_dir, pending)
                    .await
            }
        }
    }

    fn plan_add(
        &self,
        path: &str,
        lines: &[String],
        output_dir: Option<&Path>,
        pending: &mut Pending,
    ) -> std::result::Result<Planned, FileOutcome> {
        let resolved = self
            .resolve(path, output_dir)
            .map_err(|msg| fail("add", path, msg))?;
        if exists_after_pending(&resolved, pending) {
            return Err(fail(
                "add",
                path,
                format!(
                    "{} already exists — use `*** Update File:` instead",
                    resolved.display()
                ),
            ));
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
        pending.insert(resolved.clone(), Some(body.clone()));
        Ok(Planned {
            src: path.to_string(),
            effect: Effect::Add {
                path: resolved,
                body,
            },
        })
    }

    fn plan_delete(
        &self,
        path: &str,
        output_dir: Option<&Path>,
        pending: &mut Pending,
    ) -> std::result::Result<Planned, FileOutcome> {
        // A final-component symlink must be unlinked, not followed: `remove_file`
        // never follows one, so handing it the canonical target destroys the
        // pointed-at file and leaves the link dangling — data loss on a path the
        // model only asked to unlink.
        let resolved = self
            .resolve_unfollowed(path, output_dir)
            .map_err(|msg| fail("delete", path, msg))?;
        if !exists_after_pending(&resolved, pending) {
            return Err(fail(
                "delete",
                path,
                format!("{} does not exist", resolved.display()),
            ));
        }
        pending.insert(resolved.clone(), None);
        Ok(Planned {
            src: path.to_string(),
            effect: Effect::Delete { path: resolved },
        })
    }

    async fn plan_update(
        &self,
        path: &str,
        move_to: Option<&str>,
        hunks: &[Hunk],
        output_dir: Option<&Path>,
        pending: &mut Pending,
    ) -> std::result::Result<Planned, FileOutcome> {
        let resolved = self
            .resolve(path, output_dir)
            .map_err(|msg| fail("update", path, msg))?;
        // Resolve the move-to destination BEFORE applying any hunk, so a
        // rejected destination fails here rather than after the edit is
        // computed. The rename SOURCE must not follow a final-component
        // symlink: `rename` never follows one, so handing it the canonical
        // target moves the real file out from under the link instead of moving
        // the link. The write-back keeps targeting the canonical file, so the
        // patched text lands where the link points.
        let rename: Option<(PathBuf, PathBuf)> = match move_to {
            Some(new_path) => {
                let dest = self
                    .resolve(new_path, output_dir)
                    .map_err(|msg| fail("update", path, format!("move-to path rejected: {msg}")))?;
                let src = self
                    .resolve_unfollowed(path, output_dir)
                    .map_err(|msg| fail("update", path, msg))?;
                Some((src, dest))
            }
            None => None,
        };

        let mut content = match pending.get(&resolved) {
            Some(Some(text)) => text.clone(),
            Some(None) => {
                return Err(fail(
                    "update",
                    path,
                    format!("{} does not exist", resolved.display()),
                ))
            }
            None => read_text(&resolved, path).await?,
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
            // `@@ <header>` names the section the hunk belongs to. A hunk whose
            // `-` lines also occur in an EARLIER section otherwise binds to that
            // earlier occurrence and reports success — the edit lands in the
            // wrong function. The header is sought only in the un-consumed tail,
            // so the anchor can never drag the cursor back over an applied hunk,
            // and the search resumes AT the header line (not after it) so a hunk
            // that repeats the header as its first context line still matches.
            // An unfindable header is no anchor at all: fall through to the
            // whole tail rather than fail a patch whose body is perfectly good.
            let hunk_from = hunk
                .header
                .as_deref()
                .and_then(|header| locate_lines(&content[search_from..], header, false))
                .map_or(search_from, |(s, _)| search_from + s);
            // Search only the un-consumed tail `content[hunk_from..]`, then
            // translate any hit back into whole-`content` coordinates. Substring
            // `locate` is the fast path and carries the rich "why" diagnostic on
            // a miss. `locate_lines` is the codex `seek_sequence` line-anchored
            // matcher — a fallback when the substring search misses (whitespace /
            // indentation / CRLF drift), and the *primary* matcher for
            // EOF-anchored hunks, which must bind to the tail rather than a
            // head-first substring hit on an identical earlier block.
            let tail = &content[hunk_from..];
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
                    let abs = (s + hunk_from, e + hunk_from);
                    // The located block uses CRLF while every hunk line is
                    // LF-only (the parser strips `\r`), so splicing the
                    // replacement verbatim writes mixed line endings into a
                    // Windows file — the contract `LocateResult::Crlf` documents
                    // and `file_edit` honours. Both routes land here: the CRLF
                    // substring pass, and `locate_lines`' rstrip pass. The
                    // latter needs one extra step: it matches a `\r`-ended line
                    // by trimming, but the span it returns *includes* that `\r`
                    // while leaving the `\n` outside — so the final line's
                    // carriage return is consumed by the splice and has to be
                    // re-emitted, or only the last line silently converts to LF.
                    let matched = &content[abs.0..abs.1];
                    let trailing_cr = matched.ends_with('\r');
                    let replacement =
                        if (matched.contains("\r\n") || trailing_cr) && !new_text.contains('\r') {
                            let mut crlf = new_text.replace('\n', "\r\n");
                            if trailing_cr {
                                crlf.push('\r');
                            }
                            Cow::Owned(crlf)
                        } else {
                            Cow::Borrowed(new_text.as_str())
                        };
                    content = apply_ranges(&content, &[abs], &replacement);
                    // Advance the cursor past the just-spliced replacement so the
                    // next hunk cannot rebind to it or anything before it.
                    search_from = abs.0 + replacement.len();
                    hunks_applied += 1;
                }
                None => {
                    let why = match &substring {
                        LocateResult::NotFound(w) => w.as_str(),
                        _ => "no matching location in file",
                    };
                    return Err(fail(
                        "update",
                        path,
                        format!("hunk {}/{} did not match: {}", i + 1, hunks.len(), why),
                    ));
                }
            }
        }

        // Any context-less pure addition could not be anchored. Reporting
        // success would tell the model its additions landed when they were
        // dropped — fail explicitly (whether or not other hunks applied) so it
        // re-emits with surrounding context or an EOF anchor.
        if skipped_context_less > 0 {
            return Err(fail(
                "update",
                path,
                format!(
                    "{skipped_context_less} context-less addition hunk(s) could not be placed — \
                     include surrounding context lines or an EOF-anchored hunk so each addition \
                     has an anchor"
                ),
            ));
        }
        if hunks_applied == 0 && !hunks.is_empty() {
            return Err(fail(
                "update",
                path,
                "no hunks applied: context-less additions are not supported — \
                 include surrounding context lines or an EOF-anchored hunk"
                    .to_string(),
            ));
        }

        pending.insert(resolved.clone(), Some(content.clone()));
        if let Some((src, dest)) = rename.as_ref() {
            // The rename empties the source and fills the destination. When the
            // source is a symlink, the canonical file it points at keeps the
            // patched text — which is exactly why the source key is the
            // un-followed one and the write-back key is not.
            pending.insert(src.clone(), None);
            pending.insert(dest.clone(), Some(content.clone()));
        }

        Ok(Planned {
            src: path.to_string(),
            effect: Effect::Update {
                write_to: resolved,
                content,
                rename,
                hunks_applied,
            },
        })
    }

    fn resolve(
        &self,
        path: &str,
        output_dir: Option<&Path>,
    ) -> std::result::Result<PathBuf, String> {
        self.resolve_via(check_and_resolve_path, path, output_dir)
    }

    /// As [`Self::resolve`], but the final component is left unfollowed when it
    /// is a symlink — the flavour `remove_file` / `rename` need, since neither
    /// follows a final symlink and both would otherwise act on the pointed-at
    /// file.
    fn resolve_unfollowed(
        &self,
        path: &str,
        output_dir: Option<&Path>,
    ) -> std::result::Result<PathBuf, String> {
        self.resolve_via(resolve_for_removal, path, output_dir)
    }

    fn resolve_via(
        &self,
        resolver: fn(&Path, &[String], Option<&Path>) -> std::result::Result<PathBuf, ToolError>,
        path: &str,
        output_dir: Option<&Path>,
    ) -> std::result::Result<PathBuf, String> {
        if Path::new(path).is_absolute() {
            return Err(format!(
                "absolute path `{path}` is not allowed; apply_patch requires relative paths"
            ));
        }
        resolver(Path::new(path), &self.denied_paths, output_dir).map_err(|e| e.to_string())
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
// Planned effects
// =============================================================================

/// What each resolved path holds once every op planned so far has run — `None`
/// for a path an op removes. Planning consults this instead of the filesystem,
/// because the writes have not happened yet: `*** Add File: x` followed by
/// `*** Update File: x` is a legal envelope, and the update must patch the
/// pending body rather than the file on disk.
type Pending = HashMap<PathBuf, Option<String>>;

/// Does `path` exist once every op planned so far has run?
fn exists_after_pending(path: &Path, pending: &Pending) -> bool {
    match pending.get(path) {
        Some(state) => state.is_some(),
        None => path.exists(),
    }
}

/// One op's filesystem effect, fully computed but not yet performed.
struct Planned {
    /// The path as the patch spelled it. Failure outcomes name it rather than
    /// the resolved form, because that is the string the model can correct.
    src: String,
    effect: Effect,
}

enum Effect {
    Add {
        path: PathBuf,
        body: String,
    },
    Delete {
        path: PathBuf,
    },
    Update {
        write_to: PathBuf,
        content: String,
        /// `(source, destination)` for `*** Move to:`; the source is resolved
        /// without following a final-component symlink.
        rename: Option<(PathBuf, PathBuf)>,
        hunks_applied: usize,
    },
}

impl Planned {
    async fn commit(self) -> FileOutcome {
        let Planned { src, effect } = self;
        match effect {
            Effect::Add { path, body } => {
                if let Some(parent) = path.parent() {
                    if let Err(e) = tokio::fs::create_dir_all(parent).await {
                        return fail(
                            "add",
                            &src,
                            format!("failed to create parent {}: {}", parent.display(), e),
                        );
                    }
                }
                // Atomic write-back (stage to temp + fsync + rename), matching
                // the durability guarantee of file_edit/file_write — a crash
                // mid-write must never leave the file truncated.
                let byte_count = body.len();
                match crate::utils::atomic_write::atomic_write_file(&path, &body).await {
                    Ok(()) => FileOutcome {
                        path: path.display().to_string(),
                        op: "add",
                        success: true,
                        message: format!("created ({byte_count} bytes)"),
                    },
                    Err(e) => fail("add", &src, format!("write failed: {e}")),
                }
            }
            Effect::Delete { path } => match tokio::fs::remove_file(&path).await {
                Ok(()) => FileOutcome {
                    path: path.display().to_string(),
                    op: "delete",
                    success: true,
                    message: "deleted".into(),
                },
                Err(e) => fail("delete", &src, format!("delete failed: {e}")),
            },
            Effect::Update {
                write_to,
                content,
                rename,
                hunks_applied,
            } => {
                if let Err(e) =
                    crate::utils::atomic_write::atomic_write_file(&write_to, &content).await
                {
                    return fail("update", &src, format!("write-back failed: {e}"));
                }
                let final_path = match rename {
                    Some((from, to)) => {
                        if let Some(parent) = to.parent() {
                            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                                return fail(
                                    "update",
                                    &src,
                                    format!(
                                        "failed to create move-to parent {}: {}",
                                        parent.display(),
                                        e
                                    ),
                                );
                            }
                        }
                        if let Err(e) = tokio::fs::rename(&from, &to).await {
                            return fail(
                                "update",
                                &src,
                                format!(
                                    "rename {} → {} failed: {}",
                                    from.display(),
                                    to.display(),
                                    e
                                ),
                            );
                        }
                        to
                    }
                    None => write_to,
                };
                FileOutcome {
                    path: final_path.display().to_string(),
                    op: "update",
                    success: true,
                    message: format!("applied {hunks_applied} hunk(s)"),
                }
            }
        }
    }
}

/// Read `resolved` as text, refusing binary / non-UTF-8 content — mirrors
/// `file_edit::read_text_file`. `src` is the path as the patch spelled it.
async fn read_text(resolved: &Path, src: &str) -> std::result::Result<String, FileOutcome> {
    if !resolved.exists() {
        return Err(fail(
            "update",
            src,
            format!("{} does not exist", resolved.display()),
        ));
    }
    let bytes = tokio::fs::read(resolved)
        .await
        .map_err(|e| fail("update", src, format!("read failed: {e}")))?;
    if is_binary(&bytes) {
        return Err(fail(
            "update",
            src,
            format!(
                "{} is binary — apply_patch only edits text",
                resolved.display()
            ),
        ));
    }
    String::from_utf8(bytes).map_err(|_| {
        fail(
            "update",
            src,
            format!("{} is not valid UTF-8 text", resolved.display()),
        )
    })
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
    // `eof_anchor` is consumed by `plan_update`'s line-anchored fallback locator.
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
///
/// Every hit variant is treated alike: whether the replacement needs CRLF
/// newlines is read off the located span in `plan_update`, which also covers
/// the `locate_lines` fallback that never produces a [`LocateResult`] at all.
fn first_range(result: &LocateResult) -> Option<(usize, usize)> {
    match result {
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
        let outcome = run_update(
            &tool,
            dir.path(),
            app_py.to_str().unwrap(),
            None,
            vec![hunk],
        )
        .await;
        // The executor will reject the absolute path; rerun with relative.
        assert!(!outcome.success);
        let outcome2 = run_update(
            &tool,
            dir.path(),
            "app.py",
            None,
            vec![Hunk {
                header: None,
                lines: vec![
                    HunkLine::Context("x = 1".into()),
                    HunkLine::Remove("y = 2".into()),
                    HunkLine::Add("y = 20".into()),
                    HunkLine::Context("z = 3".into()),
                ],
                eof_anchor: false,
            }],
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
        let outcome = run_update(&tool, dir.path(), "app.py", None, vec![hunk]).await;
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
        let outcome = run_update(
            &tool,
            dir.path(),
            "app.py",
            None,
            vec![Hunk {
                header: None,
                lines: vec![
                    HunkLine::Context("x = 1".into()),
                    HunkLine::Remove("y = 2".into()),
                    HunkLine::Add("y = 20".into()),
                    HunkLine::Context("z = 3".into()),
                ],
                eof_anchor: false,
            }],
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
        let outcome = run_update(
            &tool,
            dir.path(),
            "f.txt",
            None,
            vec![hunk("one"), hunk("two")],
        )
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
        let outcome = run_update(&tool, dir.path(), "f.txt", None, vec![normal, pure_add]).await;
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

    /// Drive one `Update` op through the real executor — the same path `run`
    /// takes, minus the envelope parsing.
    async fn run_update(
        tool: &ApplyPatchTool,
        dir: &Path,
        path: &str,
        move_to: Option<&str>,
        hunks: Vec<Hunk>,
    ) -> FileOutcome {
        let ops = vec![PatchOp::Update {
            path: path.to_string(),
            move_to: move_to.map(str::to_string),
            hunks,
        }];
        let (_, mut outcomes) = tool.execute(&ops, Some(dir)).await;
        outcomes.pop().expect("execute reports one outcome per op")
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn delete_of_symlink_unlinks_the_link_not_its_target() {
        // `check_and_resolve_path` canonicalizes a final-component symlink to
        // its target, so deleting through the link would destroy the pointed-at
        // file and leave the link behind — data loss on a path the user only
        // asked to unlink.
        let dir = tempfile::tempdir().expect("tempdir");
        let real = dir.path().join("real.txt");
        tokio::fs::write(&real, "payload\n").await.unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let tool = ApplyPatchTool::new();
        let (ok, outcomes) = tool
            .execute(
                &[PatchOp::Delete {
                    path: "link".to_string(),
                }],
                Some(dir.path()),
            )
            .await;
        assert!(ok, "{outcomes:?}");
        assert!(
            link.symlink_metadata().is_err(),
            "the link itself must be unlinked"
        );
        assert_eq!(
            tokio::fs::read_to_string(&real).await.unwrap(),
            "payload\n",
            "the link's target must survive a delete of the link"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn move_to_on_a_symlink_renames_the_link_not_its_target() {
        // `rename` never follows a final symlink, so handing it the canonical
        // target moves the real file out from under the link instead of moving
        // the link. The write-back still goes *through* the link — only the
        // rename must not follow it.
        let dir = tempfile::tempdir().expect("tempdir");
        let real = dir.path().join("real.txt");
        tokio::fs::write(&real, "old\n").await.unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let outcome = run_update(
            &ApplyPatchTool::new(),
            dir.path(),
            "link",
            Some("renamed"),
            vec![Hunk {
                header: None,
                lines: vec![HunkLine::Remove("old".into()), HunkLine::Add("new".into())],
                eof_anchor: false,
            }],
        )
        .await;
        assert!(outcome.success, "{outcome:?}");
        assert_eq!(
            tokio::fs::read_to_string(&real).await.unwrap(),
            "new\n",
            "the write-back follows the link, so the target holds the patched text"
        );
        assert!(link.symlink_metadata().is_err(), "the link must have moved");
        assert!(
            dir.path().join("renamed").symlink_metadata().is_ok(),
            "the destination must exist"
        );
    }

    #[tokio::test]
    async fn crlf_file_hunk_keeps_crlf_line_endings() {
        // The substring locator bridges the LF hunk to the CRLF file, but the
        // spliced replacement carries bare LF newlines — writing mixed line
        // endings into a Windows file (the contract `LocateResult::Crlf`
        // documents and `file_edit` honours).
        let dir = tempfile::tempdir().expect("tempdir");
        let f = dir.path().join("win.txt");
        tokio::fs::write(&f, "x = 1\r\ny = 2\r\n").await.unwrap();

        let outcome = run_update(
            &ApplyPatchTool::new(),
            dir.path(),
            "win.txt",
            None,
            vec![Hunk {
                header: None,
                lines: vec![
                    HunkLine::Remove("x = 1".into()),
                    HunkLine::Remove("y = 2".into()),
                    HunkLine::Add("x = 10".into()),
                    HunkLine::Add("y = 20".into()),
                ],
                eof_anchor: false,
            }],
        )
        .await;
        assert!(outcome.success, "{outcome:?}");
        assert_eq!(
            tokio::fs::read_to_string(&f).await.unwrap(),
            "x = 10\r\ny = 20\r\n"
        );
    }

    #[tokio::test]
    async fn crlf_file_line_anchored_fallback_keeps_crlf_line_endings() {
        // Trailing-whitespace drift defeats the substring locator, so the match
        // comes from `locate_lines`' rstrip pass — which matches a `\r`-ended
        // line while the returned span still *includes* that `\r`. A bare-LF
        // replacement therefore silently converts the block to LF.
        let dir = tempfile::tempdir().expect("tempdir");
        let f = dir.path().join("win.txt");
        tokio::fs::write(&f, "x = 1  \r\ny = 2\r\n").await.unwrap();

        let outcome = run_update(
            &ApplyPatchTool::new(),
            dir.path(),
            "win.txt",
            None,
            vec![Hunk {
                header: None,
                lines: vec![
                    HunkLine::Remove("x = 1".into()),
                    HunkLine::Remove("y = 2".into()),
                    HunkLine::Add("x = 10".into()),
                    HunkLine::Add("y = 20".into()),
                ],
                eof_anchor: false,
            }],
        )
        .await;
        assert!(outcome.success, "{outcome:?}");
        assert_eq!(
            tokio::fs::read_to_string(&f).await.unwrap(),
            "x = 10\r\ny = 20\r\n"
        );
    }

    #[tokio::test]
    async fn failed_hunk_on_a_later_file_leaves_earlier_files_untouched() {
        // Writing op-by-op leaves the earlier files already rewritten when a
        // later hunk misses — and the model's natural whole-envelope retry then
        // fails *differently*, because hunk 1 no longer matches its own edit.
        let dir = tempfile::tempdir().expect("tempdir");
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        tokio::fs::write(&a, "alpha\n").await.unwrap();
        tokio::fs::write(&b, "beta\n").await.unwrap();

        let ops = vec![
            PatchOp::Update {
                path: "a.txt".to_string(),
                move_to: None,
                hunks: vec![Hunk {
                    header: None,
                    lines: vec![
                        HunkLine::Remove("alpha".into()),
                        HunkLine::Add("ALPHA".into()),
                    ],
                    eof_anchor: false,
                }],
            },
            PatchOp::Update {
                path: "b.txt".to_string(),
                move_to: None,
                hunks: vec![Hunk {
                    header: None,
                    lines: vec![
                        HunkLine::Remove("absent".into()),
                        HunkLine::Add("NOPE".into()),
                    ],
                    eof_anchor: false,
                }],
            },
        ];
        let (ok, outcomes) = ApplyPatchTool::new().execute(&ops, Some(dir.path())).await;
        assert!(!ok, "{outcomes:?}");
        let failure = outcomes
            .iter()
            .find(|o| !o.success)
            .expect("the miss must be reported");
        assert_eq!(failure.path, "b.txt", "the error must still name B");
        assert!(failure.message.contains("did not match"), "{failure:?}");
        assert_eq!(
            tokio::fs::read_to_string(&a).await.unwrap(),
            "alpha\n",
            "a patch that cannot fully apply must not half-apply"
        );
    }

    #[tokio::test]
    async fn a_later_op_sees_what_an_earlier_op_in_the_same_patch_wrote() {
        // Resolving the whole envelope before writing must not make the ops
        // blind to each other: `Add File: x` followed by `Update File: x` is a
        // legal envelope, and the update has to patch the pending body rather
        // than the on-disk file the add has not reached yet.
        let dir = tempfile::tempdir().expect("tempdir");
        let ops = vec![
            PatchOp::Add {
                path: "new.txt".to_string(),
                lines: vec!["alpha".to_string()],
            },
            PatchOp::Update {
                path: "new.txt".to_string(),
                move_to: None,
                hunks: vec![Hunk {
                    header: None,
                    lines: vec![
                        HunkLine::Remove("alpha".into()),
                        HunkLine::Add("ALPHA".into()),
                    ],
                    eof_anchor: false,
                }],
            },
        ];
        let (ok, outcomes) = ApplyPatchTool::new().execute(&ops, Some(dir.path())).await;
        assert!(ok, "{outcomes:?}");
        assert_eq!(
            tokio::fs::read_to_string(dir.path().join("new.txt"))
                .await
                .unwrap(),
            "ALPHA\n"
        );
    }

    #[tokio::test]
    async fn hunk_header_anchors_the_search_to_its_section() {
        // Both functions end in the same line. Without using the `@@ <header>`
        // the hunk binds to the FIRST occurrence and reports success — the edit
        // lands in the wrong function.
        let dir = tempfile::tempdir().expect("tempdir");
        let f = dir.path().join("m.py");
        tokio::fs::write(&f, "def foo():\n    return 1\n\ndef bar():\n    return 1\n")
            .await
            .unwrap();

        let outcome = run_update(
            &ApplyPatchTool::new(),
            dir.path(),
            "m.py",
            None,
            vec![Hunk {
                header: Some("def bar():".to_string()),
                lines: vec![
                    HunkLine::Remove("    return 1".into()),
                    HunkLine::Add("    return 2".into()),
                ],
                eof_anchor: false,
            }],
        )
        .await;
        assert!(outcome.success, "{outcome:?}");
        assert_eq!(
            tokio::fs::read_to_string(&f).await.unwrap(),
            "def foo():\n    return 1\n\ndef bar():\n    return 2\n"
        );
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
        let outcome = run_update(
            &tool,
            dir.path(),
            "log.txt",
            None,
            vec![Hunk {
                header: None,
                lines: vec![HunkLine::Remove("end".into()), HunkLine::Add("FIN".into())],
                eof_anchor: true,
            }],
        )
        .await;
        assert!(outcome.success, "{:?}", outcome);
        let updated = tokio::fs::read_to_string(&f).await.unwrap();
        assert_eq!(updated, "end \nmiddle\nFIN\n");
    }
}
