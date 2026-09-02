//! Event-level cut-boundary guards for compaction drain sites that operate on
//! the persisted event log (`SessionEventRecord`) rather than on the rebuilt
//! prompt (`UnifiedMessage`).
//!
//! The in-place compactor's message-level twin is
//! [`crate::context::compact::compactor`]'s `snap_boundary_forward`; the two
//! levels share one invariant — a kept region must never *begin* on a tool
//! result whose originating `tool_use` was just summarized/retired away — but
//! operate on different types, so they are parallel implementations of one
//! rule rather than one function.
//!
//! Direction rule for every guard here: a snap may only move the cut so that
//! **less** is compacted / **more** is kept verbatim — never the other way.

use crate::session::events::{SessionEvent, SessionEventRecord};

/// Advance `cut` past any contiguous run of `ToolResult` / `ToolError` so the
/// kept region never *begins* on a result whose `tool_use` was just retired.
///
/// `build_prompt` already downgrades an orphan result to plain text rather
/// than letting the provider reject it (Anthropic: `tool_result` without a
/// preceding `tool_use`), but producing the orphan in the first place turns a
/// structured result into prose for the rest of the session.
pub(crate) fn snap_past_tool_results(events: &[SessionEventRecord], cut: usize) -> usize {
    let mut c = cut;
    while c < events.len()
        && matches!(
            events[c].event,
            SessionEvent::ToolResult { .. } | SessionEvent::ToolError { .. }
        )
    {
        c += 1;
    }
    c
}

/// Pull `cut` back so it never falls between a `RunStarted` and the
/// `RunFinished` that closes it.
///
/// `load_run_markers` skips retired events, so retiring a `RunStarted` while
/// keeping its `RunFinished` leaves a dangling close that `ResumeCoordinator`
/// reads as a run it never saw start. Both markers must land on the same side
/// of the cut. Pulling back (rather than pushing forward) is the safe
/// direction: it compacts less, never more.
///
/// Consumed by the manual `/compact` cut selection. The session-split path
/// deliberately does **not** apply this guard: its `tail_start` falls inside
/// the currently-open run by construction (that run has no `RunFinished` in
/// the log yet), so the guard could only fire on a historical run whose close
/// was already lost — and pulling the split boundary back over it would
/// re-seed the child with a run the parent already finished.
pub(crate) fn snap_out_of_open_run(events: &[SessionEventRecord], cut: usize) -> usize {
    // One pass over the kept tail collects the runs it closes; the prefix scan
    // is then a set lookup rather than a nested search.
    let closed_after_cut: std::collections::HashSet<&str> = events[cut..]
        .iter()
        .filter_map(|r| match &r.event {
            SessionEvent::RunFinished { run_id, .. } => Some(run_id.as_str()),
            _ => None,
        })
        .collect();
    if closed_after_cut.is_empty() {
        return cut;
    }
    events[..cut]
        .iter()
        .position(|r| {
            matches!(&r.event, SessionEvent::RunStarted { run_id, .. }
                if closed_after_cut.contains(run_id.as_str()))
        })
        .unwrap_or(cut)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::{now_ms, MessageContent, RunOutcome, ToolOutput};

    fn rec(seq: u64, event: SessionEvent) -> SessionEventRecord {
        SessionEventRecord {
            seq,
            event,
            created_at_ms: now_ms(),
        }
    }

    fn user(seq: u64) -> SessionEventRecord {
        rec(
            seq,
            SessionEvent::UserMessage {
                turn_id: uuid::Uuid::new_v4(),
                content: MessageContent {
                    text: format!("u{seq}"),
                    blocks: vec![],
                    thinking: None,
                    thinking_signature: None,
                },
                synthetic: false,
                author_user_id: None,
                at: now_ms(),
            },
        )
    }

    fn tool_result(seq: u64) -> SessionEventRecord {
        rec(
            seq,
            SessionEvent::ToolResult {
                turn_id: uuid::Uuid::new_v4(),
                call_id: format!("c{seq}"),
                output: ToolOutput {
                    value: serde_json::json!({"ok": true}),
                    metadata: Default::default(),
                },
                at: now_ms(),
            },
        )
    }

    fn run(seq: u64, id: &str, finished: bool) -> SessionEventRecord {
        if finished {
            rec(
                seq,
                SessionEvent::RunFinished {
                    run_id: id.into(),
                    outcome: RunOutcome::Completed,
                    at: now_ms(),
                },
            )
        } else {
            rec(
                seq,
                SessionEvent::RunStarted {
                    run_id: id.into(),
                    at: now_ms(),
                    project_root: None,
                    envelope: None,
                },
            )
        }
    }

    #[test]
    fn tool_result_run_is_snapped_past() {
        let events = vec![user(1), user(2), tool_result(3), tool_result(4), user(5)];
        assert_eq!(snap_past_tool_results(&events, 2), 4);
        // No leading tool-result run: the cut stands.
        assert_eq!(snap_past_tool_results(&events, 1), 1);
        // A cut at the end is left alone (clamping is the caller's job).
        assert_eq!(snap_past_tool_results(&events, 5), 5);
    }

    #[test]
    fn cut_pulls_back_out_of_an_open_run() {
        // Run "r" starts at idx 1 and closes at idx 4. A cut inside it must
        // pull back to the RunStarted so both markers stay on the kept side.
        let events = vec![
            user(1),
            run(2, "r", false),
            user(3),
            user(4),
            run(5, "r", true),
            user(6),
        ];
        assert_eq!(snap_out_of_open_run(&events, 3), 1);
        // A cut before the run starts is untouched.
        assert_eq!(snap_out_of_open_run(&events, 1), 1);
        // No RunFinished in the kept tail → nothing to protect.
        assert_eq!(snap_out_of_open_run(&events[..4], 3), 3);
    }
}
