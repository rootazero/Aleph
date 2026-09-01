//! `RunReduction` — the one derivation of "what state is this run in".
//!
//! Three call sites used to answer this question in three different ways:
//! `resume_coordinator::classify_markers` (counted trailing `RunStarted`
//! markers), `resume_coordinator::compute_boundary_repairs` (scanned the whole
//! log for unanswered `ToolCallRequested`), and
//! `subagent_tool::recovery::classify` (matched `SubagentSpawned` against
//! `SubagentReturned`). None of them produced a named thing that said what
//! state the run was in, so "interrupted" meant three subtly different
//! predicates depending on who asked.
//!
//! Both functions here are **pure**: no I/O, no `async`, no globals. That is
//! what makes them falsifiable by mutation — a reduction that lived behind a
//! store trait would have one implementation per backend, and two shapes of
//! the same rule cancel each other out.
//!
//! Deliberately NOT in `src/harness/`: this is a read face over durable facts,
//! not Think→Act turn scheduling. R10's 12-file lock and `budget.rs::CEILING`
//! ratchet are untouched.

use crate::session::events::{EventSeq, SessionEvent, SessionEventRecord, Timestamp, TurnId};

/// How a session's run-marker tail reads.
///
/// **Deliberately two variants.** A third (`NeverStarted`, for a legacy log
/// with no run markers at all) was considered and rejected: no consumer today
/// would treat it differently from `Clean`, and a variant with no reader is a
/// claim the enum cannot honour — the same reason `ApprovalSource::Autoconfirm`
/// and six `ErrorKind` variants were removed (see `events.rs`). The next
/// variant arrives in the same commit as the consumer that reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunDisposition {
    /// Newest marker is `RunFinished` — nothing to recover.
    Clean,
    /// Interrupted; `trailing_starts` counts the consecutive `RunStarted`
    /// events after the last `RunFinished` (the crash-loop attempt counter).
    Interrupted { trailing_starts: usize },
}

/// Which run a dangling tool call belonged to.
///
/// This is the difference between a true sentence and a false one. Every
/// dangling call used to be told "the server restarted after this call was
/// dispatched", which is a lie about any call left over from an *earlier* run
/// that was never repaired — reachable when the crash happened while
/// `[resume] enabled = false`, or when a session aged past the recency filter
/// and was later resumed by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DanglingProvenance {
    /// Dispatched by the run that is being recovered right now.
    ThisRestart,
    /// Left over from an earlier run in the same session.
    ///
    /// Also the answer when the log carries no `RunStarted` at all (a legacy
    /// session, or a child that died before its run marker was durable): there
    /// is no current run for the call to belong to, so the weaker claim is the
    /// honest one. An unknown provenance must not be read as "this restart".
    EarlierRun,
}

/// A tool call that crossed the dispatch line and never got a receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DanglingCall {
    pub call_id: String,
    pub tool_name: String,
    pub turn_id: TurnId,
    pub provenance: DanglingProvenance,
}

/// What a run got done before it stopped. Scoped to the current run — see
/// [`reduce_run`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunProgress {
    pub tool_calls_dispatched: usize,
    /// Never greater than `tool_calls_dispatched`: this counts dispatched
    /// calls that got an answer, not answer events.
    pub tool_calls_answered: usize,
    pub assistant_messages: usize,
    /// `created_at_ms` of the last record in scope — the *recording* time, not
    /// a max over payload timestamps. The question is "when was it last
    /// alive", and recording order is the authoritative order.
    pub last_activity_at: Option<Timestamp>,
}

/// Everything the three consumers need to know about one session's runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunReduction {
    pub disposition: RunDisposition,
    /// `seq` of the last `RunStarted`. A **seq**, not an index: today every
    /// call site hands `reduce_run` a full log (`load_all_events`, or
    /// `get_events(id, None, None)`) rather than a page, but a seq stays
    /// meaningful regardless of how the caller sliced `events`, while an
    /// index would silently mean a different position.
    pub run_anchor: Option<EventSeq>,
    /// `run_id` of the last `RunStarted`.
    pub run_id: Option<String>,
    pub dangling: Vec<DanglingCall>,
    pub progress: RunProgress,
}

/// The one derivation of "is this interrupted".
///
/// `markers` is a run-marker sequence in `seq` order — either straight from
/// `SessionEventStore::load_run_markers`, or the marker subsequence of a full
/// log (which is what [`reduce_run`] hands it, so the two can never drift).
#[must_use]
pub fn reduce_disposition(markers: &[SessionEventRecord]) -> RunDisposition {
    let mut trailing_starts = 0usize;
    for record in markers.iter().rev() {
        match &record.event {
            SessionEvent::RunStarted { .. } => trailing_starts += 1,
            SessionEvent::RunFinished { .. } => break,
            // Precondition violation, not an input shape: `reduce_run` filters to
            // markers before calling here. A non-marker means the caller passed a
            // raw log, whose tail is almost always the dangling ToolCallRequested
            // — which would read as `Clean` and hide an interrupted run.
            _ => {
                debug_assert!(false, "reduce_disposition requires a marker-only slice");
                break;
            }
        }
    }
    if trailing_starts == 0 {
        RunDisposition::Clean
    } else {
        RunDisposition::Interrupted { trailing_starts }
    }
}

/// Reduce a session's event log to its run state.
///
/// Two passes. `events` must be in ascending `seq` order — the same
/// precondition [`reduce_disposition`] states for `markers`: pass one takes
/// the anchor as the `seq` of the `RunStarted` last *encountered* in the
/// slice, and pass two's provenance split compares against it by `seq`, so
/// an out-of-order slice would silently derive the wrong anchor and the
/// wrong disposition — no panic, just a false answer. Pass one finds the
/// anchor and the answered set, pass two attributes the dangling calls and
/// counts this run's progress.
#[must_use]
pub fn reduce_run(events: &[SessionEventRecord]) -> RunReduction {
    use std::collections::HashSet;

    debug_assert!(
        events.windows(2).all(|w| w[0].seq <= w[1].seq),
        "reduce_run requires `events` in ascending seq order; the anchor and the \
         disposition are both derived from iteration order"
    );

    // Pass 1: the anchor, the run id, and every call id that got an answer.
    let mut run_anchor: Option<EventSeq> = None;
    let mut run_id: Option<String> = None;
    let mut answered: HashSet<&str> = HashSet::new();
    let mut markers: Vec<SessionEventRecord> = Vec::new();
    for record in events {
        match &record.event {
            SessionEvent::RunStarted { run_id: rid, .. } => {
                run_anchor = Some(record.seq);
                run_id = Some(rid.clone());
                markers.push(record.clone());
            }
            SessionEvent::RunFinished { .. } => markers.push(record.clone()),
            SessionEvent::ToolResult { call_id, .. } | SessionEvent::ToolError { call_id, .. } => {
                answered.insert(call_id.as_str());
            }
            _ => {}
        }
    }

    // The disposition is not recomputed here — it is asked of the one function
    // that owns the question. G1 (proptest) pins that.
    let disposition = reduce_disposition(&markers);

    // Pass 2: attribute the dangling calls and count this run's progress.
    //
    // `in_scope` is the progress window: events after the anchor, or the whole
    // log when there is no anchor. That second case is not a fallback to
    // something looser — a log with no `RunStarted` holds exactly one run's
    // worth of events, so the whole log IS the scope.
    let mut dangling = Vec::new();
    let mut progress = RunProgress::default();
    let mut answered_in_scope: HashSet<&str> = HashSet::new();
    let mut dispatched_in_scope: Vec<&str> = Vec::new();
    for record in events {
        let in_scope = run_anchor.is_none_or(|anchor| record.seq > anchor);
        if in_scope {
            progress.last_activity_at = Some(record.created_at_ms);
        }
        match &record.event {
            SessionEvent::ToolCallRequested {
                turn_id,
                call_id,
                name,
                ..
            } => {
                if in_scope {
                    progress.tool_calls_dispatched += 1;
                    dispatched_in_scope.push(call_id.as_str());
                }
                if !answered.contains(call_id.as_str()) {
                    let provenance = match run_anchor {
                        Some(anchor) if record.seq > anchor => DanglingProvenance::ThisRestart,
                        _ => DanglingProvenance::EarlierRun,
                    };
                    dangling.push(DanglingCall {
                        call_id: call_id.clone(),
                        tool_name: name.clone(),
                        turn_id: *turn_id,
                        provenance,
                    });
                }
            }
            SessionEvent::ToolResult { call_id, .. } | SessionEvent::ToolError { call_id, .. } => {
                if in_scope {
                    answered_in_scope.insert(call_id.as_str());
                }
            }
            SessionEvent::AssistantMessage { .. } if in_scope => {
                progress.assistant_messages += 1;
            }
            _ => {}
        }
    }
    // Answered counts DISPATCHED calls that got a receipt, not receipt events:
    // a receipt for a call requested in an earlier run must not push this
    // number above `dispatched`.
    progress.tool_calls_answered = dispatched_in_scope
        .iter()
        .filter(|id| answered_in_scope.contains(*id))
        .count();

    RunReduction {
        disposition,
        run_anchor,
        run_id,
        dangling,
        progress,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::{MessageContent, RunOutcome};

    fn rec(seq: EventSeq, event: SessionEvent) -> SessionEventRecord {
        SessionEventRecord {
            seq,
            event,
            created_at_ms: seq as i64 * 10,
        }
    }

    fn started(run: &str) -> SessionEvent {
        SessionEvent::RunStarted {
            run_id: run.to_string(),
            at: 1,
            project_root: None,
        }
    }

    fn finished(run: &str) -> SessionEvent {
        SessionEvent::RunFinished {
            run_id: run.to_string(),
            outcome: RunOutcome::Completed,
            at: 2,
        }
    }

    fn requested(call: &str) -> SessionEvent {
        SessionEvent::ToolCallRequested {
            turn_id: TurnId::new_v4(),
            call_id: call.to_string(),
            name: "bash_exec".to_string(),
            input: serde_json::json!({}),
            at: 3,
        }
    }

    fn result_for(call: &str) -> SessionEvent {
        SessionEvent::ToolResult {
            turn_id: TurnId::new_v4(),
            call_id: call.to_string(),
            output: crate::session::events::ToolOutput {
                value: serde_json::json!("ok"),
                metadata: Default::default(),
            },
            at: 4,
        }
    }

    fn assistant(text: &str) -> SessionEvent {
        SessionEvent::AssistantMessage {
            turn_id: TurnId::new_v4(),
            content: MessageContent {
                text: text.to_string(),
                blocks: vec![],
                thinking: None,
                thinking_signature: None,
            },
            usage: None,
            at: 5,
        }
    }

    #[test]
    fn disposition_is_clean_when_the_newest_marker_finished() {
        let markers = vec![rec(1, started("a")), rec(2, finished("a"))];
        assert_eq!(reduce_disposition(&markers), RunDisposition::Clean);
    }

    #[test]
    fn disposition_counts_the_trailing_starts() {
        let markers = vec![
            rec(1, started("a")),
            rec(2, finished("a")),
            rec(3, started("b")),
            rec(4, started("c")),
        ];
        assert_eq!(
            reduce_disposition(&markers),
            RunDisposition::Interrupted { trailing_starts: 2 }
        );
    }

    /// G2 — the REACHABLE shape. Run `a` crashed while `[resume] enabled` was
    /// false, so nothing repaired `c1`; the user then sent a message, opening
    /// run `b`, which also crashed leaving `c2`. The two calls must not be
    /// told the same story.
    #[test]
    fn dangling_calls_are_attributed_to_their_own_run() {
        let events = vec![
            rec(1, started("a")),
            rec(2, requested("c1")),
            rec(3, started("b")),
            rec(4, requested("c2")),
        ];
        let r = reduce_run(&events);
        assert_eq!(r.run_anchor, Some(3));
        assert_eq!(r.run_id.as_deref(), Some("b"));
        assert_eq!(r.dangling.len(), 2);
        assert_eq!(r.dangling[0].call_id, "c1");
        assert_eq!(r.dangling[0].provenance, DanglingProvenance::EarlierRun);
        assert_eq!(r.dangling[1].call_id, "c2");
        assert_eq!(r.dangling[1].provenance, DanglingProvenance::ThisRestart);
    }

    /// G2b — the invariant-violation shape: a run that ended CLEANLY yet left a
    /// dangling call, i.e. one of `close_unexecuted_tool_uses` /
    /// `emit_deferred_tool_results` / the approval path failed to close it.
    /// The reduction must report the fact rather than swallow it, and must not
    /// upgrade it to "this restart".
    #[test]
    fn a_dangling_call_under_a_finished_run_is_reported_as_earlier() {
        let events = vec![
            rec(1, started("a")),
            rec(2, requested("c1")),
            rec(3, finished("a")),
            rec(4, started("b")),
        ];
        let r = reduce_run(&events);
        assert_eq!(
            r.disposition,
            RunDisposition::Interrupted { trailing_starts: 1 }
        );
        assert_eq!(r.dangling.len(), 1, "the fact must not be swallowed");
        assert_eq!(r.dangling[0].provenance, DanglingProvenance::EarlierRun);
    }

    #[test]
    fn an_answered_call_is_not_dangling() {
        let events = vec![
            rec(1, started("a")),
            rec(2, requested("c1")),
            rec(3, result_for("c1")),
        ];
        assert!(reduce_run(&events).dangling.is_empty());
    }

    #[test]
    fn a_log_with_no_run_marker_attributes_to_earlier_not_this_restart() {
        let events = vec![rec(1, requested("c1")), rec(2, assistant("hi"))];
        let r = reduce_run(&events);
        assert_eq!(r.run_anchor, None);
        assert_eq!(r.disposition, RunDisposition::Clean);
        assert_eq!(r.dangling.len(), 1);
        assert_eq!(r.dangling[0].provenance, DanglingProvenance::EarlierRun);
    }

    /// G4 — progress is scoped to the CURRENT run. A count that spans several
    /// runs names a different set.
    #[test]
    fn progress_counts_only_the_current_run() {
        let events = vec![
            rec(1, started("a")),
            rec(2, requested("old")),
            rec(3, result_for("old")),
            rec(4, assistant("run a said this")),
            rec(5, started("b")),
            rec(6, requested("c1")),
            rec(7, result_for("c1")),
            rec(8, requested("c2")),
            rec(9, assistant("run b said this")),
        ];
        let p = reduce_run(&events).progress;
        assert_eq!(p.tool_calls_dispatched, 2, "c1 and c2, not `old`");
        assert_eq!(p.tool_calls_answered, 1, "only c1 got a receipt");
        assert_eq!(p.assistant_messages, 1, "run a's message is not run b's");
        assert_eq!(p.last_activity_at, Some(90), "created_at_ms of seq 9");
    }

    #[test]
    fn answered_never_exceeds_dispatched() {
        // A stray receipt whose request lives in an earlier run must not push
        // `answered` above `dispatched`.
        let events = vec![
            rec(1, started("a")),
            rec(2, requested("old")),
            rec(3, started("b")),
            rec(4, result_for("old")),
            rec(5, requested("c1")),
        ];
        let p = reduce_run(&events).progress;
        assert_eq!(p.tool_calls_dispatched, 1);
        assert_eq!(p.tool_calls_answered, 0);
    }

    #[test]
    fn progress_covers_the_whole_log_when_there_is_no_run_marker() {
        let events = vec![
            rec(1, requested("c1")),
            rec(2, result_for("c1")),
            rec(3, assistant("hi")),
        ];
        let p = reduce_run(&events).progress;
        assert_eq!(p.tool_calls_dispatched, 1);
        assert_eq!(p.tool_calls_answered, 1);
        assert_eq!(p.assistant_messages, 1);
        assert_eq!(p.last_activity_at, Some(30));
    }

    /// G1 — the anti-drift device. `reduce_run` must ASK
    /// `reduce_disposition`, never re-derive. Falsify by adding any shortcut
    /// (e.g. "non-empty dangling implies Interrupted") to `reduce_run`.
    mod g1 {
        use super::*;
        use proptest::prelude::*;

        fn markers_of(events: &[SessionEventRecord]) -> Vec<SessionEventRecord> {
            events
                .iter()
                .filter(|r| {
                    matches!(
                        r.event,
                        SessionEvent::RunStarted { .. } | SessionEvent::RunFinished { .. }
                    )
                })
                .cloned()
                .collect()
        }

        /// 0 = RunStarted, 1 = RunFinished, 2 = ToolCallRequested,
        /// 3 = ToolResult, 4 = AssistantMessage.
        fn event_for(tag: u8, seq: EventSeq) -> SessionEvent {
            match tag % 5 {
                0 => started(&format!("r{seq}")),
                1 => finished(&format!("r{seq}")),
                2 => requested(&format!("c{seq}")),
                3 => result_for(&format!("c{seq}")),
                _ => assistant("x"),
            }
        }

        proptest! {
            #[test]
            fn reduce_run_asks_reduce_disposition(tags in prop::collection::vec(0u8..5, 0..40)) {
                let events: Vec<SessionEventRecord> = tags
                    .iter()
                    .enumerate()
                    .map(|(i, t)| rec(i as EventSeq + 1, event_for(*t, i as EventSeq + 1)))
                    .collect();
                prop_assert_eq!(
                    reduce_run(&events).disposition,
                    reduce_disposition(&markers_of(&events))
                );
            }
        }
    }
}
