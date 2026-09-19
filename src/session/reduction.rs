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
//! closed set [`LogContradiction`] therefore splits in two: the **REJECT**
//! kinds, where the slice cannot be reduced at all and the caller gets `Err`
//! (which may only ever mean "I do not know" — never `Clean`), and the
//! **REPORT** kinds, each reduced under a *corrected reading* the tests pin
//! per kind. A report that changed no reading would be a no-op that reports
//! success, so every REPORT variant names what it changes. (The counts are
//! deliberately not written here — `tests::kind_index` is the census, and a
//! number in prose is a list that rots.) One REJECT kind is raised before a
//! slice exists at all: [`LogContradiction::UndecodableRecord`], for a row
//! the store could not decode, which [`reduce_marker_slice`] lifts into the
//! same `Err`.
//!
//! Deliberately NOT in `src/harness/`: this is a read face over durable facts,
//! not Think→Act turn scheduling. R10's 12-file lock and `budget.rs::CEILING`
//! ratchet are untouched.

use std::fmt;

use serde::Serialize;

use crate::session::events::{
    EventSeq, ParkReason, RunEnvelopeSnapshot, SessionEvent, SessionEventRecord, Timestamp, TurnId,
};
use crate::session::store::{MarkerSlice, UndecodableRecord};

/// One thing a session log says that it must not say.
///
/// Closed set. Serialised under `kind` so a doctor finding, a resume receipt
/// and a sub-agent status all name the same words; [`tag`](Self::tag) is
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
    /// A slice handed to [`reduce_disposition`] carries an event that bears on
    /// no disposition (see [`is_disposition_bearing`]). REJECT — a raw log
    /// passed by mistake almost always ends on the dangling
    /// `ToolCallRequested`, which would read as `Clean`.
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
    /// A `ResumeAttempted` with nothing to resume: no run is open and no
    /// real user message is unanswered at that point. Reading: the stamp
    /// still counts as an attempt if a `RunStarted` follows it before the
    /// next `RunFinished` — [`reduce_disposition`] counts the stamps in the
    /// tail, and a stamp that named an unanswered `UserMessage` a later run
    /// answers must still spend an attempt. With nothing after it the tail is
    /// `Clean` regardless. This report is what tells the operator the stamp
    /// named nothing at the moment it was written.
    ResumeWithoutTarget { seq: EventSeq },
    /// A `ToolCallParked` with no open dispatch of its `call_id` to pair
    /// with: none was ever dispatched, or the only unanswered one was already
    /// denied. Reading: ignored — it names nothing the log can pair it with,
    /// so no dangling call is marked parked by it. A park written AFTER its
    /// call's receipt (a detached job's card) is not this — see the arm in
    /// [`reduce_run`]; it is read like an approval after a receipt: silently.
    ParkedWithoutRequest { seq: EventSeq, call_id: String },
    /// A row of this slice did not decode on this build. REJECT — the reducer
    /// never saw the record, so no reading exists; the store names it
    /// ([`UndecodableRecord`]) and this is that name on the contradiction
    /// face, via [`From`].
    UndecodableRecord { seq: EventSeq },
}

impl LogContradiction {
    /// True for the kinds that make the slice unreducible.
    #[must_use]
    pub fn rejects(&self) -> bool {
        matches!(
            self,
            Self::OutOfOrderSlice { .. }
                | Self::NonMarkerInMarkerSlice { .. }
                | Self::UndecodableRecord { .. }
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
            Self::ResumeWithoutTarget { .. } => "session-log-resume-without-target",
            Self::ParkedWithoutRequest { .. } => "session-log-parked-without-request",
            Self::UndecodableRecord { .. } => "session-log-undecodable-record",
        }
    }
}

impl From<&UndecodableRecord> for LogContradiction {
    fn from(u: &UndecodableRecord) -> Self {
        Self::UndecodableRecord { seq: u.seq }
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
            Self::ResumeWithoutTarget { seq } => write!(
                f,
                "ResumeAttempted at seq {seq} names no open run and no unanswered message"
            ),
            Self::ParkedWithoutRequest { seq, call_id } => write!(
                f,
                "park for call_id `{call_id}` at seq {seq} names no unanswered dispatch"
            ),
            Self::UndecodableRecord { seq } => write!(
                f,
                "the record at seq {seq} could not be decoded by this build; run the doctor \
                 (`core/session-log`, fix=true) to retire that one record"
            ),
        }
    }
}

impl std::error::Error for LogContradiction {}

/// How a session's disposition-bearing tail reads.
///
/// **Deliberately three variants.** A `NeverStarted` (for a legacy log with
/// no run markers at all) was considered and rejected: no consumer today
/// would treat it differently from `Clean`, and a variant with no reader is a
/// claim the enum cannot honour — the same reason `ApprovalSource::Autoconfirm`
/// and six `ErrorKind` variants were removed (see `events.rs`). The next
/// variant arrives in the same commit as the consumer that reads it, as
/// `Unanswered` did with the coordinator's `check_unanswered`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunDisposition {
    /// No `RunStarted` after the last `RunFinished`, and no real user message
    /// left waiting there — nothing to recover.
    Clean,
    /// A `RunStarted` after the last `RunFinished`; `attempts` counts the
    /// `ResumeAttempted` stamps since that finish — the crash-loop ratchet,
    /// written by the coordinator BEFORE each retrigger, so a crash anywhere
    /// before the resumed run's own `RunStarted` still counts (§5.1).
    Interrupted { attempts: u32 },
    /// A real `UserMessage` after the last `RunFinished` (or the log start)
    /// with no `RunStarted` and no `AssistantMessage` after it: the crash
    /// landed in the seed→RunStarted window (§5.2). `user_seq` is that
    /// message's seq — the `ResumeAttempted.target` and the recency anchor;
    /// `attempts` counts the stamps written after it (the ones written FOR
    /// it), never a stamp that sits between the finish and the message.
    Unanswered { user_seq: EventSeq, attempts: u32 },
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
    /// Parked at a gate when the log ends and nothing answered the gate (§6.1):
    /// the repair says what it was waiting for. Cleared by a later
    /// `ToolCallApproved` (it went on to run — unknown again) or
    /// `ToolCallDenied` (the denied arm speaks instead), and a park stamped
    /// after a denial is not paired at all
    /// ([`LogContradiction::ParkedWithoutRequest`]) — so this is never `Some`
    /// on a call whose `denied` is set.
    pub parked: Option<ParkReason>,
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
    /// alive", and recording order is the authoritative order. A
    /// `ResumeAttempted` stamp is not in the running: it is the coordinator's
    /// intent, not the run's activity.
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

/// The run-marker set: the events `SessionEventStore::load_run_markers`
/// selects, and the marker half of what [`reduce_disposition`] reads (see
/// [`is_disposition_bearing`]). One predicate, so the SQL `IN (...)` list
/// (`store::MARKER_EVENT_TYPES`) is pinned equal to it by test rather than
/// being a second spelling of the same set.
pub(crate) fn is_marker(event: &SessionEvent) -> bool {
    matches!(
        event,
        SessionEvent::RunStarted { .. }
            | SessionEvent::RunFinished { .. }
            | SessionEvent::ResumeAttempted { .. }
    )
}

/// What [`reduce_disposition`] may be handed: the run markers and the two
/// message kinds that decide "was the user answered". Anything else in a
/// slice is the raw-log-by-mistake shape and is refused.
pub(crate) fn is_disposition_bearing(event: &SessionEvent) -> bool {
    is_marker(event)
        || matches!(
            event,
            SessionEvent::UserMessage { .. } | SessionEvent::AssistantMessage { .. }
        )
}

/// The one derivation of "is this interrupted, or is someone left waiting".
///
/// `markers` is a disposition-bearing sequence in `seq` order (see
/// [`is_disposition_bearing`]): the run markers straight from
/// `SessionEventStore::load_run_markers` — which cannot say `Unanswered`,
/// and is the list face's honest ceiling — or markers plus the message tail
/// the coordinator reads past the last `RunFinished`, or the bearing
/// subsequence of a full log (which is what [`reduce_run`] hands it, so the
/// faces can never drift).
///
/// The two REJECT kinds a decoded slice can carry are checked here, over the
/// whole slice: a stray event anywhere is refused, not just one that happens
/// to sit past the trailing `RunFinished`. These used to be `debug_assert`s,
/// which read as `Clean` in release. (The third, `UndecodableRecord`, never
/// reaches a reducer — see [`reduce_marker_slice`].)
///
/// The tail after the last `RunFinished` is interrupted iff it holds a
/// `RunStarted`; `attempts` is the number of `ResumeAttempted` stamps in that
/// same tail. Counting stamps rather than trailing `RunStarted` markers is
/// what makes a resume that dies before its own `RunStarted` still count
/// (§5.1): the stamp is written before the retrigger, the marker after.
/// Otherwise the tail is unanswered iff its last real `UserMessage` has no
/// `AssistantMessage` after it (§5.2); a harness-authored `synthetic` message
/// is never "the user waiting", and the attempts counted for it are the
/// stamps written after it.
pub fn reduce_disposition(
    markers: &[SessionEventRecord],
) -> Result<RunDisposition, LogContradiction> {
    validate_slice(markers)?;
    if let Some(stray) = markers.iter().find(|r| !is_disposition_bearing(&r.event)) {
        return Err(LogContradiction::NonMarkerInMarkerSlice { seq: stray.seq });
    }
    let since_finish = markers
        .iter()
        .rposition(|r| matches!(r.event, SessionEvent::RunFinished { .. }))
        .map_or(0, |i| i + 1);
    let tail = &markers[since_finish..];
    let stamps_in = |slice: &[SessionEventRecord]| {
        let n = slice
            .iter()
            .filter(|r| matches!(r.event, SessionEvent::ResumeAttempted { .. }))
            .count();
        u32::try_from(n).unwrap_or(u32::MAX)
    };
    if tail
        .iter()
        .any(|r| matches!(r.event, SessionEvent::RunStarted { .. }))
    {
        return Ok(RunDisposition::Interrupted {
            attempts: stamps_in(tail),
        });
    }
    let last_user = tail.iter().rposition(|r| {
        matches!(
            r.event,
            SessionEvent::UserMessage {
                synthetic: false,
                ..
            }
        )
    });
    match last_user {
        Some(i)
            if !tail[i + 1..]
                .iter()
                .any(|r| matches!(r.event, SessionEvent::AssistantMessage { .. })) =>
        {
            Ok(RunDisposition::Unanswered {
                user_seq: tail[i].seq,
                attempts: stamps_in(&tail[i + 1..]),
            })
        }
        _ => Ok(RunDisposition::Clean),
    }
}

/// [`reduce_disposition`] over what `SessionEventStore::load_run_markers`
/// hands back per session: a decoded slice reduces as usual; a slice the
/// store could not decode is refused under
/// [`LogContradiction::UndecodableRecord`] — "I do not know what this session
/// holds", never "it holds no markers" (criterion #8).
pub fn reduce_marker_slice(slice: &MarkerSlice) -> Result<RunDisposition, LogContradiction> {
    match slice {
        Ok(markers) => reduce_disposition(markers),
        Err(undecodable) => Err(LogContradiction::from(undecodable)),
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
    /// The newest gate event: `Some` after a `ToolCallParked`, `None` again
    /// after the gate is answered either way.
    parked: Option<ParkReason>,
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
    // The disposition-bearing subsequence, handed to `reduce_disposition`
    // untouched — the same slice the coordinator assembles from markers plus
    // the message tail, so the two readers cannot drift.
    let mut bearing: Vec<SessionEventRecord> = Vec::new();
    let mut run_anchor: Option<EventSeq> = None;
    let mut run_id: Option<String> = None;
    let mut open_run: Option<RunStartFacts> = None;
    // `Some` while the tail is "after a RunFinished, before any RunStarted"
    // and a dispatch has been seen there; reset by the next marker.
    let mut unmarked_first: Option<EventSeq> = None;
    let mut after_finish_without_start = false;
    let mut saw_run_started = false;
    // True while a real `UserMessage` sits after the last marker with no
    // `AssistantMessage` after it — the seed→RunStarted window a stamp may
    // legitimately target (§5.2). Cleared by the answer or by any marker.
    let mut pending_unanswered = false;
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
                pending_unanswered = false;
                bearing.push(record.clone());
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
                pending_unanswered = false;
                bearing.push(record.clone());
            }
            // The coordinator's intent stamp (§5.1 / §5.2). A marker: it
            // rides into the disposition, where it is counted as an attempt.
            // It names either the open run or a real user message nobody has
            // answered; with neither at this point it names nothing —
            // reported, not acted on.
            SessionEvent::ResumeAttempted { .. } => {
                if open_run.is_none() && !pending_unanswered {
                    contradictions.push(LogContradiction::ResumeWithoutTarget { seq: record.seq });
                }
                bearing.push(record.clone());
            }
            SessionEvent::UserMessage { synthetic, .. } => {
                if !synthetic {
                    pending_unanswered = true;
                }
                bearing.push(record.clone());
            }
            SessionEvent::AssistantMessage { .. } => {
                pending_unanswered = false;
                bearing.push(record.clone());
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
                    parked: None,
                });
            }
            // The gate's intent stamp (§6.1): the nearest open dispatch of its
            // id — unanswered and not already denied — is parked from here
            // until the gate is answered. Three other shapes:
            // * a dispatch of this id exists but every one is answered: a card
            //   raised AFTER the receipt. A detached (background) shell job's
            //   capability card lands after the call that spawned it has
            //   already returned its job id (`bash_exec::spawn_background`
            //   re-enters the call identity on purpose). Nothing to mark and
            //   nothing to report — the same after-receipt reading the
            //   `ToolCallApproved` / `ToolCallDenied` arms give;
            // * the nearest unanswered dispatch was already denied: the gate
            //   has spoken, a park after its answer names nothing;
            // * no dispatch of this id at all.
            // The last two are `ParkedWithoutRequest` — reported, not acted on.
            SessionEvent::ToolCallParked {
                call_id, reason, ..
            } => {
                let any_of_id = dispatches.iter().any(|d| d.call_id == call_id);
                match dispatches
                    .iter_mut()
                    .rev()
                    .find(|d| d.call_id == call_id && d.answered.is_none())
                {
                    Some(d) if !d.denied => d.parked = Some(*reason),
                    None if any_of_id => {}
                    _ => contradictions.push(LogContradiction::ParkedWithoutRequest {
                        seq: record.seq,
                        call_id: call_id.clone(),
                    }),
                }
            }
            // The gate was answered yes: the call went on to run, so from
            // here a crash is OUTCOME UNKNOWN again, not "never ran".
            SessionEvent::ToolCallApproved { call_id, .. } => {
                if let Some(d) = dispatches
                    .iter_mut()
                    .rev()
                    .find(|d| d.call_id == call_id && d.answered.is_none())
                {
                    d.parked = None;
                }
            }
            SessionEvent::ToolCallDenied { call_id, .. } => {
                if let Some(d) = dispatches
                    .iter_mut()
                    .rev()
                    .find(|d| d.call_id == call_id && d.answered.is_none())
                {
                    d.denied = true;
                    // The denied arm speaks for it now; a park it was in is over.
                    d.parked = None;
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
    let disposition = reduce_disposition(&bearing)?;

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
            parked: d.parked,
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
        // The coordinator's own `ResumeAttempted` is an intent record, not
        // something the run did: it is the newest in-scope event after every
        // boot, and counting it would date the run by its last ATTEMPT — the
        // recency filter would then never see a stamped run as too old.
        last_activity_at: events
            .iter()
            .rev()
            .find(|r| in_scope(r.seq) && !matches!(r.event, SessionEvent::ResumeAttempted { .. }))
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

/// Index of the first event this session recorded **on its own behalf**.
///
/// A `context=fork` child's log does not start with the child's work:
/// `subagent_spawner::fork::seed` copies a verbatim slice of the parent's
/// transcript in first and stamps it with a `SessionForked` marker, and the
/// spawner then emits the child's own `TurnStarted`. Reading such a log from
/// index 0 charges the parent's tool calls and assistant turns to the child —
/// and, on the path that matters, hands the parent's last answer back as the
/// child's finding.
///
/// The **last** `SessionForked`, not the first: `context::compact::
/// session_split` writes its own marker into a child that may already carry
/// one, and only the newest marker bounds the newest copied prefix.
///
/// Two ends, two answers, and neither is "count everything":
/// * **no fork marker** → `0`. An unforked log's own work starts at its
///   beginning, byte-identical to reading the whole slice.
/// * **a fork marker with no `TurnStarted` after it** → `events.len()`. The
///   child was seeded and died before its own turn opened, so its own scope is
///   empty. Answering `0` here would report the *parent's* dispatches as the
///   child's in-flight calls, which is the one reading that makes a model
///   re-run work it never started (judged the fail-closed way: an empty scope
///   reads as "it had not begun", which is what happened).
#[must_use]
pub fn own_work_start(events: &[SessionEventRecord]) -> usize {
    let Some(forked_at) = events
        .iter()
        .rposition(|r| matches!(r.event, SessionEvent::SessionForked { .. }))
    else {
        return 0;
    };
    let after = forked_at + 1;
    events[after..]
        .iter()
        .position(|r| matches!(r.event, SessionEvent::TurnStarted { .. }))
        .map_or(events.len(), |i| after + i)
}

/// The closed set [`LogContradiction`], walked as a list — for every census
/// over it, in this module and in `diagnostics::checks::session_log`, so a
/// kind that is added shows up in all of them from one place.
#[cfg(test)]
pub(crate) mod fixtures {
    use super::LogContradiction;

    /// `kind_index` is an exhaustive match, so a new variant does not
    /// compile until it is filed here too — that is the "remember to update
    /// the other list" that cannot be forgotten.
    pub(crate) fn kind_index(c: &LogContradiction) -> usize {
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
            LogContradiction::ResumeWithoutTarget { .. } => 9,
            LogContradiction::ParkedWithoutRequest { .. } => 10,
            LogContradiction::UndecodableRecord { .. } => 11,
        }
    }
    pub(crate) const KIND_COUNT: usize = 12;

    /// One sample per variant, asserted complete against `kind_index`.
    pub(crate) fn one_of_each_kind() -> Vec<LogContradiction> {
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
            LogContradiction::ResumeWithoutTarget { seq: 1 },
            LogContradiction::ParkedWithoutRequest {
                seq: 1,
                call_id: "c".into(),
            },
            LogContradiction::UndecodableRecord { seq: 1 },
        ];
        let mut seen = vec![false; KIND_COUNT];
        for c in &all {
            seen[kind_index(c)] = true;
        }
        assert!(seen.iter().all(|s| *s), "one sample per variant");
        all
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{kind_index, one_of_each_kind};
    use super::*;
    use crate::session::events::{MessageContent, ParkReason, RunOutcome, TurnTrigger};

    /// Needles for the source census at the bottom of this module. Defined up
    /// here, far from every call site in these tests, so the census window
    /// (which looks forward from a call) can never contain them.
    const SWALLOWS: [&str; 2] = ["unwrap_or", ".ok()"];
    const WINDOW_LINES: usize = 5;
    const CALLS: [&str; 3] = ["reduce_run(", "reduce_disposition(", "reduce_marker_slice("];

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

    fn attempted(target: EventSeq, attempt: u32) -> SessionEvent {
        SessionEvent::ResumeAttempted { target, attempt }
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

    fn parked(call: &str, reason: ParkReason) -> SessionEvent {
        SessionEvent::ToolCallParked {
            turn_id: TurnId::new_v4(),
            call_id: call.to_string(),
            reason,
        }
    }

    fn approved(call: &str) -> SessionEvent {
        SessionEvent::ToolCallApproved {
            turn_id: TurnId::new_v4(),
            call_id: call.to_string(),
            by: crate::session::events::ApprovalSource::User,
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
            context_tokens: Some(1),
            context_window: Some(2),
            total_tokens: Some(3),
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
    // The samples live in `super::fixtures` so every census over the set
    // (here and `diagnostics::checks::session_log`) walks ONE list.

    #[test]
    fn the_three_reject_kinds_are_exactly_out_of_order_non_marker_and_undecodable() {
        for c in one_of_each_kind() {
            let expected = matches!(kind_index(&c), 0 | 1 | 11);
            assert_eq!(c.rejects(), expected, "{c:?}");
        }
    }

    /// The one REJECT kind the reducer does not raise itself: a slice whose
    /// row did not decode never reaches `reduce_disposition`, so
    /// `reduce_marker_slice` refuses it under its own kind — and passes a
    /// decoded slice through to the same verdict the reducer gives directly.
    #[test]
    fn a_marker_slice_that_did_not_decode_is_refused_under_its_own_kind() {
        let slice: MarkerSlice = Err(UndecodableRecord {
            seq: 4,
            kind_tag: None,
            error: "x".into(),
        });
        assert_eq!(
            reduce_marker_slice(&slice),
            Err(LogContradiction::UndecodableRecord { seq: 4 })
        );
        assert_eq!(
            reduce_marker_slice(&Ok(vec![rec(1, started("a"))])),
            Ok(RunDisposition::Interrupted { attempts: 0 })
        );
    }

    /// `tag()` and the serde `kind` are two spellings of one fact; this pins
    /// them to each other so a doctor finding id can never name a kind the
    /// wire does not.
    #[test]
    fn tags_are_derived_from_the_serde_kind() {
        for c in one_of_each_kind() {
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
    /// "this restart". The bearing slice reads the user's message as
    /// `Unanswered` (§5.2): no `RunStarted` and no `AssistantMessage` follow
    /// it, and a dispatch bears on no disposition — the run that picked the
    /// message up left no marker and no answer, so the message is still owed
    /// one.
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
        assert_eq!(
            r.disposition,
            RunDisposition::Unanswered {
                user_seq: 4,
                attempts: 0
            }
        );
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

    /// §6.1: the gate's intent stamp. A dispatch whose newest gate event is a
    /// `ToolCallParked` never ran, and the dangling call says which gate it
    /// was waiting on. Not a contradiction — it is the designed crash shape.
    #[test]
    fn a_parked_dangling_call_carries_its_reason() {
        let r = reduced(&[
            rec(1, started("a")),
            rec(2, requested("c1")),
            rec(3, parked("c1", ParkReason::Approval)),
        ]);
        assert_eq!(r.dangling[0].parked, Some(ParkReason::Approval));
        assert!(!r.dangling[0].denied && tags(&r).is_empty());
    }

    /// An answered gate ends the park: approved ⇒ the call went on to run, so
    /// a crash after that is OUTCOME UNKNOWN again; denied ⇒ the denied arm,
    /// never the parked one.
    #[test]
    fn an_answered_gate_ends_the_park() {
        let r = reduced(&[
            rec(1, started("a")),
            rec(2, requested("c1")),
            rec(3, parked("c1", ParkReason::Approval)),
            rec(4, approved("c1")),
        ]);
        assert_eq!(
            r.dangling[0].parked, None,
            "approved ⇒ it went on to run ⇒ unknown, not never-ran"
        );
        let r = reduced(&[
            rec(1, started("a")),
            rec(2, requested("c1")),
            rec(3, parked("c1", ParkReason::PreHook)),
            rec(4, denied("c1")),
        ]);
        assert!(r.dangling[0].parked.is_none() && r.dangling[0].denied);
    }

    /// Same pairing rule as receipts (③-D1): a park names the NEAREST
    /// unanswered dispatch of its id, so a reused `call_id` whose first
    /// dispatch was answered pairs the park with the second.
    #[test]
    fn a_park_pairs_with_the_nearest_unanswered_dispatch_of_its_id() {
        let r = reduced(&[
            rec(1, started("a")),
            rec(2, requested("c1")),
            rec(3, result_for("c1")),
            rec(4, requested("c1")),
            rec(5, parked("c1", ParkReason::Clarification)),
        ]);
        assert_eq!(
            (r.dangling.len(), r.dangling[0].seq, r.dangling[0].parked),
            (1, 4, Some(ParkReason::Clarification))
        );
    }

    /// A stamp with nothing to pair with is REPORTED and changes no reading:
    /// it names no dispatch, so there is nothing for it to mark parked.
    #[test]
    fn a_park_without_a_dispatch_is_reported_and_ignored() {
        let r = reduced(&[
            rec(1, started("a")),
            rec(2, parked("ghost", ParkReason::Approval)),
        ]);
        assert!(r.dangling.is_empty());
        assert_eq!(tags(&r), vec!["session-log-parked-without-request"]);
    }

    /// A park stamped after the gate already denied the call names nothing:
    /// the denial is kept (the denied arm speaks), `parked` stays `None`, and
    /// the stray stamp is reported. Production cannot write this order (both
    /// deny exits return before the stamp), so it is a malformed log — read
    /// the fail-closed way, not the confident one.
    #[test]
    fn a_park_after_a_denial_is_reported_and_the_denial_kept() {
        let r = reduced(&[
            rec(1, started("a")),
            rec(2, requested("c1")),
            rec(3, denied("c1")),
            rec(4, parked("c1", ParkReason::Approval)),
        ]);
        assert_eq!(r.dangling.len(), 1);
        assert!(r.dangling[0].denied && r.dangling[0].parked.is_none());
        assert_eq!(
            tags(&r),
            vec![
                "session-log-parked-without-request",
                "session-log-dangling-denied-call",
            ]
        );
    }

    /// A park stamped AFTER its call's receipt is a detached job's card: a
    /// background shell command asks for a capability after the `bash` call
    /// that spawned it has already returned its job id. The call is answered,
    /// so nothing dangles; and it is not a contradiction — the log is exactly
    /// what that path writes by design. The same reading an approval after a
    /// receipt gets.
    #[test]
    fn a_park_after_the_receipt_marks_nothing_and_reports_nothing() {
        let r = reduced(&[
            rec(1, started("a")),
            rec(2, requested("c1")),
            rec(3, result_for("c1")),
            rec(4, parked("c1", ParkReason::Approval)),
        ]);
        assert!(r.dangling.is_empty());
        assert!(tags(&r).is_empty(), "{:?}", tags(&r));
        assert_eq!(r.progress.tool_calls_answered, 1);
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

    /// §5.6: a `ResumeAttempted` with no run open at the moment it was
    /// written is REPORTED — and the two readings of it are pinned side by
    /// side. With nothing after it, the tail holds no `RunStarted` and the
    /// disposition is `Clean` (the stamp changes nothing). With a `RunStarted`
    /// after it and before the next `RunFinished`, the stamp is in the
    /// interrupted tail and it COUNTS: `reduce_disposition` sees markers only,
    /// and a stamp T7 lets target an unanswered `UserMessage` that a later run
    /// answers must still spend an attempt. The report is what tells the
    /// operator the stamp named nothing when it was written.
    #[test]
    fn a_stamp_with_no_open_run_is_reported_and_counts_only_if_a_run_follows() {
        let nothing_follows = vec![
            rec(1, started("a")),
            rec(2, finished("a")),
            rec(3, attempted(1, 1)),
        ];
        let r = reduced(&nothing_follows);
        assert_eq!(r.disposition, RunDisposition::Clean);
        assert_eq!(tags(&r), vec!["session-log-resume-without-target"]);
        assert_eq!(
            r.contradictions[0],
            LogContradiction::ResumeWithoutTarget { seq: 3 }
        );

        let a_run_follows = vec![
            rec(1, finished("a")),
            rec(2, attempted(1, 1)),
            rec(3, started("b")),
        ];
        let r = reduced(&a_run_follows);
        assert_eq!(
            r.disposition,
            RunDisposition::Interrupted { attempts: 1 },
            "the stamp sits in the interrupted tail, so it is an attempt"
        );
        assert_eq!(
            tags(&r),
            vec![
                "session-log-finish-without-start",
                "session-log-resume-without-target"
            ],
            "and it is still reported: no run was open when it was written"
        );
    }

    /// A coordinator's intent stamp is not something the run DID. The recency
    /// filter reads `last_activity_at` to decide "too old to resume", and a
    /// stamp that refreshed it would let every boot's own attempt resurrect a
    /// run the operator's `max_age_secs` had already ruled out.
    #[test]
    fn last_activity_at_is_not_refreshed_by_an_intent_stamp() {
        let events = vec![
            rec(1, started("a")),
            rec(2, requested("c1")),
            rec(3, attempted(1, 1)),
        ];
        let p = reduced(&events).progress;
        assert_eq!(
            p.last_activity_at,
            Some(20),
            "created_at_ms of the dispatch at seq 2, not of the stamp at seq 3"
        );
        // A run that opened, recorded nothing, and was then stamped: no
        // activity at all, so the caller falls back to the marker's own time.
        let only_stamped = vec![rec(1, started("a")), rec(2, attempted(1, 1))];
        assert_eq!(reduced(&only_stamped).progress.last_activity_at, None);
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
                // The same crash-loop as the resume coordinator writes it
                // since §5.1: the intent stamp (`target` = the crashed run's
                // `RunStarted`, seq 3 here) lands AFTER the boundary repair
                // and BEFORE the re-trigger's own marker.
                name: "crash-loop with an intent stamp",
                events: seq_log(vec![
                    turn_started(),
                    user("hi"),
                    started("r1"),
                    assistant("thinking"),
                    requested("c1"),
                    error_for("c1"),
                    attempted(3, 1),
                    started("r2"),
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
                // §6.1: the gate stamps its intent BEFORE parking; the human
                // answers; the call runs. Neither the park nor the approval
                // is a contradiction — they are the designed shape.
                name: "approval-parked call, approved, then result",
                events: seq_log(vec![
                    turn_started(),
                    user("hi"),
                    started("r1"),
                    requested("c1"),
                    parked("c1", ParkReason::Approval),
                    approved("c1"),
                    result_for("c1"),
                    assistant("done"),
                    finished("r1"),
                    run_meta("r1"),
                ]),
                allowed: &[],
            },
            LegalShape {
                // The sandbox capability-elevation card is raised INSIDE the
                // shell tool's own `execute`, after the confirm gate already
                // asked and was answered: two park / release pairs on ONE
                // call, then its receipt (`sandbox::workspace` since the final
                // review's C1 — before it the second pair had no release, and
                // a crash during the command read "never ran").
                name: "elevation-parked call, approved, then result",
                events: seq_log(vec![
                    turn_started(),
                    user("hi"),
                    started("r1"),
                    requested("c1"),
                    parked("c1", ParkReason::Approval),
                    approved("c1"),
                    parked("c1", ParkReason::Approval),
                    approved("c1"),
                    result_for("c1"),
                    assistant("done"),
                    finished("r1"),
                    run_meta("r1"),
                ]),
                allowed: &[],
            },
            LegalShape {
                // A background shell job asks for a capability after the
                // `bash` call that spawned it already returned its job id
                // (`bash_exec::spawn_background` re-enters the call identity),
                // so its park and its approval land AFTER the receipt.
                name: "background job's capability card after the spawn call returned",
                events: seq_log(vec![
                    turn_started(),
                    user("hi"),
                    started("r1"),
                    requested("c1"),
                    result_for("c1"),
                    parked("c1", ParkReason::Approval),
                    approved("c1"),
                    assistant("done"),
                    finished("r1"),
                    run_meta("r1"),
                ]),
                allowed: &[],
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
            LegalShape {
                // §5.2: the seed landed and the process died before the run's
                // own `RunStarted` — a log that ends on the user's message.
                name: "seed then crash before RunStarted",
                events: seq_log(vec![turn_started(), user("hi")]),
                allowed: &[],
            },
            LegalShape {
                // The same crash, stamped twice by two boots that each died
                // again before the run started, then given up on: the
                // coordinator's `abandoned-*` closer pairs with no
                // `RunStarted`, which is `FinishWithoutStart` by design.
                name: "unanswered then abandoned closer",
                events: seq_log(vec![
                    turn_started(),
                    user("hi"),
                    attempted(2, 1),
                    attempted(2, 2),
                    finished_as("abandoned-1", RunOutcome::Abandoned),
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
            vec![
                "session_split child",
                "fork-seeded child",
                "unanswered then abandoned closer"
            ],
            "the copied-tail shapes carry a RunFinished that closes nothing, and so \
             does the closer of a seed no run ever answered; the abandoned / \
             delegated closers of an interrupted run pair with the open RunStarted"
        );
    }

    /// The elevation shape, read at the three moments a crash could land
    /// (final review C1): with the card up it is "never ran" (`parked`); the
    /// instant the release lands it is OUTCOME UNKNOWN again (`parked: None`,
    /// still dangling — the command is running); at the receipt nothing
    /// dangles. The prefix test above only says no prefix is a contradiction;
    /// this says what each prefix READS as, which is the sentence the model
    /// is handed after a restart.
    #[test]
    fn an_elevation_parked_call_reads_unknown_once_released_and_clean_at_its_receipt() {
        let shape = legal_shapes()
            .into_iter()
            .find(|s| s.name == "elevation-parked call, approved, then result")
            .expect("the shape is in the list");
        let release = shape
            .events
            .iter()
            .rposition(|r| matches!(r.event, SessionEvent::ToolCallApproved { .. }))
            .expect("the shape carries the elevation gate's release");
        assert!(
            matches!(
                shape.events[release - 1].event,
                SessionEvent::ToolCallParked {
                    reason: ParkReason::Approval,
                    ..
                }
            ),
            "the release follows the elevation gate's own park"
        );

        let card_up = reduced(&shape.events[..release]);
        assert_eq!(
            (card_up.dangling.len(), card_up.dangling[0].parked),
            (1, Some(ParkReason::Approval)),
            "with the card up the call never ran"
        );
        let released = reduced(&shape.events[..=release]);
        assert_eq!(
            (released.dangling.len(), released.dangling[0].parked),
            (1, None),
            "released ⇒ the command is running: unknown, not never-ran"
        );
        assert!(!released.dangling[0].denied);
        let whole = reduced(&shape.events);
        assert!(whole.dangling.is_empty() && tags(&whole).is_empty());
        assert_eq!(whole.progress.tool_calls_answered, 1);
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

    /// Two bare `RunStarted` after the last finish used to count as two
    /// attempts. They are two crashes, not two resumes: nobody has stamped an
    /// intent to resume this run, so the ratchet reads 0.
    #[test]
    fn bare_run_starts_after_a_finish_are_interrupted_with_no_attempts() {
        let markers = vec![
            rec(1, started("a")),
            rec(2, finished("a")),
            rec(3, started("b")),
            rec(4, started("c")),
        ];
        assert_eq!(
            reduce_disposition(&markers),
            Ok(RunDisposition::Interrupted { attempts: 0 })
        );
    }

    /// §5.1 / §5.6: the ratchet counts `ResumeAttempted` stamps since the last
    /// `RunFinished`, not the `RunStarted` markers a resume happened to leave
    /// behind. A resume that dies before its run's own `RunStarted` still
    /// wrote its stamp, so it still counts.
    #[test]
    fn attempts_count_intent_stamps_not_trailing_starts() {
        // Three boots that each stamped intent and crashed before RunStarted.
        let markers = vec![
            rec(1, started("a")),
            rec(2, attempted(1, 1)),
            rec(3, attempted(1, 2)),
            rec(4, attempted(1, 3)),
        ];
        assert_eq!(
            reduce_disposition(&markers),
            Ok(RunDisposition::Interrupted { attempts: 3 })
        );
        // Two RunStarted with no stamp between them: the old counter said 2,
        // the ratchet says 0 — nobody has *tried* to resume this yet.
        let markers = vec![rec(1, started("a")), rec(2, started("b"))];
        assert_eq!(
            reduce_disposition(&markers),
            Ok(RunDisposition::Interrupted { attempts: 0 })
        );
        // A RunFinished resets the count.
        let markers = vec![
            rec(1, started("a")),
            rec(2, attempted(1, 1)),
            rec(3, finished("a")),
            rec(4, started("b")),
        ];
        assert_eq!(
            reduce_disposition(&markers),
            Ok(RunDisposition::Interrupted { attempts: 0 })
        );
    }

    /// §5.2: a real `UserMessage` after the last `RunFinished` with no
    /// `RunStarted` and no `AssistantMessage` after it is the seed→RunStarted
    /// crash window, and it has its own word. The slice is disposition-
    /// bearing (markers plus the two message kinds); anything else is still
    /// the raw-log-by-mistake shape and is refused.
    #[test]
    fn a_seeded_message_with_no_run_started_is_unanswered() {
        let bearing = vec![
            rec(1, started("a")),
            rec(2, finished("a")),
            rec(3, user("hi again")),
        ];
        assert_eq!(
            reduce_disposition(&bearing),
            Ok(RunDisposition::Unanswered {
                user_seq: 3,
                attempts: 0
            })
        );
        let stamped = vec![rec(1, user("hi")), rec(2, attempted(1, 1))];
        assert_eq!(
            reduce_disposition(&stamped),
            Ok(RunDisposition::Unanswered {
                user_seq: 1,
                attempts: 1
            })
        );
        // Answered by an assistant row (simple engine / fast path) → Clean.
        let answered = vec![rec(1, user("hi")), rec(2, assistant("yo"))];
        assert_eq!(reduce_disposition(&answered), Ok(RunDisposition::Clean));
        // A harness-authored message is never "the user waiting".
        let synthetic = vec![
            rec(1, started("a")),
            rec(2, finished("a")),
            rec(
                3,
                SessionEvent::synthetic_user(TurnId::new_v4(), "nudge".into()),
            ),
        ];
        assert_eq!(reduce_disposition(&synthetic), Ok(RunDisposition::Clean));
        // Still rejects a raw log: a tool dispatch bears on no disposition.
        assert_eq!(
            reduce_disposition(&[rec(1, user("hi")), rec(2, requested("c1"))]),
            Err(LogContradiction::NonMarkerInMarkerSlice { seq: 2 })
        );
    }

    /// A seed a run DID pick up and then crashed is `Interrupted`, never
    /// `Unanswered`: the `RunStarted` check comes first, so the interrupted
    /// arm (boundary repair + resume) keeps the shape and the unanswered arm
    /// (no repair) never sees it. Pinned at the full-log level because every
    /// other `Interrupted` fixture omits the seed — swap the two checks in
    /// `reduce_disposition` and only these go red.
    #[test]
    fn a_seed_a_run_picked_up_is_interrupted_not_unanswered() {
        let picked_up = seq_log(vec![
            turn_started(),
            user("hi"),
            started("r1"),
            requested("c1"),
        ]);
        assert_eq!(
            reduced(&picked_up).disposition,
            RunDisposition::Interrupted { attempts: 0 }
        );
        assert_eq!(
            reduce_disposition(&[rec(1, user("hi")), rec(2, started("r1"))]),
            Ok(RunDisposition::Interrupted { attempts: 0 })
        );
        // A stamp before the seed sits in the interrupted tail and counts —
        // the same stamp would count for nothing under the unanswered
        // reading, which only counts stamps after the message.
        let stamped_then_seeded = seq_log(vec![
            started("a"),
            finished("a"),
            attempted(1, 1),
            turn_started(),
            user("hi"),
            started("b"),
        ]);
        assert_eq!(
            reduced(&stamped_then_seeded).disposition,
            RunDisposition::Interrupted { attempts: 1 }
        );
    }

    /// The unanswered ratchet counts the stamps written FOR this message —
    /// the ones after it. A stamp that sits between the last finish and the
    /// message named something else (or nothing) and must not spend one of
    /// the message's own tries.
    #[test]
    fn unanswered_attempts_count_only_the_stamps_after_the_message() {
        let stamp_before = vec![
            rec(1, finished("a")),
            rec(2, attempted(1, 1)),
            rec(3, user("hi")),
        ];
        assert_eq!(
            reduce_disposition(&stamp_before),
            Ok(RunDisposition::Unanswered {
                user_seq: 3,
                attempts: 0
            })
        );
        let stamps_after = vec![
            rec(1, finished("a")),
            rec(2, user("hi")),
            rec(3, attempted(2, 1)),
            rec(4, attempted(2, 2)),
        ];
        assert_eq!(
            reduce_disposition(&stamps_after),
            Ok(RunDisposition::Unanswered {
                user_seq: 2,
                attempts: 2
            })
        );
    }

    /// `reduce_run` hands the reducer the bearing subsequence of the whole
    /// log, so the attach face sees the same word the coordinator does.
    #[test]
    fn reduce_run_reads_the_unanswered_tail_from_a_full_log() {
        let events = seq_log(vec![
            turn_started(),
            user("hi"),
            started("r1"),
            assistant("ok"),
            finished("r1"),
            run_meta("r1"),
            turn_started(),
            user("second"),
        ]);
        assert_eq!(
            reduced(&events).disposition,
            RunDisposition::Unanswered {
                user_seq: 8,
                attempts: 0
            }
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
        assert_eq!(r.disposition, RunDisposition::Interrupted { attempts: 0 });
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

        /// The disposition-bearing subsequence, selected by the SAME predicate
        /// `reduce_disposition` accepts and `reduce_run` collects by — not a
        /// second spelling of the set.
        fn markers_of(events: &[SessionEventRecord]) -> Vec<SessionEventRecord> {
            events
                .iter()
                .filter(|r| is_disposition_bearing(&r.event))
                .cloned()
                .collect()
        }

        /// 0 = RunStarted, 1 = RunFinished, 2 = ToolCallRequested,
        /// 3 = ToolResult, 4 = AssistantMessage, 5 = ResumeAttempted,
        /// 6 = UserMessage (real, not synthetic).
        fn event_for(tag: u8, seq: EventSeq) -> SessionEvent {
            match tag % 7 {
                0 => started(&format!("r{seq}")),
                1 => finished(&format!("r{seq}")),
                2 => requested(&format!("c{seq}")),
                3 => result_for(&format!("c{seq}")),
                4 => assistant("x"),
                5 => attempted(seq, 1),
                _ => user("u"),
            }
        }

        proptest! {
            #[test]
            fn reduce_run_asks_reduce_disposition(tags in prop::collection::vec(0u8..7, 0..40)) {
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

            /// §5.6: `attempts` is exactly the number of `ResumeAttempted`
            /// stamps after the last `RunFinished` — whenever the tail is
            /// interrupted at all.
            #[test]
            fn attempts_are_the_stamps_since_the_last_finish(tags in prop::collection::vec(0u8..7, 0..40)) {
                let events: Vec<SessionEventRecord> = tags
                    .iter()
                    .enumerate()
                    .map(|(i, t)| rec(i as EventSeq + 1, event_for(*t, i as EventSeq + 1)))
                    .collect();
                let since_finish = events
                    .iter()
                    .rposition(|r| matches!(r.event, SessionEvent::RunFinished { .. }))
                    .map_or(0, |i| i + 1);
                let expected = events[since_finish..]
                    .iter()
                    .filter(|r| matches!(r.event, SessionEvent::ResumeAttempted { .. }))
                    .count() as u32;
                if let Ok(RunDisposition::Interrupted { attempts }) =
                    reduce_disposition(&markers_of(&events))
                {
                    prop_assert_eq!(attempts, expected);
                }
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

    /// Every production construction of `SessionEvent::UserMessage` — a
    /// construction supplies all five fields and therefore never contains
    /// `..`; a pattern always does. Equality, so a sixth producer is a red
    /// test.
    ///
    /// The reason: §5.2's premise is "the only writer of a `UserMessage`
    /// outside `[RunStarted, RunFinished]` on a resumable session is
    /// `seed_session`". `Unanswered` retriggers a run for such a message, so
    /// every other producer must be classified — inside a run, a child, an
    /// ephemeral side session, or the seed — before it may exist.
    #[test]
    fn user_message_producers_are_the_known_set() {
        use crate::utils::source_scan::{code_text, production_text, rust_sources_under};

        // `production_text` asks the file's ANCESTORS, not only its parent,
        // so `src/harness/tests/*` (declared from the inline
        // `#[cfg(test)] mod tests { … }` block in `harness/mod.rs`) contributes
        // nothing here. This census carried its own walk for that shape until
        // the instrument learned it; `source_scan`'s
        // `ancestor_declared_test_modules_are_recognised` now pins it.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found: std::collections::BTreeMap<String, usize> = Default::default();
        for (path, src) in rust_sources_under(&root) {
            let code = code_text(&production_text(std::path::Path::new(&path), &src));
            for body in code.split("SessionEvent::UserMessage {").skip(1) {
                let mut depth = 1usize;
                let mut end = 0usize;
                for (i, ch) in body.char_indices() {
                    match ch {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                end = i;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let fields = body.get(..end).unwrap_or("");
                if !fields.contains("..") {
                    let key = path
                        .replace('\\', "/")
                        .rsplit("src/")
                        .next()
                        .unwrap_or(&path)
                        .to_string();
                    *found.entry(key).or_default() += 1;
                }
            }
        }
        // The map is what the scan prints at THIS commit (re-derived by T8,
        // which deleted the fast-path literals: the L0 fast path now writes
        // its `UserMessage` through `SessionEvent::user_turn`, inside a
        // `[RunStarted, RunFinished]` pair — a run, so `Interrupted` and not
        // `Unanswered` is what a crash there reads as).
        let expected: std::collections::BTreeMap<String, usize> = [
            // child seed — excluded by `unanswered_eligible` (Subagent/Ephemeral keys)
            ("agents/subagent_spawner/mod.rs", 1),
            // Simulated engine: user then assistant, no markers
            ("gateway/execution_engine/simple.rs", 1),
            // steer: only into a RUNNING session
            ("gateway/execution_engine/steering.rs", 1),
            // client history replay, Ephemeral key
            ("gateway/openai_api/completions/agent.rs", 1),
            // legacy transcript backfill, followed by a run
            ("orchestrator/harness_bridge/backfill.rs", 1),
            // THE producer (prompt + multimodal; history's trailing prompt
            // now goes through `user_turn`)
            ("orchestrator/harness_bridge/session_seed.rs", 2),
            // synthetic_user (in-run, `synthetic: true` — never "the user
            // waiting") + user_turn (the seed pair's one constructor: the
            // bridge's history seed and the L0 fast path)
            ("session/events.rs", 2),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        assert!(
            found.contains_key("orchestrator/harness_bridge/session_seed.rs"),
            "the scan found no seed — blind, not clean"
        );
        assert_eq!(
            found, expected,
            "a new UserMessage producer must be classified against §5.2 \
             (inside a run / child / ephemeral / seed)"
        );
    }
}

#[cfg(test)]
mod own_scope_tests {
    use super::*;
    use crate::session::events::{MessageContent, TurnTrigger};

    fn rec(seq: EventSeq, event: SessionEvent) -> SessionEventRecord {
        SessionEventRecord {
            seq,
            event,
            created_at_ms: seq as i64 * 10,
        }
    }

    fn forked() -> SessionEvent {
        SessionEvent::SessionForked {
            parent_session_id: "main:parent".to_string(),
            at: 1,
        }
    }

    fn turn_started() -> SessionEvent {
        SessionEvent::TurnStarted {
            turn_id: TurnId::new_v4(),
            trigger: TurnTrigger::SubagentRequest,
            at: 1,
        }
    }

    fn assistant(text: &str) -> SessionEvent {
        SessionEvent::AssistantMessage {
            turn_id: TurnId::new_v4(),
            content: MessageContent {
                text: text.to_string(),
                blocks: Vec::new(),
                thinking: None,
                thinking_signature: None,
            },
            usage: None,
            at: 1,
        }
    }

    #[test]
    fn an_unforked_log_owns_all_of_itself() {
        let events = vec![rec(1, turn_started()), rec(2, assistant("mine"))];
        assert_eq!(own_work_start(&events), 0);
    }

    #[test]
    fn an_empty_log_owns_nothing_and_says_zero() {
        assert_eq!(own_work_start(&[]), 0);
    }

    /// The defect: a forked child's log opens with the parent's transcript.
    /// Own scope starts at the child's own `TurnStarted`, which the spawner
    /// emits after `fork::seed` returns.
    #[test]
    fn a_forked_log_starts_at_its_own_turn_not_at_the_seeded_prefix() {
        let events = vec![
            rec(1, forked()),
            rec(2, assistant("the parent's conclusion")),
            rec(3, turn_started()),
            rec(4, assistant("the child's finding")),
        ];
        assert_eq!(own_work_start(&events), 2);
    }

    /// `session_split` stamps a second marker into a child that already has
    /// one; only the newest bounds the newest copied prefix.
    #[test]
    fn the_last_fork_marker_wins() {
        let events = vec![
            rec(1, forked()),
            rec(2, turn_started()),
            rec(3, forked()),
            rec(4, assistant("re-seeded prefix")),
            rec(5, turn_started()),
        ];
        assert_eq!(own_work_start(&events), 4);
    }

    /// Seeded and killed before its own turn opened. The scope is EMPTY, not
    /// the whole log: answering 0 would report the parent's dispatches as the
    /// child's own in-flight calls.
    #[test]
    fn a_fork_that_never_opened_its_own_turn_owns_nothing() {
        let events = vec![rec(1, forked()), rec(2, assistant("the parent's answer"))];
        assert_eq!(own_work_start(&events), events.len());
        assert!(events[own_work_start(&events)..].is_empty());
    }
}
