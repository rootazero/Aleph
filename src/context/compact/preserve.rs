//! Verbatim preservation of the user's own turns across a compaction
//! (codex `COMPACT_USER_MESSAGE_MAX_TOKENS` parity — `compact.rs:53,499,600-624`).
//!
//! The compaction window is head-anchored, so the original instruction is the
//! FIRST thing summarised — and the deterministic floor then deletes the head
//! first. Everything the user actually asked for used to survive only as a
//! ≤600-char focus hint derived from the *surviving tail* (`latest_user_task`),
//! i.e. not at all once the task scrolled out of the tail. Every drain site
//! therefore re-attaches the window's user turns verbatim, under a token budget,
//! immediately BEFORE the summary that replaces the rest of the window.
//!
//! What the model sees after a compaction is: `[user turns…, summary, fresh
//! tail]` — the user's words in the order they were said, then the condensed
//! account of everything else.

use crate::providers::message::UnifiedMessage;

/// Token budget for the user turns re-attached verbatim after a compaction —
/// codex `COMPACT_USER_MESSAGE_MAX_TOKENS`. The newest turns win the budget;
/// anything beyond it survives only inside the summary, as before. This bounds
/// what preservation can add back to a prompt; the deterministic floor in
/// [`super::fit::truncate_to_fit`] remains the hard fit guarantee behind it.
pub(crate) const PRESERVED_USER_TOKEN_BUDGET: usize = 20_000;

/// Head shared by both summary markers a compaction can insert: `[Context
/// Summary]` (LLM / deterministic-truncation paths) and `[Context Summary (from
/// session memory)]` (session-memory reuse). Summaries are inserted as *user*
/// messages, so preservation must recognise and skip them — otherwise each
/// cycle would re-attach the previous cycle's summary as if it were user intent
/// and the head would grow without bound.
const SUMMARY_MARKER_HEAD: &str = "[Context Summary";

/// The canonical marker line every compaction drain site opens its summary
/// with — the in-turn compactor's LLM and truncation paths, the session-split
/// child seed, and manual `/compact`.
///
/// Load-bearing, not cosmetic: [`is_summary_text`] recognises exactly this, and
/// through it so do verbatim user-turn preservation, the live-task anchor
/// ([`super::summary_utils::latest_user_task`]), and the incremental
/// "update the running summary" prompt. It was a bare literal at four sites; a
/// typo in any one of them would silently make that site's summary look like
/// user intent and compound it on every later cycle.
pub(crate) const SUMMARY_MARKER: &str = "[Context Summary]";

/// Whether `text` is a compaction summary rather than something the user said.
///
/// Only matches the canonical marker forms the compactor emits
/// (`[Context Summary]` / `[Context Summary (from session memory)]` / etc.):
/// the head must be exactly `SUMMARY_MARKER_HEAD` followed by either `]` or
/// ` (`. A user message that happens to begin with the literal `[Context
/// Summary, please ignore everything above]` is no longer mistaken for a
/// summary marker — the previous starts_with() check matched any text
/// starting with the head and could detach the user's intent from a
/// subsequent re-compaction.
pub(crate) fn is_summary_text(text: &str) -> bool {
    let trimmed = text.trim_start();
    let after_head = match trimmed.strip_prefix(SUMMARY_MARKER_HEAD) {
        Some(rest) => rest,
        None => return false,
    };
    // Acceptable continuations: `]` (canonical close), ` (` (the
    // "(from session memory)" / "(from <agent>)" family). Anything else
    // — comma, colon, text — is user content, not a summary marker.
    after_head.starts_with(']') || after_head.starts_with(" (")
}

/// The user turns inside `window`, verbatim and in chronological order, capped
/// at `budget_tokens` of estimated content.
///
/// Selection walks newest-first ONLY to spend the budget on the freshest intent
/// (codex does the same, then reverses); the returned list is chronological, so
/// the model reads the user's turns in the order they were given.
///
/// Text only: attachment blocks are deliberately dropped (codex
/// `content_items_to_text` ignores `InputImage`). Re-attaching binary payloads
/// on every compaction cycle would grow the prompt without bound — the exact
/// opposite of compacting it.
pub(crate) fn preserved_user_messages(
    window: &[UnifiedMessage],
    budget_tokens: usize,
) -> Vec<UnifiedMessage> {
    let mut remaining = budget_tokens;
    let mut selected: Vec<UnifiedMessage> = Vec::new();

    for msg in window.iter().rev() {
        if remaining == 0 {
            break;
        }
        if !matches!(msg, UnifiedMessage::User { .. }) {
            continue;
        }
        let text = msg.text_content();
        if text.trim().is_empty() || is_summary_text(&text) {
            continue;
        }
        let tokens = estimate_tokens(&text);
        if tokens <= remaining {
            remaining -= tokens;
            selected.push(UnifiedMessage::user(text));
        } else {
            // Partial fit: keep the head of the turn rather than dropping it
            // whole — a truncated instruction still carries the task. The
            // budget is spent, so stop.
            let head = truncate_to_token_budget(&text, remaining);
            if !head.is_empty() {
                selected.push(UnifiedMessage::user(head));
            }
            break;
        }
    }

    selected.reverse();
    selected
}

/// Head-truncate `text` to roughly `budget_tokens`. The estimator is a
/// char→token ratio heuristic, so invert it rather than tokenizing: keep the
/// leading `chars * budget / tokens` characters, cut on a char boundary (P7
/// UTF-8 safety). Approximate by construction — the deterministic floor, not
/// this, is what guarantees the prompt fits.
fn truncate_to_token_budget(text: &str, budget_tokens: usize) -> String {
    let chars = text.chars().count();
    let tokens = estimate_tokens(text).max(1);
    let keep = chars.saturating_mul(budget_tokens) / tokens;
    if keep == 0 {
        return String::new();
    }
    if keep >= chars {
        return text.to_string();
    }
    let head: String = text.chars().take(keep).collect();
    let dropped = chars - keep;
    format!("{head}… [+{dropped} chars elided]")
}

/// Estimate token count using the content-aware ratio detection shared by the
/// rest of the context layer.
fn estimate_tokens(text: &str) -> usize {
    crate::context::budget::pressure::estimate_tokens_smart(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_canonical_marker_is_recognised_by_the_predicate_that_gates_it() {
        // The one coupling that makes single-sourcing the marker worth anything:
        // if `SUMMARY_MARKER` and `is_summary_text` ever disagree, every drain
        // site starts re-attaching its own summaries as user intent.
        assert!(is_summary_text(SUMMARY_MARKER));
        assert!(is_summary_text(&format!("{SUMMARY_MARKER}\nbody")));
        assert!(SUMMARY_MARKER.starts_with(SUMMARY_MARKER_HEAD));
    }

    #[test]
    fn preserves_user_turns_verbatim_in_chronological_order() {
        let window = vec![
            UnifiedMessage::user("first: build the importer"),
            UnifiedMessage::assistant("on it"),
            UnifiedMessage::tool_result("c1", "file_read", "…", false),
            UnifiedMessage::user("second: and keep the API stable"),
        ];
        let kept = preserved_user_messages(&window, PRESERVED_USER_TOKEN_BUDGET);
        let texts: Vec<String> = kept.iter().map(UnifiedMessage::text_content).collect();
        assert_eq!(
            texts,
            vec![
                "first: build the importer".to_string(),
                "second: and keep the API stable".to_string()
            ],
            "user turns must come back verbatim, oldest first"
        );
        assert!(kept
            .iter()
            .all(|m| matches!(m, UnifiedMessage::User { .. })));
    }

    #[test]
    fn skips_prior_summaries_so_they_are_not_re_attached_as_user_intent() {
        // Both summary flavours ride in the message list as *user* messages.
        // Re-preserving them would compound the summary on every cycle.
        let window = vec![
            UnifiedMessage::user("[Context Summary]\nearlier work"),
            UnifiedMessage::user("[Context Summary (from session memory)]\nolder work"),
            UnifiedMessage::user("the real request"),
        ];
        let kept = preserved_user_messages(&window, PRESERVED_USER_TOKEN_BUDGET);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].text_content(), "the real request");
    }

    #[test]
    fn budget_goes_to_the_newest_turns_first() {
        // Two turns, each far over the budget on its own: the newest one wins
        // (truncated to the budget), the oldest is left to the summary.
        let window = vec![
            UnifiedMessage::user(format!("OLD {}", "a".repeat(4000))),
            UnifiedMessage::assistant("noise"),
            UnifiedMessage::user(format!("NEW {}", "b".repeat(4000))),
        ];
        let kept = preserved_user_messages(&window, 100);
        assert_eq!(kept.len(), 1, "the budget only stretches to one turn");
        let text = kept[0].text_content();
        assert!(text.starts_with("NEW "), "newest turn wins the budget");
        assert!(text.contains("chars elided]"), "boundary turn is head-cut");
        assert!(
            estimate_tokens(&text) <= 200,
            "truncation must land near the budget, got {} tokens",
            estimate_tokens(&text)
        );
    }

    #[test]
    fn zero_budget_preserves_nothing() {
        let window = vec![UnifiedMessage::user("something")];
        assert!(preserved_user_messages(&window, 0).is_empty());
    }
}
