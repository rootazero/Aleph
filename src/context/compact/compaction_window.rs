//! Pure window-construction helpers shared by every compaction path.
//!
//! Selecting a compaction window (the messages the summarizer will see), then
//! fingerprinting it, then serialising it for the LLM, are the three steps
//! every drain site performs. Keeping them here — without state, without I/O —
//! means the from-scratch `cache_reapply` arm, the extend-merge loop and the
//! session-split pre-tail seed can each pick exactly the slice they need
//! without rebuilding a sibling that has already been written.
//!
//! All of these functions are pure with respect to `&[UnifiedMessage]`, so
//! they take their inputs by slice and return their results by value.

use super::preserve::is_summary_text;
use super::summary_utils::cap_transcript_text;
use crate::providers::message::UnifiedMessage;

/// Advance a proposed cut index forward past any contiguous run of `ToolResult`
/// messages so the cut never falls *between* a `ToolCall` and the result that
/// answers it. Tool results immediately follow their call, so the only mid-pair
/// position is one where `messages[idx]` is a `ToolResult`; skipping the whole
/// run lands the boundary on a clean message (or at `messages.len()`).
///
/// Used for both compaction boundaries (`window_start`, `cut_end`) so the
/// compactor preserves the call/result pairing invariant at the source, rather
/// than relying solely on the wire-level repair in
/// [`crate::providers::message::normalize_tool_pairs`].
pub(super) fn snap_boundary_forward(messages: &[UnifiedMessage], idx: usize) -> usize {
    let mut i = idx;
    while i < messages.len() && matches!(messages[i], UnifiedMessage::ToolResult { .. }) {
        i += 1;
    }
    i
}

/// Select the exclusive end of a compaction window that starts at `start`.
///
/// Walks forward from `start`, accumulating each message's *capped* transcript
/// token estimate (matching what [`serialize_transcript`] actually sends the
/// summarizer, via [`cap_transcript_text`]), and stops once `budget_tokens` is
/// reached or `max_messages` messages have been taken — whichever binds first.
/// The end is then snapped forward past any tool-result run (so the kept region
/// never begins on an orphaned result whose call was drained into the summary)
/// and clamped to `[start + 1, hard_end]`, so the window is always non-empty and
/// never spills past the fresh-tail boundary `hard_end`.
///
/// This is the single source of window bounding shared by the from-scratch
/// window selection in [`ContextCompactor::compact_inner`](super::compactor) and the
/// extend-merge in `reapply_cached`, keeping both calls within the same
/// summarizer-input budget.
pub(super) fn select_window_end(
    messages: &[UnifiedMessage],
    start: usize,
    hard_end: usize,
    max_messages: usize,
    budget_tokens: usize,
) -> usize {
    if start >= hard_end {
        return hard_end;
    }
    let msg_ceiling = start.saturating_add(max_messages.max(1));
    let mut acc = 0usize;
    let mut end = start;
    while end < hard_end && end < msg_ceiling {
        let text = messages[end].transcript_text();
        let capped = cap_transcript_text(&text);
        acc = acc.saturating_add(estimate_tokens(capped.as_ref()));
        end += 1;
        if acc >= budget_tokens {
            break;
        }
    }
    // Snap past any tool-result run so the kept region [end..] never begins on an
    // orphan, then clamp: never past the fresh-tail boundary, always ≥ 1 message.
    snap_boundary_forward(messages, end)
        .min(hard_end)
        .max(start + 1)
        .min(hard_end)
}

/// Content fingerprint of a message window: role discriminant + text content
/// per message. Deterministic across turns because the prompt builder and the
/// preflight cheap passes are deterministic functions of an append-only
/// session log — when a pass *does* change an old message (e.g. a new file op
/// supersedes an earlier result), the hash misses and the compactor falls
/// back to a full recompaction.
pub(super) fn hash_window(messages: &[UnifiedMessage]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for m in messages {
        std::mem::discriminant(m).hash(&mut h);
        m.transcript_text().hash(&mut h);
    }
    h.finish()
}

/// Extract the text content of the first content block in a message (if Text).
pub(super) fn first_message_text(msg: &UnifiedMessage) -> Option<&str> {
    msg.content_blocks().first().and_then(|b| b.as_text())
}

/// If `text` opens with a compaction-summary marker line — `[Context Summary]`
/// (LLM / truncation paths) or `[Context Summary (from session memory)]` (the
/// reuse path) — return the body after that line; `None` for raw turns. A
/// window carrying one holds a *prior* summary being folded into a wider one,
/// which routes summarization to the incremental "update" prompt instead of
/// re-summarizing the already-condensed text from scratch. Both markers share
/// the `[Context Summary` head recognised by [`is_summary_text`]; requiring
/// the marker line to close with `]` keeps a raw turn that merely opens with
/// those words from matching. `'\n'` is ASCII, so the byte split lands on a
/// UTF-8 boundary.
pub(super) fn strip_context_summary_prefix(text: &str) -> Option<&str> {
    let line_end = text.find('\n').unwrap_or(text.len());
    let marker = &text[..line_end];
    if !is_summary_text(marker) || !marker.trim_end().ends_with(']') {
        return None;
    }
    Some(text[line_end..].trim_start_matches('\n'))
}

/// Serialize a slice of messages into a human-readable transcript, capping each
/// message body via [`cap_transcript_text`] so a few huge old tool results can
/// never blow up the side-channel summarizer prompt.
pub(super) fn serialize_transcript(messages: &[UnifiedMessage]) -> String {
    let mut lines = Vec::with_capacity(messages.len());
    for msg in messages {
        let text = msg.transcript_text();
        let capped = cap_transcript_text(&text);
        let role = match msg {
            UnifiedMessage::User { .. } => "user",
            UnifiedMessage::Assistant { .. } => "assistant",
            UnifiedMessage::ToolResult { tool_name, .. } => {
                lines.push(format!("tool_result({tool_name}): {capped}"));
                continue;
            }
        };
        lines.push(format!("{role}: {capped}"));
    }
    lines.join("\n")
}

/// Estimate token count using content-aware ratio detection.
///
/// Thin alias for [`pressure::estimate_tokens_smart`] — the single source of
/// truth for the prose-anchored, CJK/code-aware char→token estimate (which now
/// blends mixed content proportionally). Kept as a local name so the ten call
/// sites below read clearly.
pub(super) fn estimate_tokens(text: &str) -> usize {
    crate::context::budget::pressure::estimate_tokens_smart(text)
}
