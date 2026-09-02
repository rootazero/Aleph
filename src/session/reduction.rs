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
//! Every function here is **pure**: no I/O, no `async`, no globals. That is
//! what makes them falsifiable by mutation — a reduction that lived behind a
//! store trait would have one implementation per backend, and two shapes of
//! the same rule cancel each other out.
//!
//! ## Contradictions
//!
//! A session log has many writers — the harness, steering, resume, split,
//! compaction, backfill, the L0 fast path — each appending under its own seq,
//! so "the log is exactly what one protocol can produce" is not a rule this
//! reducer can enforce without refusing Aleph's own designed shapes. The
//! closed set [`LogContradiction`] therefore splits in two: two **REJECT**
//! kinds, where the slice cannot be reduced at all and the caller gets `Err`
//! (which may only ever mean "I do not know" — never `Clean`), and seven
//! **REPORT** kinds, each reduced under a *corrected reading* the tests pin
//! per kind. A report that changed no reading would be a no-op that reports
//! success, so every REPORT variant names what it changes.
//!
//! Deliberately NOT in `src/harness/`: this is a read face over durable facts,
//! not Think→Act turn scheduling. R10's 12-file lock and `budget.rs::CEILING`
//! ratchet are untouched.

use std::fmt;

use serde::Serialize;

use crate::session::events::{
    EventSeq, RunEnvelopeSnapshot, SessionEvent, SessionEventRecord, Timestamp, TurnId,
};

/// One thing a session log says that it must not say.
///
/// Closed set. Serialised under `kind` so a doctor finding, a resume receipt
/// and a sub-agent status all name the same nine words; [`tag`](Self::tag) is
/// the same word with the `session-log-` finding prefix, pinned to the serde
/// name by test.
///
/// **REJECT** (`rejects() == true`): the slice cannot be reduced — a reducer
/// that proceeded would derive the anchor and the disposition from a false
/// order. **REPORT**: reduced, with the reading the variant's doc names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LogContradiction {
    /// `seq` decreased between two adjacent records. REJECT.
    OutOfOrderSlice { at_seq: EventSeq },
    /// A slice handed to [`reduce_disposition`] as run markers carries some
    /// other event. REJECT — a raw log passed by mistake almost always ends
    /// on the dangling `ToolCallRequested`, which would read as `Clean`.
    NonMarkerInMarkerSlice { seq: EventSeq },
    /// A tool was dispatched after the last `RunFinished` with no
    /// `RunStarted` after that finish — the run whose `RunStarted` append
    /// failed. Reading: no run is open, so the dispatch is `EarlierRun`.
    ///
    /// Only a `ToolCallRequested` counts as activity here. `TurnStarted` and
    /// `UserMessage` are seeded BEFORE `RunStarted` by design, the gateway
    /// stamps `AssistantRunMeta` AFTER `RunFinished`, and the L0 fast path /
    /// simple engine / backfill write `AssistantMessage` rows under no marker
    /// at all — so any wider definition would report every real log. A tool
    /// dispatch can only come from a live Think→Act loop, and it is the only
    /// event whose reading (provenance) this report corrects. Reported only
    /// once at least one `RunStarted` has been seen: a marker-free log is one
    /// run's worth of events, read as such.
    UnmarkedActivity { first_seq: EventSeq },
    /// A `RunFinished` with no open run to close. Info-level: the split's
    /// copied tail, a fork seed that carries a `Cancelled` finish, and the
    /// `abandoned-*` / `delegated-*` closers can all produce it by design.
    /// Reading: unchanged (`open_run` was already `None`).
    FinishWithoutStart { seq: EventSeq, run_id: String },
    /// One `call_id` dispatched more than once. Reading: a receipt answers the
    /// NEAREST preceding dispatch of its id, so each dispatch pairs on its own
    /// and the unanswered one stays dangling — the whole-log set used to let
    /// the first receipt hide the second dispatch (③-D1).
    DuplicateDispatch {
        call_id: String,
        seqs: Vec<EventSeq>,
    },
    /// A `ToolResult` / `ToolError` whose `call_id` was never dispatched.
    /// Reading: it answers nothing and counts for nothing.
    ReceiptWithoutDispatch { call_id: String, seq: EventSeq },
    /// A second receipt for a dispatch that was already answered. Reading:
    /// the dispatch stays answered once; the extra receipt counts for nothing.
    DuplicateReceipt {
        call_id: String,
        seqs: Vec<EventSeq>,
    },
    /// A dispatch that was denied and never received the `ToolError` receipt
    /// the approval path owes it (③-D4). Reading: still dangling — the model
    /// must see the call answered — but [`DanglingCall::denied`] is set so
    /// the repair says "did not run" instead of "may have landed". `seq` is
    /// the dispatch's.
    DanglingDeniedCall { call_id: String, seq: EventSeq },
    /// `created_at_ms` is zero or earlier than the previous record's. Reading:
    /// this log's recency is unknown — a consumer must neither abandon nor
    /// resume on age. Reported once per log (the first offender).
    ClockAnomaly { seq: EventSeq },
}

impl LogContradiction {
    /// True for the kinds that make the slice unreducible.
    #[must_use]
    pub fn rejects(&self) -> bool {
        matches!(
            self,
            Self::OutOfOrderSlice { .. } | Self::NonMarkerInMarkerSlice { .. }
        )
    }

    /// The doctor finding id for this kind: `session-log-<kind>`, where
    /// `<kind>` is the serde name with `_` → `-`.
    #[must_use]
    pub fn tag(&self) -> &'static str {
        match self {
            Self::OutOfOrderSlice { .. } => "session-log-out-of-order-slice",
            Self::NonMarkerInMarkerSlice { .. } => "session-log-non-marker-in-marker-slice",
            Self::UnmarkedActivity { .. } => "session-log-unmarked-activity",
            Self::FinishWithoutStart { .. } => "session-log-finish-without-start",
            Self::DuplicateDispatch { .. } => "session-log-duplicate-dispatch",
            Self::ReceiptWithoutDispatch { .. } => "session-log-receipt-without-dispatch",
            Self::DuplicateReceipt { .. } => "session-log-duplicate-receipt",
            Self::DanglingDeniedCall { .. } => "session-log-dangling-denied-call",
            Self::ClockAnomaly { .. } => "session-log-clock-anomaly",
        }
    }
}

impl fmt::Display for LogContradiction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfOrderSlice { at_seq } => {
                write!(f, "event slice is not in ascending seq order at seq {at_seq}")
            }
            Self::NonMarkerInMarkerSlice { seq } => {
                write!(f, "run-marker slice carries a non-marker event at seq {seq}")
            }
            Self::UnmarkedActivity { first_seq } => write!(
                f,
                "a tool was dispatched after the last RunFinished with no RunStarted (first at seq {first_seq})"
            ),
            Self::FinishWithoutStart { seq, run_id } => {
                write!(f, "RunFinished `{run_id}` at seq {seq} closes no open run")
            }
            Self::DuplicateDispatch { call_id, seqs } => {
                write!(f, "call_id `{call_id}` dispatched more than once (seqs {seqs:?})")
            }
            Self::ReceiptWithoutDispatch { call_id, seq } => {
                write!(f, "receipt for call_id `{call_id}` at seq {seq} answers no dispatch")
            }
            Self::DuplicateReceipt { call_id, seqs } => {
                write!(f, "call_id `{call_id}` received more than one receipt (seqs {seqs:?})")
            }
            Self::DanglingDeniedCall { call_id, seq } => write!(
                f,
                "call_id `{call_id}` dispatched at seq {seq} was denied by the approval gate and never received a receipt"
            ),
            Self::ClockAnomaly { seq } => write!(
                f,
                "created_at_ms at seq {seq} is zero or earlier than the previous record's"
            ),
        }
    }
}

impl std::error::Error for LogContradiction {}

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
    /// Dispatched by the run that is being recovered right now: the log has
    /// an [`RunReduction::open_run`] and the call's `seq` is past it.
    ThisRestart,
    /// Left over from an earlier run in the same session.
    ///
    /// Also the answer whenever there is no open run for the call to belong
    /// to: a log with no `RunStarted` at all (legacy, or a child that died
    /// before its marker was durable), or one whose last `RunStarted` has a
    /// `RunFinished` after it — including the run whose own `RunStarted`
    /// append failed ([`LogContradiction::UnmarkedActivity`]). In every one of
    /// those the weaker claim is the honest one: an unknown provenance must
    /// not be read as "this restart".
    EarlierRun,
}

/// A tool call that crossed the dispatch line and never got a receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DanglingCall {
    pub call_id: String,
    pub tool_name: String,
    pub turn_id: TurnId,
    /// `seq` of the `ToolCallRequested`. A `call_id` can be dispatched more
    /// than once ([`LogContradiction::DuplicateDispatch`]), so the id alone
    /// does not name a dispatch; the seq does.
    pub seq: EventSeq,
    pub provenance: DanglingProvenance,
    /// A `ToolCallDenied` answered this dispatch and nothing else did: the
    /// call did not run, and the repair must say so rather than "may have
    /// landed". Always paired with a
    /// [`LogContradiction::DanglingDeniedCall`] in `contradictions`.
    pub denied: bool,
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

/// What the last `RunStarted` recorded — read off the marker itself, never
/// re-derived. This is the one place a resume reads the crashed run's
/// `project_root` and (④) its knob envelope from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunStartFacts {
    pub seq: EventSeq,
    pub run_id: String,
    pub project_root: Option<String>,
    pub envelope: Option<RunEnvelopeSnapshot>,
}

/// Everything the consumers need to know about one session's runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunReduction {
    pub disposition: RunDisposition,
    /// `seq` of the last `RunStarted` — the **scope** of `progress`, whether
    /// or not that run is still open. A **seq**, not an index: today every
    /// call site hands `reduce_run` a full log (`load_all_events`, or
    /// `get_events(id, None, None)`) rather than a page, but a seq stays
    /// meaningful regardless of how the caller sliced `events`, while an
    /// index would silently mean a different position.
    pub run_anchor: Option<EventSeq>,
    /// `run_id` of the last `RunStarted`.
    pub run_id: Option<String>,
    /// The last `RunStarted` **iff no `RunFinished` follows it** — the run
    /// that is actually open. Provenance and the ④ envelope read THIS, not
    /// `run_anchor`: a closed anchor is still the progress scope but no longer
    /// a run that a dangling call can belong to.
    pub open_run: Option<RunStartFacts>,
    pub dangling: Vec<DanglingCall>,
    pub progress: RunProgress,
    /// Every REPORT kind found, in detection order. Never holds a REJECT kind
    /// — those come back as `Err`.
    pub contradictions: Vec<LogContradiction>,
}

/// The ascending-`seq` precondition every reduction rests on, as a value.
///
/// Non-decreasing, not strictly increasing: a fixture that stamps every
/// record `seq: 1` is still a slice. Only a *decrease* is the shape that
/// derives a false anchor and a false disposition.
pub fn validate_slice(events: &[SessionEventRecord]) -> Result<(), LogContradiction> {
    match events.windows(2).find(|w| w[1].seq < w[0].seq) {
        Some(w) => Err(LogContradiction::OutOfOrderSlice { at_seq: w[1].seq }),
        None => Ok(()),
    }
}

fn is_marker(event: &SessionEvent) -> bool {
    matches!(
        event,
        SessionEvent::RunStarted { .. } | SessionEvent::RunFinished { .. }
    )
}

/// The one derivation of "is this interrupted".
///
/// `markers` is a run-marker sequence in `seq` order — either straight from
/// `SessionEventStore::load_run_markers`, or the marker subsequence of a full
/// log (which is what [`reduce_run`] hands it, so the two can never drift).
///
/// Both REJECT kinds are checked here, over the whole slice: a non-marker
/// anywhere is refused, not just one that happens to sit past the trailing
/// `RunFinished`. These used to be `debug_assert`s, which read as `Clean`
/// in release.
pub fn reduce_disposition(
    markers: &[SessionEventRecord],
) -> Result<RunDisposition, LogContradiction> {
    validate_slice(markers)?;
    if let Some(stray) = markers.iter().find(|r| !is_marker(&r.event)) {
        return Err(LogContradiction::NonMarkerInMarkerSlice { seq: stray.seq });
    }
    let trailing_starts = markers
        .iter()
        .rev()
        .take_while(|r| matches!(r.event, SessionEvent::RunStarted { .. }))
        .count();
    if trailing_starts == 0 {
        Ok(RunDisposition::Clean)
    } else {
        Ok(RunDisposition::Interrupted { trailing_starts })
    }
}

/// One dispatch, as the single ascending scan tracks it.
struct Dispatch<'a> {
    seq: EventSeq,
    call_id: &'a str,
    tool_name: &'a str,
    turn_id: TurnId,
    /// `seq` of the receipt that answered it, once one has.
    answered: Option<EventSeq>,
    denied: bool,
}

/// Reduce a session's event log to its run state.
///
/// `events` must be in ascending `seq` order — the same precondition
/// [`reduce_disposition`] states for `markers`; an out-of-order slice comes
/// back as `Err(OutOfOrderSlice)` rather than as a false answer. `Err` is
/// returned for the REJECT kinds only; every REPORT kind is reduced under its
/// corrected reading and listed in [`RunReduction::contradictions`].
///
/// One ascending scan. Dispatches and receipts are paired by **nearest
/// preceding dispatch of the same `call_id`** — never by a whole-log set of
/// answered ids, which let a reused id's first receipt hide its second
/// dispatch. The progress window is `run_anchor` (events after the last
/// `RunStarted`, or the whole log when there is none — not a looser fallback:
/// a log with no `RunStarted` holds exactly one run's worth of events, so the
/// whole log IS the scope). Provenance is decided against `open_run`.
pub fn reduce_run(events: &[SessionEventRecord]) -> Result<RunReduction, LogContradiction> {
    validate_slice(events)?;

    let mut contradictions: Vec<LogContradiction> = Vec::new();
    let mut dispatches: Vec<Dispatch<'_>> = Vec::new();
    let mut markers: Vec<SessionEventRecord> = Vec::new();
    let mut run_anchor: Option<EventSeq> = None;
    let mut run_id: Option<String> = None;
    let mut open_run: Option<RunStartFacts> = None;
    // `Some` while the tail is "after a RunFinished, before any RunStarted"
    // and a dispatch has been seen there; reset by the next marker.
    let mut unmarked_first: Option<EventSeq> = None;
    let mut after_finish_without_start = false;
    let mut saw_run_started = false;
    let mut prev_created: Option<Timestamp> = None;
    let mut clock_reported = false;

    for record in events {
        if !clock_reported
            && (record.created_at_ms == 0
                || prev_created.is_some_and(|prev| record.created_at_ms < prev))
        {
            contradictions.push(LogContradiction::ClockAnomaly { seq: record.seq });
            clock_reported = true;
        }
        prev_created = Some(record.created_at_ms);

        match &record.event {
            SessionEvent::RunStarted {
                run_id: rid,
                project_root,
                envelope,
                ..
            } => {
                run_anchor = Some(record.seq);
                run_id = Some(rid.clone());
                open_run = Some(RunStartFacts {
                    seq: record.seq,
                    run_id: rid.clone(),
                    project_root: project_root.clone(),
                    envelope: envelope.clone(),
                });
                saw_run_started = true;
                after_finish_without_start = false;
                unmarked_first = None;
                markers.push(record.clone());
            }
            SessionEvent::RunFinished { run_id: rid, .. } => {
                if open_run.is_none() {
                    contradictions.push(LogContradiction::FinishWithoutStart {
                        seq: record.seq,
                        run_id: rid.clone(),
                    });
                }
                open_run = None;
                after_finish_without_start = true;
                unmarked_first = None;
                markers.push(record.clone());
            }
            SessionEvent::ToolCallRequested {
                turn_id,
                call_id,
                name,
                ..
            } => {
                if after_finish_without_start && saw_run_started && unmarked_first.is_none() {
                    unmarked_first = Some(record.seq);
                }
                let prior: Vec<EventSeq> = dispatches
                    .iter()
                    .filter(|d| d.call_id == call_id)
                    .map(|d| d.seq)
                    .collect();
                if !prior.is_empty() {
                    note_duplicate(
                        &mut contradictions,
                        call_id,
                        prior,
                        record.seq,
                        |c, id| matches!(c, LogContradiction::DuplicateDispatch { call_id, .. } if call_id == id),
                        |id, seqs| LogContradiction::DuplicateDispatch { call_id: id, seqs },
                    );
                }
                dispatches.push(Dispatch {
                    seq: record.seq,
                    call_id,
                    tool_name: name,
                    turn_id: *turn_id,
                    answered: None,
                    denied: false,
                });
            }
            SessionEvent::ToolCallDenied { call_id, .. } => {
                if let Some(d) = dispatches
                    .iter_mut()
                    .rev()
                    .find(|d| d.call_id == call_id && d.answered.is_none())
                {
                    d.denied = true;
                }
            }
            SessionEvent::ToolResult { call_id, .. } | SessionEvent::ToolError { call_id, .. } => {
                match dispatches.iter_mut().rev().find(|d| d.call_id == call_id) {
                    None => contradictions.push(LogContradiction::ReceiptWithoutDispatch {
                        call_id: call_id.clone(),
                        seq: record.seq,
                    }),
                    Some(d) => match d.answered {
                        None => d.answered = Some(record.seq),
                        Some(first) => note_duplicate(
                            &mut contradictions,
                            call_id,
                            vec![first],
                            record.seq,
                            |c, id| matches!(c, LogContradiction::DuplicateReceipt { call_id, .. } if call_id == id),
                            |id, seqs| LogContradiction::DuplicateReceipt { call_id: id, seqs },
                        ),
                    },
                }
            }
            _ => {}
        }
    }

    // The disposition is not recomputed here — it is asked of the one function
    // that owns the question. G1 (proptest) pins that.
    let disposition = reduce_disposition(&markers)?;

    let open_seq = open_run.as_ref().map(|facts| facts.seq);
    let mut dangling = Vec::new();
    for d in dispatches.iter().filter(|d| d.answered.is_none()) {
        let provenance = match open_seq {
            Some(open) if d.seq > open => DanglingProvenance::ThisRestart,
            _ => DanglingProvenance::EarlierRun,
        };
        if d.denied {
            contradictions.push(LogContradiction::DanglingDeniedCall {
                call_id: d.call_id.to_string(),
                seq: d.seq,
            });
        }
        dangling.push(DanglingCall {
            call_id: d.call_id.to_string(),
            tool_name: d.tool_name.to_string(),
            turn_id: d.turn_id,
            seq: d.seq,
            provenance,
            denied: d.denied,
        });
    }
    if let Some(first_seq) = unmarked_first {
        contradictions.push(LogContradiction::UnmarkedActivity { first_seq });
    }

    let in_scope = |seq: EventSeq| run_anchor.is_none_or(|anchor| seq > anchor);
    let progress = RunProgress {
        tool_calls_dispatched: dispatches.iter().filter(|d| in_scope(d.seq)).count(),
        // Answered counts DISPATCHED calls that got a receipt, not receipt
        // events: a receipt for a call requested in an earlier run pairs with
        // that earlier dispatch and never reaches this number.
        tool_calls_answered: dispatches
            .iter()
            .filter(|d| in_scope(d.seq) && d.answered.is_some())
            .count(),
        assistant_messages: events
            .iter()
            .filter(|r| in_scope(r.seq) && matches!(r.event, SessionEvent::AssistantMessage { .. }))
            .count(),
        last_activity_at: events
            .iter()
            .rev()
            .find(|r| in_scope(r.seq))
            .map(|r| r.created_at_ms),
    };

    Ok(RunReduction {
        disposition,
        run_anchor,
        run_id,
        open_run,
        dangling,
        progress,
        contradictions,
    })
}

/// Record a duplicate (dispatch or receipt) as ONE contradiction per
/// `call_id`, extending its `seqs` if that id was already reported.
fn note_duplicate(
    contradictions: &mut Vec<LogContradiction>,
    call_id: &str,
    prior: Vec<EventSeq>,
    seq: EventSeq,
    is_same: impl Fn(&LogContradiction, &str) -> bool,
    make: impl FnOnce(String, Vec<EventSeq>) -> LogContradiction,
) {
    let existing = contradictions.iter_mut().find(|c| is_same(c, call_id));
    match existing {
        Some(
            LogContradiction::DuplicateDispatch { seqs, .. }
            | LogContradiction::DuplicateReceipt { seqs, .. },
        ) => seqs.push(seq),
        _ => {
            let mut seqs = prior;
            seqs.push(seq);
            contradictions.push(make(call_id.to_string(), seqs));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::{MessageContent, RunOutcome, TurnTrigger};

    /// Needles for the source census at the bottom of this module. Defined up
    /// here, far from every call site in these tests, so the census window
    /// (which looks forward from a call) can never contain them.
    const SWALLOWS: [&str; 2] = ["unwrap_or", ".ok()"];
    const WINDOW_LINES: usize = 5;
    const CALLS: [&str; 2] = ["reduce_run(", "reduce_disposition("];

    fn rec(seq: EventSeq, event: SessionEvent) -> SessionEventRecord {
        SessionEventRecord {
            seq,
            event,
            created_at_ms: seq as i64 * 10,
        }
    }

    fn rec_at(seq: EventSeq, created_at_ms: Timestamp, event: SessionEvent) -> SessionEventRecord {
        SessionEventRecord {
            seq,
            event,
            created_at_ms,
        }
    }

    fn started(run: &str) -> SessionEvent {
        SessionEvent::RunStarted {
            run_id: run.to_string(),
            at: 1,
            project_root: None,
            envelope: None,
        }
    }

    fn started_with_project(run: &str, root: &str) -> SessionEvent {
        SessionEvent::RunStarted {
            run_id: run.to_string(),
            at: 1,
            project_root: Some(root.to_string()),
            envelope: None,
        }
    }

    fn started_with_envelope(
        run: &str,
        envelope: Option<crate::session::events::RunEnvelopeSnapshot>,
    ) -> SessionEvent {
        SessionEvent::RunStarted {
            run_id: run.to_string(),
            at: 1,
            project_root: None,
            envelope,
        }
    }

    fn finished(run: &str) -> SessionEvent {
        finished_as(run, RunOutcome::Completed)
    }

    fn finished_as(run: &str, outcome: RunOutcome) -> SessionEvent {
        SessionEvent::RunFinished {
            run_id: run.to_string(),
            outcome,
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

    fn error_for(call: &str) -> SessionEvent {
        SessionEvent::ToolError {
            turn_id: TurnId::new_v4(),
            call_id: call.to_string(),
            error: "boom".to_string(),
            at: 4,
        }
    }

    fn denied(call: &str) -> SessionEvent {
        SessionEvent::ToolCallDenied {
            turn_id: TurnId::new_v4(),
            call_id: call.to_string(),
            reason: "operator said no".to_string(),
            at: 4,
        }
    }

    fn content(text: &str) -> MessageContent {
        MessageContent {
            text: text.to_string(),
            blocks: vec![],
            thinking: None,
            thinking_signature: None,
        }
    }

    fn assistant(text: &str) -> SessionEvent {
        SessionEvent::AssistantMessage {
            turn_id: TurnId::new_v4(),
            content: content(text),
            usage: None,
            at: 5,
        }
    }

    fn turn_started() -> SessionEvent {
        SessionEvent::TurnStarted {
            turn_id: TurnId::new_v4(),
            trigger: TurnTrigger::UserMessage,
            at: 5,
        }
    }

    fn user(text: &str) -> SessionEvent {
        SessionEvent::UserMessage {
            turn_id: TurnId::new_v4(),
            content: content(text),
            at: 5,
            synthetic: false,
            author_user_id: None,
        }
    }

    fn system(text: &str) -> SessionEvent {
        SessionEvent::SystemMessage {
            turn_id: TurnId::new_v4(),
            content: text.to_string(),
            at: 5,
        }
    }

    fn run_meta(run: &str) -> SessionEvent {
        SessionEvent::AssistantRunMeta {
            turn_id: TurnId::new_v4(),
            run_id: run.to_string(),
            context_tokens: 1,
            context_window: 2,
            total_tokens: 3,
            input_tokens: 1,
            output_tokens: 1,
            cost_usd: None,
            model: None,
            model_provider: None,
            at: 6,
        }
    }

    fn forked() -> SessionEvent {
        SessionEvent::SessionForked {
            parent_session_id: "agent:a/main".to_string(),
            at: 0,
        }
    }

    /// Reduce a log the test asserts is legal. A refusal is a test failure with
    /// the contradiction in the message — not a value to fall back from.
    fn reduced(events: &[SessionEventRecord]) -> RunReduction {
        match reduce_run(events) {
            Ok(r) => r,
            Err(c) => panic!("a legal log was refused: {c}"),
        }
    }

    fn tags(r: &RunReduction) -> Vec<&'static str> {
        r.contradictions.iter().map(LogContradiction::tag).collect()
    }

    // ---- the closed set -------------------------------------------------

    /// One sample per variant. `kind_index` is an exhaustive match, so a tenth
    /// variant does not compile until it is added here too — that is the
    /// "remember to update the other list" that cannot be forgotten.
    fn kind_index(c: &LogContradiction) -> usize {
        match c {
            LogContradiction::OutOfOrderSlice { .. } => 0,
            LogContradiction::NonMarkerInMarkerSlice { .. } => 1,
            LogContradiction::UnmarkedActivity { .. } => 2,
            LogContradiction::FinishWithoutStart { .. } => 3,
            LogContradiction::DuplicateDispatch { .. } => 4,
            LogContradiction::ReceiptWithoutDispatch { .. } => 5,
            LogContradiction::DuplicateReceipt { .. } => 6,
            LogContradiction::DanglingDeniedCall { .. } => 7,
            LogContradiction::ClockAnomaly { .. } => 8,
        }
    }
    const KIND_COUNT: usize = 9;

    fn one_of_each() -> Vec<LogContradiction> {
        let all = vec![
            LogContradiction::OutOfOrderSlice { at_seq: 1 },
            LogContradiction::NonMarkerInMarkerSlice { seq: 1 },
            LogContradiction::UnmarkedActivity { first_seq: 1 },
            LogContradiction::FinishWithoutStart {
                seq: 1,
                run_id: "r".into(),
            },
            LogContradiction::DuplicateDispatch {
                call_id: "c".into(),
                seqs: vec![1, 2],
            },
            LogContradiction::ReceiptWithoutDispatch {
                call_id: "c".into(),
                seq: 1,
            },
            LogContradiction::DuplicateReceipt {
                call_id: "c".into(),
                seqs: vec![1, 2],
            },
            LogContradiction::DanglingDeniedCall {
                call_id: "c".into(),
                seq: 1,
            },
            LogContradiction::ClockAnomaly { seq: 1 },
        ];
        let mut seen = vec![false; KIND_COUNT];
        for c in &all {
            seen[kind_index(c)] = true;
        }
        assert!(seen.iter().all(|s| *s), "one sample per variant");
        all
    }

    #[test]
    fn the_two_reject_kinds_are_exactly_out_of_order_and_non_marker() {
        for c in one_of_each() {
            let expected = matches!(kind_index(&c), 0 | 1);
            assert_eq!(c.rejects(), expected, "{c:?}");
        }
    }

    /// `tag()` and the serde `kind` are two spellings of one fact; this pins
    /// them to each other so a doctor finding id can never name a kind the
    /// wire does not.
    #[test]
    fn tags_are_derived_from_the_serde_kind() {
        for c in one_of_each() {
            let v = serde_json::to_value(&c).unwrap();
            let kind = v["kind"].as_str().expect("internally tagged");
            assert_eq!(c.tag(), format!("session-log-{}", kind.replace('_', "-")));
        }
    }

    // ---- REJECT ---------------------------------------------------------

    #[test]
    fn an_out_of_order_slice_is_refused_by_reduce_run() {
        let events = vec![rec(2, started("a")), rec(1, requested("c1"))];
        assert_eq!(
            reduce_run(&events).map(|r| r.disposition),
            Err(LogContradiction::OutOfOrderSlice { at_seq: 1 })
        );
        assert_eq!(
            validate_slice(&events),
            Err(LogContradiction::OutOfOrderSlice { at_seq: 1 })
        );
        // Equal seqs are tolerated: the precondition is non-decreasing, and a
        // fixture that stamps every record `seq: 1` is still a slice.
        assert_eq!(
            validate_slice(&[rec(1, started("a")), rec(1, requested("c1"))]),
            Ok(())
        );
    }

    #[test]
    fn an_out_of_order_marker_slice_is_refused_by_reduce_disposition() {
        let markers = vec![rec(2, started("a")), rec(1, finished("a"))];
        assert_eq!(
            reduce_disposition(&markers),
            Err(LogContradiction::OutOfOrderSlice { at_seq: 1 })
        );
    }

    /// Both positions: a non-marker at the tail (the raw-log-by-mistake shape,
    /// which used to read as `Clean`) and one BEFORE the trailing `RunFinished`,
    /// which a reverse scan that stops at the first `RunFinished` never sees.
    #[test]
    fn a_non_marker_in_the_marker_slice_is_refused() {
        let tail = vec![rec(1, started("a")), rec(2, requested("c1"))];
        assert_eq!(
            reduce_disposition(&tail),
            Err(LogContradiction::NonMarkerInMarkerSlice { seq: 2 })
        );
        let buried = vec![
            rec(1, requested("c1")),
            rec(2, started("a")),
            rec(3, finished("a")),
        ];
        assert_eq!(
            reduce_disposition(&buried),
            Err(LogContradiction::NonMarkerInMarkerSlice { seq: 1 })
        );
    }

    // ---- REPORT, each with its corrected reading -------------------------

    /// ③-D2: the run's `RunStarted` append failed, the run dispatched a tool
    /// and crashed. The markers say `Clean`; the dispatch says otherwise. The
    /// corrected reading: no run is open, so the call is `EarlierRun` — never
    /// "this restart".
    #[test]
    fn unmarked_activity_reads_as_earlier_run_with_no_open_run() {
        let events = vec![
            rec(1, started("r1")),
            rec(2, finished("r1")),
            rec(3, turn_started()),
            rec(4, user("again")),
            rec(5, requested("c2")),
        ];
        let r = reduced(&events);
        assert_eq!(tags(&r), vec!["session-log-unmarked-activity"]);
        assert_eq!(
            r.contradictions[0],
            LogContradiction::UnmarkedActivity { first_seq: 5 }
        );
        assert!(r.open_run.is_none(), "a closed run is not open");
        assert_eq!(r.run_anchor, Some(1), "the anchor is still the scope");
        assert_eq!(r.disposition, RunDisposition::Clean);
        assert_eq!(r.dangling.len(), 1);
        assert_eq!(r.dangling[0].provenance, DanglingProvenance::EarlierRun);
    }

    /// The shapes that are NOT unmarked activity, each a real producer:
    /// a marker-free log (legacy, or a child that died before its marker),
    /// and an assistant row written with no marker at all (the L0 fast path
    /// and the simple engine do exactly that, by design).
    #[test]
    fn unmarked_activity_is_only_a_tool_dispatch_after_a_seen_run() {
        let legacy = vec![rec(1, requested("c1")), rec(2, assistant("hi"))];
        assert!(reduced(&legacy).contradictions.is_empty());

        let fast_path_reply = vec![
            rec(1, started("r1")),
            rec(2, finished("r1")),
            rec(3, user("/help")),
            rec(4, assistant("usage: ...")),
        ];
        assert!(reduced(&fast_path_reply).contradictions.is_empty());
    }

    #[test]
    fn finish_without_start_is_reported_and_changes_no_reading() {
        let bare = vec![rec(1, finished("x"))];
        let r = reduced(&bare);
        assert_eq!(
            r.contradictions,
            vec![LogContradiction::FinishWithoutStart {
                seq: 1,
                run_id: "x".into()
            }]
        );
        assert!(r.open_run.is_none());
        assert_eq!(r.disposition, RunDisposition::Clean);

        let double = vec![
            rec(1, started("a")),
            rec(2, finished("a")),
            rec(3, finished("b")),
        ];
        let r = reduced(&double);
        assert_eq!(
            r.contradictions,
            vec![LogContradiction::FinishWithoutStart {
                seq: 3,
                run_id: "b".into()
            }]
        );
    }

    /// ③-D1. Two dispatches of one id, then one receipt: the receipt answers the
    /// NEAREST preceding dispatch, and the other stays dangling. The whole-log
    /// set used to read both as answered.
    #[test]
    fn duplicate_dispatch_pairs_each_dispatch_with_its_nearest_receipt() {
        let open_open = vec![
            rec(1, started("r1")),
            rec(2, requested("c1")),
            rec(3, requested("c1")),
            rec(4, result_for("c1")),
        ];
        let r = reduced(&open_open);
        assert_eq!(
            r.contradictions,
            vec![LogContradiction::DuplicateDispatch {
                call_id: "c1".into(),
                seqs: vec![2, 3]
            }]
        );
        assert_eq!(r.dangling.len(), 1, "the receipt answered seq 3, not seq 2");
        assert_eq!(r.dangling[0].seq, 2);
        assert_eq!(r.progress.tool_calls_dispatched, 2);
        assert_eq!(r.progress.tool_calls_answered, 1);

        // The weak-model shape: id reused AFTER its first call completed, then
        // the crash. The second dispatch is still dangling.
        let reused = vec![
            rec(1, started("r1")),
            rec(2, requested("c1")),
            rec(3, result_for("c1")),
            rec(4, requested("c1")),
        ];
        let r = reduced(&reused);
        assert_eq!(
            r.contradictions,
            vec![LogContradiction::DuplicateDispatch {
                call_id: "c1".into(),
                seqs: vec![2, 4]
            }]
        );
        assert_eq!(r.dangling.len(), 1, "the second dispatch is still dangling");
        assert_eq!(r.dangling[0].seq, 4);
        assert_eq!(r.dangling[0].provenance, DanglingProvenance::ThisRestart);
    }

    #[test]
    fn a_receipt_with_no_dispatch_is_reported_and_answers_nothing() {
        let events = vec![
            rec(1, started("r1")),
            rec(2, result_for("c9")),
            rec(3, requested("c1")),
        ];
        let r = reduced(&events);
        assert_eq!(
            r.contradictions,
            vec![LogContradiction::ReceiptWithoutDispatch {
                call_id: "c9".into(),
                seq: 2
            }]
        );
        assert_eq!(r.dangling.len(), 1);
        assert_eq!(r.dangling[0].call_id, "c1");
        assert_eq!(r.progress.tool_calls_answered, 0);
    }

    #[test]
    fn a_duplicate_receipt_is_reported_and_counts_once() {
        let events = vec![
            rec(1, started("r1")),
            rec(2, requested("c1")),
            rec(3, result_for("c1")),
            rec(4, error_for("c1")),
        ];
        let r = reduced(&events);
        assert_eq!(
            r.contradictions,
            vec![LogContradiction::DuplicateReceipt {
                call_id: "c1".into(),
                seqs: vec![3, 4]
            }]
        );
        assert!(r.dangling.is_empty());
        assert_eq!(
            r.progress.tool_calls_answered, 1,
            "one dispatch, one answer"
        );
    }

    /// ③-D4: a denial whose `ToolError` receipt never landed. Still dangling
    /// (the model must see the call was answered), but marked `denied` so the
    /// repair can say "did not run" instead of "may have landed".
    #[test]
    fn a_denied_call_with_no_receipt_is_dangling_and_flagged_denied() {
        let unreceipted = vec![
            rec(1, started("r1")),
            rec(2, requested("c1")),
            rec(3, denied("c1")),
        ];
        let r = reduced(&unreceipted);
        assert_eq!(
            r.contradictions,
            vec![LogContradiction::DanglingDeniedCall {
                call_id: "c1".into(),
                seq: 2
            }]
        );
        assert_eq!(r.dangling.len(), 1);
        assert!(r.dangling[0].denied);
        assert_eq!(r.dangling[0].provenance, DanglingProvenance::ThisRestart);

        // The normal approval path: denial then its receipt — nothing to say.
        let receipted = vec![
            rec(1, started("r1")),
            rec(2, requested("c1")),
            rec(3, denied("c1")),
            rec(4, error_for("c1")),
        ];
        let r = reduced(&receipted);
        assert!(r.contradictions.is_empty());
        assert!(r.dangling.is_empty());
    }

    #[test]
    fn a_clock_anomaly_is_reported_once_per_log() {
        let zero = vec![
            rec_at(1, 100, started("r1")),
            rec_at(2, 0, assistant("a")),
            rec_at(3, 50, assistant("b")),
        ];
        assert_eq!(
            reduced(&zero).contradictions,
            vec![LogContradiction::ClockAnomaly { seq: 2 }],
            "seq 3 is also anomalous but one report per log is enough"
        );
        let backwards = vec![rec_at(1, 100, started("r1")), rec_at(2, 90, assistant("a"))];
        assert_eq!(
            reduced(&backwards).contradictions,
            vec![LogContradiction::ClockAnomaly { seq: 2 }]
        );
        let equal = vec![
            rec_at(1, 100, started("r1")),
            rec_at(2, 100, assistant("a")),
        ];
        assert!(
            reduced(&equal).contradictions.is_empty(),
            "same millisecond is not backwards"
        );
    }

    // ---- open_run ---------------------------------------------------------

    #[test]
    fn open_run_is_the_last_run_started_when_nothing_finished_it() {
        let events = vec![
            rec(1, started("a")),
            rec(2, requested("c1")),
            rec(3, started_with_project("b", "/p")),
        ];
        let r = reduced(&events);
        assert_eq!(
            r.open_run,
            Some(RunStartFacts {
                seq: 3,
                run_id: "b".into(),
                project_root: Some("/p".into()),
                envelope: None,
            })
        );
        assert_eq!(r.run_anchor, Some(3));
    }

    /// ④ The envelope belongs to the run that is OPEN, not to whichever
    /// `RunStarted` the log happens to hold first. A crash-loop leaves several
    /// of them, each with its own knobs; replaying the earliest would resume
    /// the crashed run under settings a later attempt had already changed.
    #[test]
    fn open_run_carries_the_envelope_of_the_run_that_is_actually_open() {
        let stale = crate::session::events::RunEnvelopeSnapshot {
            exec_tier: Some("full".into()),
            ..Default::default()
        };
        let live = crate::session::events::RunEnvelopeSnapshot {
            exec_tier: Some("ask".into()),
            model: Some("m-live".into()),
            ..Default::default()
        };
        let events = vec![
            rec(1, started_with_envelope("a", Some(stale))),
            rec(2, finished("a")),
            rec(3, started_with_envelope("b", Some(live.clone()))),
            rec(4, requested("c1")),
        ];
        let r = reduced(&events);
        let facts = r.open_run.expect("the second run is still open");
        assert_eq!(facts.run_id, "b");
        assert_eq!(facts.envelope, Some(live));
    }

    /// A marker written before ④ existed reduces to `None`, which is what lets
    /// the coordinator count it as `unsnapshotted` instead of reading an empty
    /// envelope as "the gateway resolved nothing".
    #[test]
    fn a_pre_envelope_marker_reduces_to_no_envelope_at_all() {
        let events = vec![rec(1, started("a"))];
        let r = reduced(&events);
        assert_eq!(
            r.open_run.expect("open").envelope,
            None,
            "a legacy marker must not grow an envelope on the way through"
        );
    }

    #[test]
    fn open_run_is_none_once_a_run_finished_follows_it() {
        let events = vec![rec(1, started("a")), rec(2, finished("a"))];
        let r = reduced(&events);
        assert!(r.open_run.is_none());
        assert_eq!(
            r.run_anchor,
            Some(1),
            "the anchor outlives the run: it is the scope"
        );
        assert_eq!(r.run_id.as_deref(), Some("a"));
    }

    // ---- every prefix of every legal shape is green ------------------------

    struct LegalShape {
        name: &'static str,
        events: Vec<SessionEventRecord>,
        /// REPORT tags this shape is allowed to carry (by design). Everything
        /// else — every REJECT and every other REPORT — fails the shape.
        allowed: &'static [&'static str],
    }

    const FINISH_WITHOUT_START: &str = "session-log-finish-without-start";

    fn seq_log(events: Vec<SessionEvent>) -> Vec<SessionEventRecord> {
        events
            .into_iter()
            .enumerate()
            .map(|(i, e)| rec(i as EventSeq + 1, e))
            .collect()
    }

    /// The production shapes, in production order: the seed (`TurnStarted`,
    /// `UserMessage`) lands BEFORE `RunStarted`, and the gateway's
    /// `AssistantRunMeta` lands AFTER `RunFinished`. A guard that only knows
    /// the textbook order would misreport every real log.
    fn legal_shapes() -> Vec<LegalShape> {
        vec![
            LegalShape {
                name: "normal run",
                events: seq_log(vec![
                    turn_started(),
                    user("hi"),
                    started("r1"),
                    assistant("thinking"),
                    requested("c1"),
                    result_for("c1"),
                    assistant("done"),
                    finished("r1"),
                    run_meta("r1"),
                ]),
                allowed: &[],
            },
            LegalShape {
                // Crash mid-call; boot repair lands before the re-trigger's
                // marker; a resume skips re-seeding, so no second seed pair.
                name: "crash-loop with two RunStarted",
                events: seq_log(vec![
                    turn_started(),
                    user("hi"),
                    started("r1"),
                    assistant("thinking"),
                    requested("c1"),
                    error_for("c1"),
                    started("r2"),
                    assistant("again"),
                    requested("c2"),
                    result_for("c2"),
                    assistant("done"),
                    finished("r2"),
                    run_meta("r2"),
                ]),
                allowed: &[],
            },
            LegalShape {
                name: "session_split parent",
                events: seq_log(vec![
                    turn_started(),
                    user("hi"),
                    started("r1"),
                    assistant("thinking"),
                    requested("c1"),
                    result_for("c1"),
                    finished("split-1"),
                    run_meta("r1"),
                ]),
                allowed: &[],
            },
            LegalShape {
                // `SessionForked`, the summary, then the parent's fresh tail
                // copied verbatim — which can start with the previous run's
                // `RunFinished` and carry the current run's `RunStarted` —
                // then the split's own marker, then the run finishes on the
                // child under the original marker id.
                name: "session_split child",
                events: seq_log(vec![
                    forked(),
                    system("summary"),
                    finished("r0"),
                    run_meta("r0"),
                    turn_started(),
                    user("hi"),
                    started("r1"),
                    assistant("thinking"),
                    requested("c1"),
                    result_for("c1"),
                    started("split-1"),
                    assistant("done"),
                    finished("r1"),
                    run_meta("r1"),
                ]),
                allowed: &[FINISH_WITHOUT_START],
            },
            LegalShape {
                // Too old to resume: the coordinator closes the run with an
                // `abandoned-*` closer; the user's next message opens a new one.
                // `c1` stays dangling (`EarlierRun`) — a fact, not a
                // contradiction.
                name: "abandoned-<uuid> closer",
                events: seq_log(vec![
                    turn_started(),
                    user("hi"),
                    started("r1"),
                    assistant("thinking"),
                    requested("c1"),
                    finished_as("abandoned-1", RunOutcome::Abandoned),
                    turn_started(),
                    user("again"),
                    started("r2"),
                    assistant("done"),
                    finished("r2"),
                    run_meta("r2"),
                ]),
                allowed: &[FINISH_WITHOUT_START],
            },
            LegalShape {
                // Cron / heartbeat / team session: the scan closes the marker
                // with a `delegated-*` closer and the owning scheduler re-runs
                // by its own rule.
                name: "delegated-<uuid> closer",
                events: seq_log(vec![
                    turn_started(),
                    user("tick"),
                    started("r1"),
                    assistant("thinking"),
                    requested("c1"),
                    finished_as("delegated-1", RunOutcome::Abandoned),
                    turn_started(),
                    user("tick"),
                    started("r2"),
                    assistant("done"),
                    finished("r2"),
                    run_meta("r2"),
                ]),
                allowed: &[FINISH_WITHOUT_START],
            },
            LegalShape {
                name: "steering UserMessage in a tool gap",
                events: seq_log(vec![
                    turn_started(),
                    user("hi"),
                    started("r1"),
                    assistant("thinking"),
                    requested("c1"),
                    user("actually, use the other file"),
                    result_for("c1"),
                    assistant("done"),
                    finished("r1"),
                    run_meta("r1"),
                ]),
                allowed: &[],
            },
            LegalShape {
                // `fork::seed` copies the parent's prompt-bearing events with
                // no `RunStarted` (bookkeeping is not seeded) — but a
                // `Cancelled` `RunFinished` IS prompt-bearing and rides along,
                // so a tool dispatch can follow a `RunFinished` with no
                // `RunStarted` anywhere yet.
                name: "fork-seeded child",
                events: seq_log(vec![
                    forked(),
                    user("parent asked"),
                    assistant("parent said"),
                    finished_as("p-run", RunOutcome::Cancelled),
                    user("parent asked more"),
                    requested("c1"),
                    result_for("c1"),
                    assistant("parent said more"),
                    turn_started(),
                    user("child task"),
                    started("child"),
                    assistant("done"),
                    finished("child"),
                    run_meta("child"),
                ]),
                allowed: &[FINISH_WITHOUT_START],
            },
        ]
    }

    #[test]
    fn every_prefix_of_every_legal_shape_is_green() {
        for shape in legal_shapes() {
            for n in 0..=shape.events.len() {
                let prefix = &shape.events[..n];
                let Ok(r) = reduce_run(prefix) else {
                    panic!("{}: prefix of {n} was refused", shape.name);
                };
                for c in &r.contradictions {
                    assert!(
                        !c.rejects(),
                        "{}: prefix of {n} reports a REJECT kind: {c}",
                        shape.name
                    );
                    assert!(
                        shape.allowed.contains(&c.tag()),
                        "{}: prefix of {n} reports {} which this shape does not permit",
                        shape.name,
                        c.tag()
                    );
                }
            }
        }
    }

    /// The allowances above are not vacuous: the shapes that exhibit
    /// `FinishWithoutStart` do so, and the ones that merely permit it are named.
    #[test]
    fn the_finish_without_start_allowance_is_exercised_where_the_shape_produces_it() {
        let mut exhibited = Vec::new();
        for shape in legal_shapes() {
            let r = reduced(&shape.events);
            if tags(&r).contains(&FINISH_WITHOUT_START) {
                assert!(shape.allowed.contains(&FINISH_WITHOUT_START));
                exhibited.push(shape.name);
            }
        }
        assert_eq!(
            exhibited,
            vec!["session_split child", "fork-seeded child"],
            "the copied-tail shapes carry a RunFinished that closes nothing; the \
             abandoned / delegated closers pair with the open RunStarted"
        );
    }

    #[test]
    fn a_full_normal_run_reduces_to_nothing_to_recover() {
        let shape = legal_shapes().remove(0);
        let r = reduced(&shape.events);
        assert!(r.contradictions.is_empty());
        assert!(r.dangling.is_empty());
        assert!(r.open_run.is_none());
        assert_eq!(r.disposition, RunDisposition::Clean);
        assert_eq!(r.progress.tool_calls_dispatched, 1);
        assert_eq!(r.progress.tool_calls_answered, 1);
        assert_eq!(r.progress.assistant_messages, 2);
    }

    // ---- the round-1 tests, over `Result` --------------------------------

    #[test]
    fn disposition_is_clean_when_the_newest_marker_finished() {
        let markers = vec![rec(1, started("a")), rec(2, finished("a"))];
        assert_eq!(reduce_disposition(&markers), Ok(RunDisposition::Clean));
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
            Ok(RunDisposition::Interrupted { trailing_starts: 2 })
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
        let r = reduced(&events);
        assert_eq!(r.run_anchor, Some(3));
        assert_eq!(r.run_id.as_deref(), Some("b"));
        assert_eq!(r.open_run.as_ref().map(|f| f.seq), Some(3));
        assert_eq!(r.dangling.len(), 2);
        assert_eq!(r.dangling[0].call_id, "c1");
        assert_eq!(r.dangling[0].provenance, DanglingProvenance::EarlierRun);
        assert_eq!(r.dangling[1].call_id, "c2");
        assert_eq!(r.dangling[1].provenance, DanglingProvenance::ThisRestart);
        assert!(
            r.contradictions.is_empty(),
            "two crashes are two facts, not a contradiction"
        );
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
        let r = reduced(&events);
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
        assert!(reduced(&events).dangling.is_empty());
    }

    #[test]
    fn a_log_with_no_run_marker_attributes_to_earlier_not_this_restart() {
        let events = vec![rec(1, requested("c1")), rec(2, assistant("hi"))];
        let r = reduced(&events);
        assert_eq!(r.run_anchor, None);
        assert!(r.open_run.is_none());
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
        let p = reduced(&events).progress;
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
        let p = reduced(&events).progress;
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
        let p = reduced(&events).progress;
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
                    reduce_run(&events).map(|r| r.disposition),
                    reduce_disposition(&markers_of(&events))
                );
            }
        }
    }

    // ---- census -----------------------------------------------------------

    /// Criterion #8 at the source level: `Err` from this module means "I do
    /// not know", and no caller in `src/` may read it as a permissive value.
    /// Source-level because a swallowed refusal is runtime-indistinguishable
    /// from a clean log — that is exactly what makes it worth a guard.
    #[test]
    fn no_caller_swallows_a_refused_reduction() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        for entry in walkdir::WalkDir::new(&root)
            .into_iter()
            .filter_map(Result::ok)
        {
            if entry.path().extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let src = std::fs::read_to_string(entry.path())
                .unwrap_or_else(|e| panic!("{}: {e}", entry.path().display()));
            let code: Vec<&str> = src
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect();
            for (i, line) in code.iter().enumerate() {
                if !CALLS.iter().any(|needle| line.contains(needle)) {
                    continue;
                }
                let window = &code[i..(i + WINDOW_LINES).min(code.len())];
                if window
                    .iter()
                    .any(|l| SWALLOWS.iter().any(|s| l.contains(s)))
                {
                    offenders.push(format!(
                        "{} (code line {}, comments stripped)",
                        entry.path().display(),
                        i + 1
                    ));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "a refused reduction is read as a value at: {offenders:#?}"
        );
    }
}
