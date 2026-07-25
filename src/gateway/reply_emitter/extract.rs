//! Canonical "final answer" extraction from a recorded `StreamEvent` log.
//!
//! Aleph keeps **no dedicated table** for a run's final answer — the terminal
//! text is recovered by scanning the emitted `StreamEvent` log for the
//! `RunComplete` summary (see FEATURE_LOCATOR §4.7). Several consumers need
//! this exact recovery — the group-chat broadcaster (`teams::broadcast`) and
//! the cron executor (`tasks::cron`) — so the logic lives here once instead of
//! drifting across hand-rolled copies.
//!
//! Lives in `reply_emitter` because that is the module that already owns the
//! "agent run → deliverable user text" concern (and `sanitize_llm_output`);
//! the dependency points down to `event_emitter`, never back up.

use super::sanitize::sanitize_llm_output;
use crate::gateway::event_emitter::StreamEvent;

/// Sanitize a run's terminal text into clean, deliverable user text.
///
/// The single atom for "given the raw text a run produced, what do we actually
/// hand to a surface" — strips LLM-internal tags (`<think>`, completion markers)
/// and returns `None` when nothing visible survives (a pure-thinking or
/// completion-only turn). Every final-answer **delivery** path routes through
/// this: [`extract_final_response`] (group-chat transcript, cron), the
/// cross-surface `OriginFanoutEmitter`, and the team `TeamFanoutEmitter`. That
/// is what stops a live Panel bubble, an origin-channel message, and the
/// persisted transcript from ever disagreeing
/// on the text — or leaking raw reasoning tags that the inbound `ReplyEmitter`
/// path already strips.
#[must_use]
fn sanitize_final_text(text: &str) -> Option<String> {
    let sanitized = sanitize_llm_output(text);
    if sanitized.trim().is_empty() {
        None
    } else {
        Some(sanitized.into_owned())
    }
}

/// Back-compat name used by fanout/telegram/cron/broadcast.
#[must_use]
pub(crate) fn sanitize_final_response(text: &str) -> Option<String> {
    sanitize_final_text(text)
}

/// Recover the deliverable final text from a completed run's event log.
///
/// Resolution order:
/// 1. The newest `RunComplete` whose `summary.final_response` sanitizes to
///    non-empty text — the authoritative terminal answer.
/// 2. Fallback: the concatenation of every **answer** `ResponseChunk` delta,
///    sanitized — covers runs whose terminal turn carried text only as stream
///    chunks with no summary `final_response` (e.g. a provider that never set
///    it). `is_intermediate` chunks are excluded: those are standalone
///    progress messages (approval prompts, scratchpad ticks) that were already
///    delivered on their own, and folding them into the answer put e.g. an
///    approval prompt at the head of the group-chat transcript row.
///
/// Returns `None` when neither path yields non-empty text after sanitization
/// (e.g. the last turn was a pure completion-protocol confirmation).
#[must_use]
pub(crate) fn extract_final_response(events: &[StreamEvent]) -> Option<String> {
    // Primary: newest RunComplete carrying a usable final_response.
    for event in events.iter().rev() {
        if let StreamEvent::RunComplete { summary, .. } = event {
            if let Some(text) = summary.final_response.as_deref() {
                if let Some(clean) = sanitize_final_response(text) {
                    return Some(clean);
                }
            }
        }
    }

    // Fallback: stitch the visible answer deltas back together in emit order.
    let mut full_text = String::new();
    for event in events {
        if let StreamEvent::ResponseChunk {
            delta,
            is_intermediate: false,
            ..
        } = event
        {
            full_text.push_str(delta);
        }
    }
    sanitize_final_response(&full_text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::event_emitter::RunSummary;

    fn run_complete(final_response: Option<&str>) -> StreamEvent {
        StreamEvent::RunComplete {
            run_id: "r".into(),
            seq: 0,
            summary: RunSummary {
                final_response: final_response.map(str::to_string),
                ..Default::default()
            },
            total_duration_ms: 0,
        }
    }

    fn chunk(delta: &str) -> StreamEvent {
        StreamEvent::ResponseChunk {
            run_id: "r".into(),
            seq: 0,
            delta: delta.into(),
            full_text: delta.into(),
            chunk_index: 0,
            is_final: false,
            is_intermediate: false,
        }
    }

    #[test]
    fn prefers_run_complete_final_response() {
        let events = vec![chunk("streamed"), run_complete(Some("authoritative"))];
        assert_eq!(
            extract_final_response(&events).as_deref(),
            Some("authoritative")
        );
    }

    #[test]
    fn sanitizes_internal_tags_from_final_response() {
        let events = vec![run_complete(Some("<think>hidden</think>visible answer"))];
        assert_eq!(
            extract_final_response(&events).as_deref(),
            Some("visible answer")
        );
    }

    #[test]
    fn falls_back_to_concatenated_chunks_when_summary_empty() {
        // Summary present but final_response unset → stitch the deltas.
        let events = vec![chunk("hello "), chunk("world"), run_complete(None)];
        assert_eq!(
            extract_final_response(&events).as_deref(),
            Some("hello world")
        );
    }

    /// Standalone progress messages (approval prompts, scratchpad ticks —
    /// `approval::operator_requester` is a live producer) were already
    /// delivered on their own. Folding them into the stitched fallback put
    /// the approval prompt at the head of the transcript row / cron result.
    #[test]
    fn fallback_skips_intermediate_progress_chunks() {
        let intermediate = StreamEvent::ResponseChunk {
            run_id: "r".into(),
            seq: 0,
            delta: "[approval needed] run bash?".into(),
            full_text: String::new(),
            chunk_index: 0,
            is_final: false,
            is_intermediate: true,
        };
        let events = vec![
            intermediate,
            chunk("the "),
            chunk("answer"),
            run_complete(None),
        ];
        assert_eq!(
            extract_final_response(&events).as_deref(),
            Some("the answer")
        );
    }

    #[test]
    fn returns_none_for_silent_completion() {
        let events = vec![run_complete(None)];
        assert_eq!(extract_final_response(&events), None);
    }

    #[test]
    fn uses_newest_run_complete() {
        let events = vec![run_complete(Some("first")), run_complete(Some("second"))];
        assert_eq!(extract_final_response(&events).as_deref(), Some("second"));
    }

    // ── sanitize_final_response atom (single source for surface delivery) ──

    #[test]
    fn atom_strips_reasoning_tags() {
        assert_eq!(
            sanitize_final_response("<think>scratch</think>answer").as_deref(),
            Some("answer")
        );
    }

    #[test]
    fn atom_returns_none_for_pure_reasoning_or_empty() {
        // A turn that sanitizes to nothing visible yields None (no surface
        // delivery): a pure-reasoning block collapses, as does empty input.
        assert_eq!(
            sanitize_final_response("<think>only thinking</think>"),
            None
        );
        assert_eq!(sanitize_final_response(""), None);
    }

    #[test]
    fn atom_is_idempotent_on_clean_text() {
        assert_eq!(
            sanitize_final_response("already clean").as_deref(),
            Some("already clean")
        );
    }
}
