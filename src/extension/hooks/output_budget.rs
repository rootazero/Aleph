//! Budget for hook-injected context.
//!
//! # Why
//!
//! A hook signals "give this text to the model next turn" by printing
//! `context: <text>` (or a JSON `additionalContext`), which lands in
//! [`HookResult::additional_contexts`](super::HookResult::additional_contexts).
//! Four seams consume that list and splice it into the prompt:
//!
//! | seam | lands in |
//! |---|---|
//! | `SessionStart` (`run_loop/inner.rs`) | session context, **every turn after** |
//! | `UserPromptSubmit` (`run_loop/inner.rs`) | that run's trailing `<system-reminder>` blocks |
//! | `Before/AfterToolCall` (`tools/scoped/dispatch.rs`) | the tool result the model reads |
//! | `Before/AfterCompaction` (`memory/session_compactor/`) | the compaction input |
//!
//! None of them bounded it. Only the *bare stdout* fallback had a cap, and
//! that one was a private 4KB constant inside `run_loop/inner.rs` — so a hook
//! doing `context: $(cat build.log)` could push ~64KB (the stdout read cap)
//! into **every subsequent turn** of the session, blowing out the prompt-cache
//! prefix and crowding out real context.
//!
//! # What this does
//!
//! One policy for every seam: text under [`HOOK_CONTEXT_TOKEN_LIMIT`] passes
//! through untouched; oversized text is written in full to
//! `~/.aleph/data/hook_outputs/<session>/<uuid>.txt` and replaced with a
//! head/tail preview plus the path, so the model can still `file_read` the
//! whole thing when it actually needs it. Spilling (rather than discarding)
//! is what codex (`HookOutputSpiller`) and hermes (`hook_output_spill.py`,
//! itself a port of codex) both do; the previous behaviour threw the tail away.
//!
//! # Invariants
//!
//! - **Never fails.** A disk error degrades to an in-place truncation with a
//!   notice. A hook's context reaching the model in bounded form always beats
//!   failing the turn.
//! - **Behaviour-preserving under the limit.** Short contexts are returned
//!   byte-identical, so the common case is untouched.
//! - **Not a security boundary.** The fail-closed 64KB stdout cap in
//!   `executor.rs` is what protects the *decision* protocol. This budget sits
//!   downstream of it and only governs how much text reaches the model.

use std::path::{Path, PathBuf};

use tracing::warn;

/// Token budget for a single injected context block.
///
/// Sized to codex's `DEFAULT_HOOK_OUTPUT_TOKEN_LIMIT` (2 500). A hook that
/// wants to say more than a couple thousand tokens per fire is reporting, not
/// steering — and reporting belongs in a spill file the model can choose to
/// open.
const HOOK_CONTEXT_TOKEN_LIMIT: usize = 2_500;

/// Characters kept from the start of an over-budget context.
const PREVIEW_HEAD_CHARS: usize = 1_500;

/// Characters kept from the end of an over-budget context. A hook's verdict
/// usually lands last (a script echoes its log, then its conclusion), so the
/// tail is worth as much as the head.
const PREVIEW_TAIL_CHARS: usize = 1_000;

/// Directory under `~/.aleph/data/` holding spilled hook output.
const SPILL_DIR: &str = "hook_outputs";

/// Spill files retained per session before the oldest are reaped. Each is at
/// most the 64KB stdout cap, so this bounds a session at a few MB — enough
/// history for the model to look back over a run, bounded enough that a
/// per-tool-call hook cannot fill the disk over months of use.
const MAX_SPILL_FILES_PER_SESSION: usize = 32;

/// Join a hook's plain-stdout lines into a single context block.
///
/// Bare stdout counts as injected context on the `UserPromptSubmit` and
/// `SessionStart` seams (Claude-Code convention). Returns `None` when there is
/// nothing to say. Deliberately **uncapped** — bounding is
/// [`budget_hook_contexts`]'s job, so the two kinds of hook output share one
/// policy instead of the two divergent limits that existed before.
#[must_use]
pub fn join_messages(messages: &[String]) -> Option<String> {
    let joined = messages
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    (!joined.is_empty()).then_some(joined)
}

/// Apply the injected-context budget to every block in `contexts`.
///
/// Blocks within budget are returned unchanged. Over-budget blocks are spilled
/// to disk and replaced by a preview carrying the recovery path. Empty and
/// whitespace-only blocks are dropped — they only ever cost prompt bytes.
pub async fn budget_hook_contexts(session_id: &str, contexts: Vec<String>) -> Vec<String> {
    if contexts.is_empty() {
        return contexts;
    }
    let mut out = Vec::with_capacity(contexts.len());
    for text in contexts {
        if text.trim().is_empty() {
            continue;
        }
        out.push(budget_one(session_id, text).await);
    }
    out
}

/// Budget a single block. Split out so both the `Vec` entry point and tests
/// exercise the same path.
async fn budget_one(session_id: &str, text: String) -> String {
    if crate::context::budget::pressure::estimate_tokens_smart(&text) <= HOOK_CONTEXT_TOKEN_LIMIT {
        return text;
    }
    let saved = match spill_dir(session_id) {
        Some(dir) => write_spill(dir, &text).await,
        None => None,
    };
    preview(&text, saved.as_deref())
}

/// Resolve `~/.aleph/data/hook_outputs/<session>/`, or `None` when the data
/// directory cannot be located (spilling then degrades to plain truncation).
fn spill_dir(session_id: &str) -> Option<PathBuf> {
    let data = crate::utils::paths::get_data_dir().ok()?;
    Some(data.join(SPILL_DIR).join(sanitize_segment(session_id)))
}

/// Make a session id safe as a single path segment.
///
/// Aleph session ids embed `:` (`sess:s1`), which is **illegal in a Windows
/// filename** — an unsanitised id makes every spill write fail there while
/// working fine on macOS. Path separators and `..` are also folded so a
/// hostile id cannot escape the spill root.
fn sanitize_segment(session_id: &str) -> String {
    let cleaned: String = session_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "no-session".to_string()
    } else {
        cleaned
    }
}

/// Write the full text into `dir`, returning the path on success.
///
/// `tokio::fs` rather than `std::fs`: this is called from inside the run loop
/// and the tool-dispatch path, so a blocking write here would stall the
/// executor thread that other concurrent turns share. Any error is logged and
/// swallowed — the caller falls back to a path-less preview.
async fn write_spill(dir: PathBuf, text: &str) -> Option<PathBuf> {
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        warn!(dir = %dir.display(), error = %e, "failed to create hook spill directory");
        return None;
    }
    let path = dir.join(format!("{}.txt", uuid::Uuid::new_v4()));
    let body = if text.ends_with('\n') {
        text.to_string()
    } else {
        format!("{text}\n")
    };
    match tokio::fs::write(&path, body).await {
        Ok(()) => {
            prune_spill_dir(&dir).await;
            Some(path)
        }
        Err(e) => {
            warn!(path = %path.display(), error = %e, "failed to write hook spill file");
            None
        }
    }
}

/// Keep at most [`MAX_SPILL_FILES_PER_SESSION`] spill files per session,
/// deleting the oldest first.
///
/// Without this the directory grows without bound: a chatty `AfterToolCall`
/// hook spills once per tool call, and nothing else ever removes these files.
/// codex sidesteps it by writing under the OS temp dir (which the OS reaps);
/// Aleph writes under `~/.aleph/data/` so the model can read spills back with
/// its normal file tools, which means Aleph has to do the reaping itself.
/// Best-effort throughout — a failure here must never affect the turn.
async fn prune_spill_dir(dir: &Path) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Ok(meta) = entry.metadata().await else {
            continue;
        };
        if let (true, Ok(mtime)) = (meta.is_file(), meta.modified()) {
            files.push((mtime, entry.path()));
        }
    }
    if files.len() <= MAX_SPILL_FILES_PER_SESSION {
        return;
    }
    // Oldest first, then drop everything past the cap.
    files.sort_by_key(|(mtime, _)| *mtime);
    let excess = files.len() - MAX_SPILL_FILES_PER_SESSION;
    for (_, path) in files.into_iter().take(excess) {
        if let Err(e) = tokio::fs::remove_file(&path).await {
            // Best-effort cleanup; log at debug so operators can chase
            // permission/TOCTOU issues without spamming at warn level.
            tracing::debug!(path = %path.display(), error = %e, "spill prune: remove_file failed");
        }
    }
}

/// Build the model-visible replacement for an over-budget context.
fn preview(text: &str, saved: Option<&Path>) -> String {
    let total = text.chars().count();
    let head: String = text.chars().take(PREVIEW_HEAD_CHARS).collect();
    // Only emit a tail when it would show something the head didn't.
    let tail: String = if total > PREVIEW_HEAD_CHARS + PREVIEW_TAIL_CHARS {
        text.chars().skip(total - PREVIEW_TAIL_CHARS).collect()
    } else {
        String::new()
    };

    let recovery = saved.map_or_else(
        || "the rest was dropped (spill write failed)".to_string(),
        |p| format!("full output saved to {}", p.display()),
    );
    let mut out = format!("[hook context truncated — {total} chars; {recovery}]\n{head}");
    if !tail.is_empty() {
        out.push_str("\n[…]\n");
        out.push_str(&tail);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every test below that reaches `budget_one` with over-budget text writes a
    // spill file, and `spill_dir` resolves that path off the *process-global*
    // `ALEPH_HOME`. So each of them holds an `IsolatedAlephHome` for its whole
    // body: without it the spill lands either in the developer's real `~/.aleph`
    // or — worse — inside a sibling test's tempdir, which is deleted the moment
    // that test ends, i.e. between the write and the read-back in
    // `oversized_context_is_recoverable_from_the_spill_file`. That was a real
    // flake: green when run alone, red under a full parallel run.

    /// Comfortably over the 2 500-token budget whatever ratio the estimator
    /// picks for this content.
    fn oversized() -> String {
        "lorem ipsum dolor sit amet ".repeat(4_000)
    }

    #[tokio::test]
    async fn short_context_passes_through_byte_identical() {
        let input = vec!["File auto-formatted".to_string()];
        let out = budget_hook_contexts("sess", input.clone()).await;
        assert_eq!(out, input);
    }

    #[tokio::test]
    async fn empty_and_blank_contexts_are_dropped() {
        let out = budget_hook_contexts(
            "sess",
            vec![String::new(), "   \n\t ".to_string(), "real".to_string()],
        )
        .await;
        assert_eq!(out, vec!["real".to_string()]);
    }

    #[tokio::test]
    async fn oversized_context_is_bounded_and_keeps_head_and_tail() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let big = format!("HEAD-MARKER {} TAIL-MARKER", oversized());
        let out = budget_one("spill-test-session", big.clone()).await;

        assert!(
            out.chars().count() < big.chars().count() / 2,
            "must actually shrink: {} vs {}",
            out.chars().count(),
            big.chars().count()
        );
        assert!(out.starts_with("[hook context truncated"), "got: {out:.80}");
        assert!(out.contains("HEAD-MARKER"), "head must survive");
        assert!(out.contains("TAIL-MARKER"), "tail must survive");
    }

    #[tokio::test]
    async fn oversized_context_is_recoverable_from_the_spill_file() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let big = format!("UNIQUE-SPILL-BODY {}", oversized());
        let out = budget_one("spill-roundtrip", big.clone()).await;

        // Extract the path from the preview header and read it back.
        let path = out
            .lines()
            .next()
            .and_then(|l| l.split("full output saved to ").nth(1))
            .map(|p| p.trim_end_matches(']').to_string());
        let Some(path) = path else {
            // No data dir in this environment — the fallback preview is still
            // bounded, which is the contract that matters.
            assert!(out.contains("spill write failed"));
            return;
        };
        let restored = tokio::fs::read_to_string(&path).await.expect("spill file readable");
        assert!(restored.starts_with("UNIQUE-SPILL-BODY"));
        assert_eq!(
            restored.trim_end_matches('\n').chars().count(),
            big.chars().count(),
            "spill must hold the FULL text, not the preview"
        );
        // No cleanup: the isolated home owns the file and takes it with it.
    }

    #[tokio::test]
    async fn spill_dir_is_pruned_to_the_cap_oldest_first() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Seed past the cap, with ascending mtimes so "oldest" is well-defined.
        for i in 0..(MAX_SPILL_FILES_PER_SESSION + 5) {
            let p = dir.path().join(format!("{i:03}.txt"));
            tokio::fs::write(&p, "x").await.expect("write");
            let t = std::time::SystemTime::UNIX_EPOCH
                + std::time::Duration::from_secs(1_700_000_000 + i as u64);
            filetime::set_file_mtime(&p, filetime::FileTime::from_system_time(t))
                .expect("set mtime");
        }

        prune_spill_dir(dir.path()).await;

        let remaining: Vec<String> = tokio::fs::read_dir(dir.path()).await
            .expect("read_dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(remaining.len(), MAX_SPILL_FILES_PER_SESSION);
        // The five oldest went; the newest survived.
        assert!(!remaining.contains(&"000.txt".to_string()));
        assert!(!remaining.contains(&"004.txt".to_string()));
        assert!(remaining.contains(&format!("{:03}.txt", MAX_SPILL_FILES_PER_SESSION + 4)));
    }

    #[tokio::test]
    async fn prune_is_a_noop_under_the_cap_and_on_a_missing_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        tokio::fs::write(dir.path().join("only.txt"), "x").await.expect("write");
        prune_spill_dir(dir.path()).await;
        assert!(dir.path().join("only.txt").exists());
        // Must not panic on a directory that was never created.
        prune_spill_dir(&dir.path().join("nope")).await;
    }

    #[test]
    fn preview_without_a_spill_path_says_so() {
        let out = preview(&oversized(), None);
        assert!(out.contains("spill write failed"));
    }

    #[test]
    fn session_ids_with_colons_become_legal_path_segments() {
        // Windows rejects ':' in filenames; an unsanitised `sess:s1` made
        // every spill write fail there while passing on macOS.
        assert_eq!(sanitize_segment("sess:s1"), "sess_s1");
        assert_eq!(sanitize_segment("../../etc/passwd"), "______etc_passwd");
        assert_eq!(sanitize_segment(""), "no-session");
        assert_eq!(sanitize_segment("plain-id_9"), "plain-id_9");
    }

    #[test]
    fn join_messages_trims_and_drops_blanks() {
        assert_eq!(
            join_messages(&["  a  ".to_string(), String::new(), "b".to_string()]),
            Some("a\nb".to_string())
        );
        assert_eq!(join_messages(&[]), None);
        assert_eq!(join_messages(&["   ".to_string()]), None);
    }

    #[tokio::test]
    async fn preview_is_char_boundary_safe_for_multibyte_text() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        // Slicing by bytes at 1 500 would panic mid-codepoint on CJK.
        let big = "配置".repeat(20_000);
        let out = budget_one("cjk", big).await;
        assert!(out.contains('配'));
    }
}
