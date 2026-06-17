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

/// Recover the deliverable final text from a completed run's event log.
///
/// Resolution order:
/// 1. The newest `RunComplete` whose `summary.final_response` sanitizes to
///    non-empty text — the authoritative terminal answer.
/// 2. Fallback: the concatenation of every `ResponseChunk` delta, sanitized —
///    covers runs whose terminal turn carried text only as stream chunks with
///    no summary `final_response` (e.g. a provider that never set it).
///
/// Returns `None` when neither path yields non-empty text after sanitization
/// (e.g. the last turn was a pure completion-protocol confirmation).
#[must_use]
pub(crate) fn extract_final_response(events: &[StreamEvent]) -> Option<String> {
    // Primary: newest RunComplete carrying a usable final_response.
    for event in events.iter().rev() {
        if let StreamEvent::RunComplete { summary, .. } = event {
            if let Some(text) = summary.final_response.as_deref() {
                let sanitized = sanitize_llm_output(text);
                if !sanitized.is_empty() {
                    return Some(sanitized.into_owned());
                }
            }
        }
    }

    // Fallback: stitch the visible stream deltas back together in emit order.
    let mut full_text = String::new();
    for event in events {
        if let StreamEvent::ResponseChunk { delta, .. } = event {
            full_text.push_str(delta);
        }
    }
    if full_text.is_empty() {
        return None;
    }
    let sanitized = sanitize_llm_output(&full_text);
    if sanitized.is_empty() {
        None
    } else {
        Some(sanitized.into_owned())
    }
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
            content: delta.into(),
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
}
