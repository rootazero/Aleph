//! Pure helpers for applying the tool-result budget pipeline:
//! `compress → persist-if-large → truncate-if-small`.
//!
//! Extracted from `pipeline/helpers.rs` so it can be consumed by the
//! production `ScopedToolService::execute` path (which is not routed
//! through the still-orphaned `ToolPipeline`).
//!
//! Layering:
//! - `resolve_result_budget(name, explicit)` resolves the per-tool token
//!   budget. `read_file`-family tools always return `None` to break the
//!   read → marker → re-read → persist loop, even if a misconfigured tool
//!   declares its own budget.
//! - `apply_result_budget(...)` runs the compress/persist/truncate cascade
//!   over a tool's text output and returns `ProcessedResult`.

use std::path::PathBuf;

use crate::context::budget::pressure::estimate_tokens_smart;
use crate::context::retrieval::IndexOutcome;
use crate::tools::result_store::{extract_persisted_ref, ToolResultStore};

/// Global default budget for tools that neither declare an explicit
/// `max_result_tokens` nor appear in the legacy name table. Mirrors the
/// `MAX_TOOL_RESULT_TOKENS` constant in `pipeline/helpers.rs`.
pub const DEFAULT_RESULT_BUDGET_TOKENS: usize = 8_000;

/// Resolve a tool's per-result token budget.
///
/// Lookup order:
/// 1. `read_file` / `Read` / `file_read` always return `None` (system
///    invariant — a `read_file` result is the only way the model can pull
///    a persisted marker file back into context, so persisting one would
///    create a loop).
/// 2. `explicit` (typically the tool's own `max_result_tokens()` value)
///    wins for every other name.
/// 3. Otherwise fall back to a hardcoded name table for legacy builtin
///    names that have not been migrated to the trait method.
/// 4. Otherwise fall back to [`DEFAULT_RESULT_BUDGET_TOKENS`].
///
/// `None` from this function means "do not persist this tool's output;
/// just truncate when it exceeds the global default".
pub fn resolve_result_budget(name: &str, explicit: Option<usize>) -> Option<usize> {
    match name {
        "read_file" | "Read" | "file_read" => return None,
        _ => {}
    }
    if let Some(n) = explicit {
        return Some(n);
    }
    match name {
        "Bash" | "bash" | "bash_exec" | "terminal" => Some(8_000),
        "WebFetch" | "web_fetch" => Some(10_000),
        "Grep" | "search_files" => Some(6_000),
        _ => Some(DEFAULT_RESULT_BUDGET_TOKENS),
    }
}

/// Output of [`apply_result_budget`]. `text` is what the LLM should see.
/// `persisted_path` is `Some(path)` iff the original text was offloaded
/// to disk via `ToolResultStore::persist_if_large`.
#[derive(Debug, Clone)]
pub struct ProcessedResult {
    pub text: String,
    pub tokens_in_context: usize,
    pub persisted_path: Option<PathBuf>,
}

/// Apply Layer 2 of the budget pipeline to a successful tool output.
///
/// Caller is responsible for any tool-specific compression (e.g.
/// `compress_tool_output`) before invoking this helper; this layer
/// just decides between "keep verbatim", "persist + marker", and
/// "truncate".
pub fn apply_result_budget(
    tool_call_id: &str,
    tool_name: &str,
    text: &str,
    store: Option<&ToolResultStore>,
    budget: Option<usize>,
) -> ProcessedResult {
    let tokens = estimate_tokens_smart(text);
    let Some(budget) = budget else {
        // Budget = None → never persist; distill salient errors/paths if any,
        // else fall back to a global head+tail truncate.
        let truncated = distill_or_truncate(text, DEFAULT_RESULT_BUDGET_TOKENS);
        let tokens_after = estimate_tokens_smart(&truncated);
        return ProcessedResult {
            text: truncated,
            tokens_in_context: tokens_after,
            persisted_path: None,
        };
    };
    if tokens <= budget {
        return ProcessedResult {
            text: text.to_string(),
            tokens_in_context: tokens,
            persisted_path: None,
        };
    }
    if let Some(store) = store {
        if let Some(marker) = store.persist_if_large(tool_call_id, tool_name, text, budget) {
            let path = extract_persisted_ref(&marker).and_then(parse_marker_path);
            // Index the offloaded blob so the model can BM25-retrieve only the
            // relevant slices via `ctx_search` instead of re-reading the whole
            // file (which would defeat the offload). Best-effort: on failure
            // the bare persist marker still lets the model `read_file` it back.
            let indexed = store.index_output(tool_call_id, tool_name, text);
            let full_marker = match indexed.filter(|o| o.sections > 0) {
                Some(outcome) => format!("{marker}\n{}", search_hint(&outcome)),
                None => marker,
            };
            // Surface key errors inline above the marker so a failed command's
            // diagnostics are visible immediately, not gated behind a
            // ctx_search round-trip. Bounded; absent when there is no error.
            let full_marker = match inline_error_digest(text) {
                Some(errors) => format!("{errors}\n{full_marker}"),
                None => full_marker,
            };
            let tokens_after = estimate_tokens_smart(&full_marker);
            return ProcessedResult {
                text: full_marker,
                tokens_in_context: tokens_after,
                persisted_path: path,
            };
        }
        // Persist failed (the store logs internally); fall through to truncate.
    }
    let truncated = distill_or_truncate(text, budget);
    let tokens_after = estimate_tokens_smart(&truncated);
    ProcessedResult {
        text: truncated,
        tokens_in_context: tokens_after,
        persisted_path: None,
    }
}

/// Build the model-facing hint appended to a persist marker when the output
/// was also indexed for retrieval. Tells the model it can `ctx_search` the
/// offloaded blob instead of re-reading the whole file, and lists the first
/// few section titles as orientation. Kept to a few hundred bytes so the
/// offload's token saving is preserved.
fn search_hint(outcome: &IndexOutcome) -> String {
    let preview = outcome.previews.join(" · ");
    if preview.is_empty() {
        format!(
            "[Indexed {} sections — use ctx_search(query=\"…\") to retrieve only \
             the relevant parts instead of re-reading the whole file]",
            outcome.sections
        )
    } else {
        format!(
            "[Indexed {} sections — use ctx_search(query=\"…\") to retrieve only the \
             relevant parts instead of re-reading the whole file. First sections: {}]",
            outcome.sections, preview
        )
    }
}

/// Reduce over-budget text to a salient digest when it carries error / path
/// signal, otherwise fall back to head+tail [`truncate_with_budget`].
///
/// This is the "only the key errors, paths, context" path: for command / log
/// output whose real signal sits in the *middle* of the stream (compile
/// errors, panics, failing assertions), head+tail truncation drops exactly
/// that middle. [`distill_output`](crate::tool_output::distill::distill_output)
/// extracts it locally. The digest is preferred only when it both carries an
/// error and fits the budget; signal-free output still truncates as before.
fn distill_or_truncate(text: &str, budget_tokens: usize) -> String {
    if let Some(digest) = crate::tool_output::distill::distill_output(text) {
        if digest.error_count > 0 {
            let rendered = digest.render(digest.salient.len());
            if estimate_tokens_smart(&rendered) <= budget_tokens {
                return rendered;
            }
        }
    }
    truncate_with_budget(text, budget_tokens)
}

/// Inline error preview prepended to a persist marker, so the model sees the
/// key failures immediately instead of having to `ctx_search` the offloaded
/// blob first. Returns `None` when there is no error signal. Bounded to a
/// handful of lines to preserve the offload's token saving.
fn inline_error_digest(text: &str) -> Option<String> {
    let digest = crate::tool_output::distill::distill_output(text)?;
    if digest.error_count == 0 {
        return None;
    }
    Some(digest.render(8))
}

/// Head + tail truncation under the budget. Mirrors
/// `pipeline/helpers.rs::truncate_tool_result_with_budget`, which is
/// removed in the same cycle that ships this module.
pub fn truncate_with_budget(text: &str, budget_tokens: usize) -> String {
    let estimated = estimate_tokens_smart(text);
    if estimated <= budget_tokens {
        return text.to_string();
    }
    // Roughly 4 chars per token; keep ~70 % head + 30 % tail under the budget.
    let target_chars = budget_tokens.saturating_mul(4);
    let head_chars = (target_chars * 7 / 10).min(text.len());
    let tail_budget = target_chars.saturating_sub(head_chars);
    let tail_chars = tail_budget.min(text.len().saturating_sub(head_chars));

    let head_end = floor_char_boundary(text, head_chars);
    let tail_start_raw = text.len().saturating_sub(tail_chars);
    let tail_start = ceil_char_boundary(text, tail_start_raw);
    let tail_start = tail_start.max(head_end);

    let omitted = estimated.saturating_sub(budget_tokens);
    format!(
        "{}\n... [output truncated, ~{} tokens omitted] ...\n{}",
        &text[..head_end],
        omitted,
        &text[tail_start..]
    )
}

fn floor_char_boundary(s: &str, idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    let mut i = idx;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char_boundary(s: &str, idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    let mut i = idx;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

fn parse_marker_path(line: &str) -> Option<PathBuf> {
    // Marker format: "[Full output persisted: <path> (<n> tokens, <tool>)]".
    let prefix = "[Full output persisted: ";
    let start = line.find(prefix)? + prefix.len();
    let rest = &line[start..];
    let end = rest.find(" (")?;
    Some(PathBuf::from(rest[..end].to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_store(name: &str) -> (ToolResultStore, PathBuf) {
        let base = std::env::temp_dir()
            .join("aleph_test_result_processing")
            .join(name);
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let store = ToolResultStore::with_dir_for_tests(base.clone());
        (store, base)
    }

    // ---------------------------------------------------------------
    // resolve_result_budget
    // ---------------------------------------------------------------

    #[test]
    fn read_file_family_always_returns_none() {
        assert_eq!(resolve_result_budget("read_file", None), None);
        assert_eq!(resolve_result_budget("Read", None), None);
        assert_eq!(resolve_result_budget("file_read", None), None);
        // Even an explicit setting cannot override the read-recursion guard.
        assert_eq!(resolve_result_budget("read_file", Some(99_999)), None);
    }

    #[test]
    fn explicit_wins_over_fallback_table() {
        assert_eq!(resolve_result_budget("bash", Some(123)), Some(123));
        assert_eq!(resolve_result_budget("custom_thing", Some(50)), Some(50));
    }

    #[test]
    fn fallback_table_bash_returns_eight_k() {
        assert_eq!(resolve_result_budget("bash_exec", None), Some(8_000));
        assert_eq!(resolve_result_budget("Bash", None), Some(8_000));
    }

    #[test]
    fn fallback_table_web_and_grep() {
        assert_eq!(resolve_result_budget("web_fetch", None), Some(10_000));
        assert_eq!(resolve_result_budget("WebFetch", None), Some(10_000));
        assert_eq!(resolve_result_budget("Grep", None), Some(6_000));
        assert_eq!(resolve_result_budget("search_files", None), Some(6_000));
    }

    #[test]
    fn unknown_tool_falls_back_to_default() {
        assert_eq!(
            resolve_result_budget("some_other_tool", None),
            Some(DEFAULT_RESULT_BUDGET_TOKENS)
        );
    }

    // ---------------------------------------------------------------
    // apply_result_budget
    // ---------------------------------------------------------------

    #[test]
    fn small_text_unchanged() {
        let (store, _base) = test_store("small_unchanged");
        let out = apply_result_budget("c1", "bash", "hello", Some(&store), Some(10_000));
        assert_eq!(out.text, "hello");
        assert!(out.persisted_path.is_none());
    }

    #[test]
    fn budget_none_truncates_no_persist() {
        let (store, base) = test_store("budget_none");
        let big = "x".repeat(60_000);
        let out = apply_result_budget("c2", "read_file", &big, Some(&store), None);
        assert!(
            out.persisted_path.is_none(),
            "must not persist when budget is None"
        );
        assert!(
            !out.text.starts_with("[Full output persisted:"),
            "should be truncated, got: {}",
            &out.text[..80.min(out.text.len())]
        );
        // The store directory should remain empty.
        let entries: Vec<_> = std::fs::read_dir(&base)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 0, "no file should be written");
    }

    #[test]
    fn large_text_persists_returns_marker() {
        let (store, base) = test_store("large_persists");
        // Build text with retrievable structure so indexing produces sections.
        let big = (0..2000)
            .map(|i| format!("line {i} payload alpha beta gamma"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = apply_result_budget("c3", "bash", &big, Some(&store), Some(100));
        assert!(
            out.text.starts_with("[Full output persisted:"),
            "expected marker, got: {}",
            &out.text[..80.min(out.text.len())]
        );
        assert!(out.persisted_path.is_some());
        // The marker is now augmented with a ctx_search retrieval hint.
        assert!(
            out.text.contains("ctx_search"),
            "expected ctx_search hint in marker, got: {}",
            out.text
        );
        // Exactly one persisted blob (.txt); the FTS5 index.db lives alongside.
        let txt_count = std::fs::read_dir(&base)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "txt"))
            .count();
        assert_eq!(txt_count, 1, "exactly one .txt blob should be written");
        // The blob's content is searchable through the same store.
        let hits = store.search("payload alpha", 3);
        assert!(!hits.is_empty(), "offloaded blob should be searchable");
    }

    #[test]
    fn no_store_means_truncate_only() {
        let big = "z".repeat(40_000);
        let out = apply_result_budget("c4", "bash", &big, None, Some(100));
        assert!(out.persisted_path.is_none());
        assert!(!out.text.starts_with("[Full output persisted:"));
        assert!(
            out.text.contains("[output truncated"),
            "expected truncate marker: {}",
            &out.text[..120.min(out.text.len())]
        );
    }

    #[test]
    fn parse_marker_path_roundtrip() {
        let marker = "[Full output persisted: /tmp/aleph/x.txt (1234 tokens, bash)]";
        let path = parse_marker_path(marker).expect("parse");
        assert_eq!(path, PathBuf::from("/tmp/aleph/x.txt"));
    }

    #[test]
    fn budget_none_distills_errors_instead_of_truncating() {
        let (store, _base) = test_store("budget_none_distill");
        // Error signal buried in the middle of head/tail noise.
        let mut big = (0..400)
            .map(|i| format!("   Compiling crate_{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        big.push_str("\nerror[E0382]: borrow of moved value: `x`\n");
        big.push_str("  --> src/main.rs:42:9\n");
        big.push_str(
            &(0..400)
                .map(|i| format!("   Finished step_{i}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let out = apply_result_budget("c-distill", "read_file", &big, Some(&store), None);
        assert!(out.persisted_path.is_none());
        assert!(
            out.text.contains("E0382") && out.text.contains("Output digest"),
            "expected distilled errors, got: {}",
            &out.text[..120.min(out.text.len())]
        );
        // Head/tail compile noise must be gone.
        assert!(!out.text.contains("Compiling crate_0"));
    }

    #[test]
    fn persist_branch_prepends_inline_errors() {
        let (store, _base) = test_store("persist_inline_errors");
        let mut big = String::new();
        big.push_str("error: linker failed with exit code 1\n");
        big.push_str("  --> src/net.rs:10:3\n");
        // Enough retrievable structure to index into sections + exceed budget.
        for i in 0..2000 {
            big.push_str(&format!("trace line {i} payload alpha beta gamma\n"));
        }
        let out = apply_result_budget("c-persist", "bash", &big, Some(&store), Some(100));
        assert!(out.persisted_path.is_some(), "should have persisted");
        // The marker is still present...
        assert!(out.text.contains("[Full output persisted:"));
        // ...but errors now lead so they are visible without a ctx_search.
        assert!(
            out.text.contains("Output digest") && out.text.contains("linker failed"),
            "expected inline error digest above marker, got: {}",
            &out.text[..160.min(out.text.len())]
        );
    }

    #[test]
    fn truncate_preserves_head_and_tail() {
        let text = format!("HEAD{}TAIL", "x".repeat(80_000));
        let out = truncate_with_budget(&text, 100);
        assert!(out.starts_with("HEAD"), "head missing: {}", &out[..80]);
        assert!(
            out.ends_with("TAIL"),
            "tail missing: {}",
            &out[out.len() - 80..]
        );
        assert!(out.contains("[output truncated"));
    }
}
