//! `MessageProjector` — a [`SessionEventObserver`] that materialises session
//! events into the `messages` table via a single ordered async drain task.
//!
//! Each assistant row carries the tokens of the single LLM call that produced
//! it, read straight off `AssistantMessage.usage` — the harness emits one
//! `AssistantMessage` per Think step, so calls and rows are 1:1. The session
//! row's counters are the fold of those same events (`session::usage_fold`),
//! accumulated once per run when its `AssistantRunMeta` lands — see
//! [`fold_run_bill`] — or, for a run that finished but whose meta never
//! reached the log, when a whole-session heal synthesizes that stamp
//! ([`synthesize_missing_stamps`]).
//!
//! The observer itself is **non-blocking**: `on_appended` enqueues the event
//! onto an mpsc channel and returns immediately.
//!
//! # Nothing is dropped
//!
//! Back-pressure (`Full`) and a stopped drain (`Closed`) no longer lose the
//! row. Both record the event's `seq` in [`MessageProjector::missed`] — seqs
//! only, because the payload is already durable in `session_events`, which
//! stays the single source of truth — and the next heal pass re-reads those
//! events from the log and projects them. `Closed` additionally calls
//! [`MessageProjector::ensure_drain`], which respawns the writer.
//!
//! A heal is a **seq-set difference**, not a watermark: the transcript's own
//! row ids carry the source seq ([`parse_source_seq`]), so a hole BELOW the
//! newest row is as visible as a missing tail. That is the whole reason the
//! previous design lost rows — it back-filled only above `max(seq)` and only
//! for sessions whose run markers read as interrupted, so a gap in a session
//! that then finished cleanly was invisible and permanent.
//!
//! # The one honest boundary that remains
//!
//! `missed` is **process memory**. A crash between an event's durable append
//! and its drain leaves no in-process record of the gap, so recovery of THAT
//! gap is the next boot's job: [`crate::gateway::projection_reconciler`] asks
//! this projector to repair every session in the activity window (plus every
//! session whose markers read as interrupted), and the `core/projection-holes`
//! doctor check does the unbounded sweep for anything older than that window.
//! No durable projection watermark is written — see
//! `docs/superpowers/specs/2026-09-02-crash-recovery-r2-design.md` A6.
//!
//! The drain task is the **single writer** for a session: heals run inside it,
//! so a repair can never interleave with the live drain of the same session.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::events::GatewayEventFrame;
use crate::gateway::session_store::types::MessageRecord;
use crate::gateway::session_store::{RunBill, SessionStore, StampOutcome};
use crate::session::events::{EventSeq, SessionEvent, SessionEventRecord};
use crate::session::observer::SessionEventObserver;
use crate::session::projection::{parse_source_seq, project_row, row_id};
use crate::session::service::SessionId;
use crate::session::store::SessionEventStore;

// Internal submodule — splits off `MissedSeqs` + `RepairReport` + `FlushTimeout`
// (used by heal_session) and `RunSpan` + `collect_run_spans` +
// `synthesize_missing_stamps` (used by both heal_session and the boot
// reconciler). The drain task and `MessageProjector` itself stay here — they
// are the single writer and the only path that owns the in-memory state.
pub mod projector_sub;
// Re-export so callers reach the moved types through the canonical
// `crate::gateway::session_projector::*` path. `MissedSeqs` and `RunSpan` are
// `pub(crate)` and stay internal — no glob re-export for those.
pub use projector_sub::missed_seqs::{FlushTimeout, RepairReport};
use projector_sub::missed_seqs::MissedSeqs;
use projector_sub::run_span::{collect_run_spans, synthesize_missing_stamps};

/// Capacity of the internal mpsc channel between the observer and the drain task.
const QUEUE_CAP: usize = 4096;

/// The process-wide projector, for consumers that cannot be handed one.
///
/// Today that is the `core/projection-holes` doctor check, which is built by
/// two registries (`builtin_tools::doctor`, `gateway::handlers::diagnostics`)
/// that have neither the session store nor the projector in scope. `None`
/// makes that check report UNKNOWN rather than "no holes" — the check's own
/// arm says so.
static GLOBAL_PROJECTOR: CapabilitySlot<Arc<MessageProjector>> = CapabilitySlot::new(
    "gateway/message-projector",
    MissingSemantics::ConsumerDecides,
);

/// Install the process-wide projector. Called once at daemon boot. Idempotent.
#[inline]
pub fn set_global_message_projector(projector: Arc<MessageProjector>) {
    let _ = GLOBAL_PROJECTOR.install(projector);
}

// There is deliberately NO `decline_*` wrapper here: boot's install is
// unconditional (the projector is constructed and published in the same two
// lines), so there is no branch in which the slot is reached and skipped. A
// decline arm with no caller reads as "never reached" about a handle that is
// always installed — `capability::census`'s
// `every_decline_wrapper_has_a_production_caller` is the guard that says so.

/// The process-wide projector, or `None` when boot never installed one.
#[inline]
#[must_use]
pub fn global_message_projector() -> Option<Arc<MessageProjector>> {
    GLOBAL_PROJECTOR.get().cloned()
}

/// The handle above, type-erased for the capability roster.
pub(crate) const fn message_projector_slot() -> &'static dyn SlotStatus {
    &GLOBAL_PROJECTOR
}

/// What the drain task is asked to do. Every variant is handled by the SAME
/// task, which is what makes "one writer per session" true by construction
/// rather than by convention.
enum ProjectorMsg {
    /// An event was appended to the SSOT log; materialise it.
    ///
    /// Boxed because this variant is ~3x the next largest and the enum sizes
    /// the whole bounded channel: unboxed, every queued `Repair`/`Flush`
    /// also reserved a record's worth of buffer. The extra allocation is free
    /// next to the `record.clone()` the sender already does.
    Event(SessionId, Box<SessionEventRecord>),
    /// Fill this session's holes and re-apply its stamps, then answer.
    Repair(SessionId, oneshot::Sender<RepairReport>),
    /// Answer once every message queued before this one has been handled.
    Flush(oneshot::Sender<()>),
}

/// Materialises a session event stream into the `messages` store.
pub struct MessageProjector {
    /// Replaced wholesale by [`Self::ensure_drain`] when the drain dies, which
    /// is why it is behind a lock rather than being a plain field.
    tx: StdMutex<mpsc::Sender<ProjectorMsg>>,
    store: Arc<dyn SessionStore>,
    bus: Option<Arc<GatewayEventBus>>,
    /// A pinned SSOT log, or `None` to read the process-wide slot at use time.
    events: Option<Arc<dyn SessionEventStore>>,
    missed: Arc<StdMutex<MissedSeqs>>,
}

impl MessageProjector {
    /// Create a new projector and spawn its drain task.
    ///
    /// `bus` is what makes this drain the producer of the live peer echo
    /// ([`GatewayEventFrame::SessionUserMessage`]). `None` keeps the projector
    /// fully usable without a running gateway (tests, tools that open the store
    /// directly) — it then only materialises rows.
    pub fn new(store: Arc<dyn SessionStore>, bus: Option<Arc<GatewayEventBus>>) -> Arc<Self> {
        Self::with_event_store(store, bus, None)
    }

    /// As [`Self::new`], with the SSOT log pinned rather than resolved from the
    /// process-wide slot at use time.
    ///
    /// Production takes the slot: boot constructs the projector BEFORE it opens
    /// `session_events`, so a handle captured here would always be `None`.
    /// Callers that own a log — the boot reconciler's tests, anything driving a
    /// second store in one process — pin it, because the slot installs once per
    /// process and a shared log would let one caller's heal read another's
    /// events.
    pub fn with_event_store(
        store: Arc<dyn SessionStore>,
        bus: Option<Arc<GatewayEventBus>>,
        events: Option<Arc<dyn SessionEventStore>>,
    ) -> Arc<Self> {
        let (tx, rx) = mpsc::channel::<ProjectorMsg>(QUEUE_CAP);
        let missed: Arc<StdMutex<MissedSeqs>> = Arc::default();
        spawn_drain(
            store.clone(),
            bus.clone(),
            missed.clone(),
            events.clone(),
            rx,
        );
        Arc::new(Self {
            tx: StdMutex::new(tx),
            store,
            bus,
            events,
            missed,
        })
    }

    /// The projection this writes into — the doctor check reads the transcript
    /// through it rather than being handed a second handle to the same store.
    #[must_use]
    pub fn projection_store(&self) -> Arc<dyn SessionStore> {
        self.store.clone()
    }

    /// Respawn the drain if it has stopped.
    ///
    /// The receiver moved into the dead task, so the channel cannot be reused:
    /// a restart is a NEW channel plus a new sender. Anything still queued on
    /// the old one is gone — which is exactly why the seqs are recorded in
    /// `missed` at enqueue-failure time and why a heal reads the SSOT log
    /// rather than a queue.
    ///
    /// Cheap and idempotent: the common case is one `is_closed()` load.
    pub fn ensure_drain(&self) {
        let mut tx = self
            .tx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !tx.is_closed() {
            return;
        }
        let (new_tx, rx) = mpsc::channel::<ProjectorMsg>(QUEUE_CAP);
        spawn_drain(
            self.store.clone(),
            self.bus.clone(),
            self.missed.clone(),
            self.events.clone(),
            rx,
        );
        *tx = new_tx;
        tracing::warn!("projector drain restarted; missed seqs will be healed");
    }

    /// A clone of the current sender, taken without holding the lock across an
    /// await.
    fn sender(&self) -> mpsc::Sender<ProjectorMsg> {
        self.tx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Fill this session's holes and re-apply its stamps.
    ///
    /// Runs INSIDE the drain task, so it cannot race the live projection of the
    /// same session. An undeliverable request answers `errored` — "I could not
    /// find out", never "there was nothing to do".
    pub async fn request_repair(&self, id: &SessionId) -> RepairReport {
        self.ensure_drain();
        let (reply_tx, reply_rx) = oneshot::channel();
        if self
            .sender()
            .send(ProjectorMsg::Repair(id.clone(), reply_tx))
            .await
            .is_err()
        {
            return RepairReport {
                errored: true,
                ..RepairReport::default()
            };
        }
        reply_rx.await.unwrap_or(RepairReport {
            errored: true,
            ..RepairReport::default()
        })
    }

    /// Return once every event enqueued before this call has been projected.
    ///
    /// The shutdown barrier: without it the process can drop the store while
    /// the drain still holds rows, which is a projection gap manufactured by
    /// the orderly path rather than by a crash.
    pub async fn flush(&self, timeout: Duration) -> Result<(), FlushTimeout> {
        self.ensure_drain();
        let (reply_tx, reply_rx) = oneshot::channel();
        if self
            .sender()
            .send(ProjectorMsg::Flush(reply_tx))
            .await
            .is_err()
        {
            return Err(FlushTimeout);
        }
        match tokio::time::timeout(timeout, reply_rx).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) | Err(_) => Err(FlushTimeout),
        }
    }

    /// Simulate a drain that stopped: point the sender at a channel whose
    /// receiver is already gone, which both closes the live channel (dropping
    /// the last sender ends the running task) and makes the next `try_send`
    /// report `Closed`.
    #[cfg(test)]
    fn kill_drain(&self) {
        let (dead_tx, dead_rx) = mpsc::channel::<ProjectorMsg>(1);
        drop(dead_rx);
        let mut tx = self
            .tx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *tx = dead_tx;
    }

    /// Seqs this projector knows are absent from `id`'s transcript.
    #[cfg(test)]
    fn missed_seqs(&self, id: &SessionId) -> BTreeSet<EventSeq> {
        self.missed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .seqs
            .get(id)
            .cloned()
            .unwrap_or_default()
    }
}

/// Spawn the single ordered writer.
fn spawn_drain(
    store: Arc<dyn SessionStore>,
    bus: Option<Arc<GatewayEventBus>>,
    missed: Arc<StdMutex<MissedSeqs>>,
    pinned_events: Option<Arc<dyn SessionEventStore>>,
    mut rx: mpsc::Receiver<ProjectorMsg>,
) {
    tokio::spawn(async move {
        // Per-session seq of the `RunStarted` that opened the run in flight.
        // Lives in the task rather than on the struct because the task is the
        // only reader and the only writer — see the module doc's "single
        // writer" note.
        let mut run_start: HashMap<SessionId, EventSeq> = HashMap::new();
        while let Some(msg) = rx.recv().await {
            match msg {
                ProjectorMsg::Event(id, rec) => {
                    if matches!(rec.event, SessionEvent::RunStarted { .. }) {
                        run_start.insert(id.clone(), rec.seq);
                    }
                    let events = resolve_events(&pinned_events);
                    let never = |_: EventSeq| false;
                    let ctx = ProjectionCtx {
                        store: &store,
                        events: events.as_ref(),
                        present: &never,
                        run_start: run_start.get(&id).copied().unwrap_or(0),
                        bus: bus.as_ref(),
                    };
                    if matches!(project_event(&id, &rec, &ctx).await, Projected::Retry) {
                        lock_missed(&missed).record(&id, rec.seq);
                    }
                    // Bound to a local on purpose: a guard held in the `if`
                    // condition would still be alive inside the block, and
                    // `heal_session` takes the same lock.
                    let dirty = lock_missed(&missed).is_dirty(&id);
                    if dirty {
                        let _ = heal_session(
                            &store,
                            &id,
                            &missed,
                            &pinned_events,
                            &mut run_start,
                            HealScope::KnownGaps,
                        )
                        .await;
                    }
                }
                ProjectorMsg::Repair(id, reply) => {
                    let report = heal_session(
                        &store,
                        &id,
                        &missed,
                        &pinned_events,
                        &mut run_start,
                        HealScope::WholeSession,
                    )
                    .await;
                    let _ = reply.send(report);
                }
                ProjectorMsg::Flush(reply) => {
                    let _ = reply.send(());
                }
            }
        }
    });
}

fn lock_missed(missed: &Arc<StdMutex<MissedSeqs>>) -> std::sync::MutexGuard<'_, MissedSeqs> {
    missed
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The pinned log if there is one, else the process-wide slot. Read at use time
/// rather than at construction because boot builds the projector before it
/// opens `session_events`.
fn resolve_events(
    pinned: &Option<Arc<dyn SessionEventStore>>,
) -> Option<Arc<dyn SessionEventStore>> {
    pinned
        .clone()
        .or_else(crate::session::store::global_session_event_store)
}

/// How far back a heal pass reads the log before answering "up to date".
///
/// The two callers ask different questions, and the floor is the difference: a
/// drain-triggered pass exists BECAUSE this process recorded misses, while a
/// requested repair is asking about gaps this process never saw.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum HealScope {
    /// Drain-triggered. The recorded misses are the gaps, so the lowest of them
    /// is the floor — the live path must not re-read a session's whole log on
    /// every back-pressure event.
    KnownGaps,
    /// Explicitly requested: the boot [`crate::gateway::projection_reconciler`]
    /// and the `core/projection-holes` doctor check. Sweeps from the first
    /// event, because an in-process floor answers the wrong question here — the
    /// holes such a caller is asking about were left by ANOTHER process, and
    /// `missed` holding one recent seq would start the pass above every one of
    /// them and report "filled 0" for a session the caller measured as holed.
    ///
    /// Also the only scope that synthesizes the stamp for a finished run whose
    /// `AssistantRunMeta` never landed ([`synthesize_missing_stamps`]). Not
    /// because it sees more of the log — both scopes read it to the head, and
    /// the one meta neither can see is the one not yet APPENDED, the live
    /// window between a run's `RunFinished` and its meta — but because of
    /// exposure and floor: a [`KnownGaps`](Self::KnownGaps) pass runs
    /// routinely on live sessions, inside that window as a matter of course,
    /// and from a floor that can cut a run in half; this scope runs at boot
    /// before any run is live, or on a rare explicit request, and reads every
    /// span from its opener. Stamping inside the window bills the run twice:
    /// the real meta carries a different id from the synthesized stamp and
    /// overwrites it (`synthesize_missing_stamps`'s race note).
    WholeSession,
}

/// Re-project everything this session's transcript is missing.
///
/// The predicate is a **set**, not a watermark: `present(seq)` answers from the
/// transcript's own row ids, so a gap at seq 10 with 11 and 12 written is
/// filled. How far down the log the pass starts is [`HealScope`]'s job, not the
/// missed set's: a requested repair always starts at 1.
async fn heal_session(
    store: &Arc<dyn SessionStore>,
    id: &SessionId,
    missed: &Arc<StdMutex<MissedSeqs>>,
    pinned_events: &Option<Arc<dyn SessionEventStore>>,
    run_start: &mut HashMap<SessionId, EventSeq>,
    scope: HealScope,
) -> RepairReport {
    let mut report = RepairReport::default();
    let key = id.to_key_string();

    let transcript = match store.get_history(id, None).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(session = ?id, error = %e, "projector heal: get_history failed");
            report.errored = true;
            return report;
        }
    };
    let seqs: HashSet<EventSeq> = transcript
        .iter()
        .filter_map(|m| parse_source_seq(&m.id, &key))
        .collect();
    if !transcript.is_empty() && seqs.is_empty() {
        report.legacy = true;
        return report;
    }

    let claimed = lock_missed(missed).take(id);
    let from = match scope {
        HealScope::WholeSession => 1,
        HealScope::KnownGaps => claimed.iter().next().copied().unwrap_or(1),
    };

    let Some(event_store) = resolve_events(pinned_events) else {
        // No SSOT log installed: this pass cannot tell a whole session from a
        // holed one. Put the claim back and say so.
        lock_missed(missed).restore(id, claimed);
        report.errored = true;
        return report;
    };

    let events = match event_store.load_events_range(id, Some(from), None).await {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(session = ?id, error = %e, "projector heal: load_events_range failed");
            lock_missed(missed).restore(id, claimed);
            report.errored = true;
            return report;
        }
    };

    let present = |s: EventSeq| seqs.contains(&s);
    let mut retry: BTreeSet<EventSeq> = BTreeSet::new();
    for rec in &events {
        if matches!(rec.event, SessionEvent::RunStarted { .. }) {
            run_start.insert(id.clone(), rec.seq);
        }
        let ctx = ProjectionCtx {
            store,
            events: Some(&event_store),
            present: &present,
            run_start: run_start.get(id).copied().unwrap_or(0),
            // A heal replays messages typed in a previous process (or before
            // the gap was noticed). Announcing them as live would replay a
            // finished conversation into every Panel that is open.
            bus: None,
        };
        match project_event(id, rec, &ctx).await {
            Projected::Row => report.holes_filled += 1,
            Projected::Stamped { bill } => {
                report.stamps_reapplied += 1;
                if bill == BillOutcome::Billed {
                    report.usage_rebilled += 1;
                }
            }
            Projected::Nothing => {}
            Projected::Retry => {
                retry.insert(rec.seq);
            }
        }
    }
    let deferred = retry.len();
    lock_missed(missed).restore(id, retry);
    // After the walk, so every row a finished run produced is in the
    // transcript before its stamp is looked for. Whole-session only — not
    // because this pass sees more of the log (both scopes read it to the
    // head, and the one meta neither can see is the one not yet appended)
    // but because of where each runs: a drain-triggered pass runs on live
    // sessions, routinely inside the window between a `RunFinished` and its
    // meta, and from a floor that can start mid-run; this scope runs at boot
    // before any run is live, or on a rare explicit request, and its spans
    // start at their openers (`HealScope::WholeSession`).
    //
    // The spans are folded over the live rows PLUS the retired openers and
    // metas: a rewind that cut a finished run's meta away leaves that run
    // reading finished-without-meta on the live rows alone, and it was billed
    // when the meta landed. An unreadable answer to "what was retired?" is not
    // "nothing was retired" — synthesis is refused for this pass (`errored`,
    // so the report does not claim the session is whole) rather than read as
    // permission to bill again (criterion #8).
    if scope == HealScope::WholeSession {
        match event_store.load_retired_run_anchors(id).await {
            Ok(retired) => {
                synthesize_missing_stamps(
                    store,
                    id,
                    &event_store,
                    &present,
                    &collect_run_spans(&events, &retired),
                    &mut report,
                )
                .await;
            }
            Err(e) => {
                tracing::warn!(
                    session = ?id,
                    error = %e,
                    "projector heal: retired run anchors unreadable; no stamp synthesized"
                );
                report.errored = true;
            }
        }
    }
    // `retry` is part of this answer. Every failure inside the loop — an append
    // that would not write, a stamp that would not land, a retirement flag that
    // would not read, a usage fold that would not read — comes back as
    // `Projected::Retry` and sets none of the counters, so without this clause
    // a heal that wrote nothing BECAUSE it could not returns `up_to_date: true`,
    // and the reconciler counts the session as whole in the boot line.
    //
    // Deliberately not folded into `errored`: a `NoRowInRange` deferral is
    // benign (a run that produced no assistant row at all), and reporting it as
    // a failure would mark such sessions permanently broken. "Not up to date"
    // is the honest middle — the pass did not find out.
    report.up_to_date = report.holes_filled == 0
        && report.stamps_reapplied == 0
        && report.stamps_synthesized == 0
        && !report.errored
        && deferred == 0;
    report
}
/// Pure and separate from the write so the decision is unit-testable without a
/// store, a bus, or a runtime — every condition below is a way this has to be
/// able to say "no", and each one is load-bearing:
///
/// - **Live drain only.** The heal path passes no bus at all (see
///   [`heal_session`]), so this function never sees it; the `live` flag keeps
///   the rule stated where the decision is, rather than only where the caller
///   happens to be.
/// - **Real user messages only.** `synthetic` user events are the prompt
///   builder's `<system-reminder>` scaffolding, not something a human typed.
/// - **Attributed only.** See the frame's own doc: an author-less message
///   cannot be told apart from the viewer's own, so there is nobody it can be
///   safely rendered to. Outside a project room `ambient_room_author` is
///   `None`, which is why single-author deployments never emit this at all.
/// - **Non-empty only.** `hydrate_session_history` skips blank rows, so
///   emitting one would put up a bubble that the next reload takes away —
///   the precise failure this whole frame exists to avoid.
fn peer_echo_frame(
    session_key: &str,
    seq: u64,
    event: &SessionEvent,
    author_user_id: Option<&str>,
    record: &MessageRecord,
    live: bool,
) -> Option<GatewayEventFrame> {
    if !live {
        return None;
    }
    let SessionEvent::UserMessage {
        synthetic: false, ..
    } = event
    else {
        return None;
    };
    let author = author_user_id.filter(|a| !a.is_empty())?;
    if record.content.trim().is_empty() {
        return None;
    }
    Some(GatewayEventFrame::SessionUserMessage {
        session_key: session_key.to_string(),
        author_user_id: author.to_string(),
        content: record.content.clone(),
        // The record's own accessor, not a hand-rolled format of
        // `created_at_ms` — see the field's doc on the frame.
        timestamp: record.rfc3339(),
        seq,
    })
}

/// Was this event retired after it was enqueued?
///
/// The drain is asynchronous, so a queued event can be retired *before* it
/// reaches `messages`, and writing it then would silently un-clear the
/// conversation the user just cleared.
///
/// ⚠️ `retired_at` has two writers with **opposite** intent, and this gate reads
/// only the flag. `Retire::From` (`chat.clear` / `chat.rewind` /
/// `session.truncate`) means "erase" — suppressing the row is the whole point.
/// `Retire::Through` (manual `/compact`, `context::compact::manual`, the
/// `retire` argument of its one `append_batch`) means "stop replaying, keep
/// everything" — for it the suppression is collateral: a compacted event that
/// had not yet drained loses its Panel row. Distinguishing the two would take a
/// retirement *reason* on the row; that is a schema change with no observed
/// failure behind it. If one ever shows up, this is the place.
///
/// `Ok(false)` when there is no event log installed (CLI one-shot, unit tests):
/// nothing can have been retired. An `Err` is neither "retired" nor "live" —
/// the caller turns it into [`Projected::Retry`], which keeps the seq and comes
/// back for it, instead of the old fail-closed `true` that dropped the row and
/// called it a decision.
async fn event_retired(
    events: Option<&Arc<dyn SessionEventStore>>,
    id: &SessionId,
    seq: EventSeq,
) -> Result<bool, crate::session::service::SessionError> {
    match events {
        Some(store) => store.is_retired(id, seq).await,
        None => Ok(false),
    }
}

/// Everything one projection step needs besides the record itself.
///
/// A struct rather than a widening parameter list because `present` and
/// `run_start` are two halves of the same question — "where does this record
/// belong in a transcript that may already hold part of it" — and the drain and
/// the heal answer both differently.
pub(crate) struct ProjectionCtx<'a> {
    /// The projection target.
    pub store: &'a Arc<dyn SessionStore>,
    /// The SSOT log, for the retirement re-check. `None` = no log installed.
    pub events: Option<&'a Arc<dyn SessionEventStore>>,
    /// True when this seq's row is already in the transcript. The live drain
    /// passes `|_| false` (it sees each seq once); a heal passes the
    /// transcript's seq SET, which is what makes a hole below the newest row
    /// visible.
    pub present: &'a (dyn Fn(EventSeq) -> bool + Send + Sync),
    /// Seq of the `RunStarted` that opened this record's run, or 0 when the
    /// replay window began after it. Bounds the row an `AssistantRunMeta`
    /// stamp may land on.
    pub run_start: EventSeq,
    /// Live peer echo sink. Absent on the heal path by construction.
    pub bus: Option<&'a Arc<GatewayEventBus>>,
}

/// What happened to a run's spend when its stamp landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BillOutcome {
    /// The fold found spend, and it was accumulated with the stamp.
    Billed,
    /// Nothing to add: no usage on the run's messages and no price.
    NothingToBill,
    /// Nothing to fold from — no event log installed, or no `RunStarted`
    /// before the meta (a `/compact` retired the opener, or a legacy log).
    /// The row is stamped and the spend is NOT accumulated, said at `warn!`
    /// rather than read as "billed nothing" (criterion #8).
    Unfoldable,
}

/// What one projection step did — the drain's and the heal's shared vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Projected {
    /// A transcript row was appended.
    Row,
    /// An `AssistantRunMeta` stamp landed on a row that had none, together
    /// with whatever the run owed (`bill`).
    Stamped { bill: BillOutcome },
    /// Nothing to do: not row-producing, already present, already stamped,
    /// deliberately retired, or a stamp with no row in its run's range.
    Nothing,
    /// This seq must be tried again — its retirement state is unknown, a write
    /// failed, a usage fold could not be read, or the row a stamp needs is
    /// not in the projection yet.
    Retry,
}

/// Project one session event into `ctx.store` — the single source of projection
/// truth shared by the live drain and by [`heal_session`].
///
/// A row-producing event whose seq has been RETIRED is suppressed, so a
/// clear/rewind that races the drain queue cannot re-materialise.
///
/// `ctx.bus`, when present, publishes the live peer echo for a newly-materialised
/// user row (see [`peer_echo_frame`]). It is published from HERE rather than
/// from the run engines because this is the one point every producer of a user
/// message passes through — `harness_bridge::session_seed` (the main path),
/// `fast_path` (which re-emits the event by hand for exactly this reason),
/// `SimpleExecutionEngine`, and mid-run `steering` — and because it is the only
/// point where the text being announced is, by construction, the text
/// `chat.history` will replay.
pub(crate) async fn project_event(
    id: &SessionId,
    rec: &SessionEventRecord,
    ctx: &ProjectionCtx<'_>,
) -> Projected {
    let key = id.to_key_string();
    if (ctx.present)(rec.seq) {
        return Projected::Nothing;
    }
    match event_retired(ctx.events, id, rec.seq).await {
        Ok(true) => return Projected::Nothing,
        Ok(false) => {}
        Err(e) => {
            tracing::warn!(
                session = ?id,
                seq = rec.seq,
                error = %e,
                "projector: retirement check failed; keeping the seq for the next heal"
            );
            return Projected::Retry;
        }
    }
    match &rec.event {
        SessionEvent::AssistantMessage { content, usage, .. } => {
            // The tokens of the one call that produced this message. The
            // cross-event accumulator this replaced (`LlmCallStarted` /
            // `LlmCallEnded` folded per turn_id) was a correct design for an
            // event pair no production code has ever emitted, so it summed
            // nothing and wrote 0 onto every assistant row since the projector
            // was written. Both events are gone; the number now rides on the
            // message that spent it.
            let usage = usage.clone().unwrap_or_default();
            if let Err(e) = ctx
                .store
                .append_message(
                    id,
                    MessageRecord {
                        id: row_id(&key, rec.seq),
                        role: "assistant".into(),
                        content: content.text.clone(),
                        timestamp: rec.created_at_ms,
                        metadata: None,
                        input_tokens: i64::from(usage.input),
                        output_tokens: i64::from(usage.output),
                        tool_call_id: None,
                        tool_name: None,
                    },
                )
                .await
            {
                tracing::warn!(error = %e, "projector assistant append failed");
                return Projected::Retry;
            }
            Projected::Row
        }
        SessionEvent::AssistantRunMeta {
            run_id,
            context_tokens,
            context_window,
            total_tokens,
            cost_usd,
            model,
            model_provider,
            ..
        } => {
            // The gauge is stamped only when the run resolved one. A meta
            // whose three gauge fields are `None` stamps the run_id alone;
            // the Panel reads the missing keys as absent, never as 0.
            let occupancy = match (context_tokens, context_window, total_tokens) {
                (Some(context_tokens), Some(context_window), Some(total_tokens)) => Some(
                    crate::gateway::execution_engine::helpers::RunContextOccupancy {
                        context_tokens: *context_tokens,
                        context_window: *context_window,
                        total_tokens: *total_tokens,
                        cost_usd: *cost_usd,
                        model: model.clone(),
                        model_provider: model_provider.clone(),
                    },
                ),
                _ => None,
            };
            let Some(meta) =
                crate::gateway::agent_instance::build_message_metadata(Some(run_id), occupancy)
            else {
                return Projected::Nothing;
            };
            // Fold BEFORE the stamp (F10): the stamp is the bill's idempotence
            // guard, so a stamp that landed without its bill could never be
            // billed again. A failed read writes nothing and retries.
            let Some((bill, if_stamped, anchor)) = fold_run_bill(
                id,
                rec.seq,
                ctx,
                run_id,
                *cost_usd,
                model.as_deref(),
                model_provider.as_deref(),
            )
            .await
            .plan(ctx.run_start) else {
                return Projected::Retry;
            };
            // The row this run's numbers belong to is the last assistant row
            // BETWEEN the run's own `RunStarted` and this meta — not "the last
            // assistant row in the table", which on a session with two runs
            // (or with a later row already back-filled) is somebody else's.
            // The lower bound is `anchor`, not `ctx.run_start` (I2): they
            // agree when `run_start` is this run's own opener, but
            // `run_start` can go stale-low (0, after a drain respawn) while
            // the fold still anchors correctly — using `run_start` there
            // would let the stamp reach back into the PRIOR run's row.
            match ctx
                .store
                .stamp_and_bill_in_range(id, anchor, rec.seq, &meta, bill.as_ref())
                .await
            {
                Ok(StampOutcome::AlreadyStamped) => Projected::Nothing,
                Ok(StampOutcome::NoRowInRange) => {
                    // No assistant row in this run's range: the run produced
                    // none (a hook stopped it before its first Think), or the
                    // row was dropped by the drain and is still a hole. Both
                    // finalise here. The hole case needs no retry of its own:
                    // a heal re-reads from the lowest missed seq to the end
                    // of the log, and a meta always sits above its row, so the
                    // pass that fills the row reaches this meta and stamps it
                    // (`a_meta_whose_row_was_dropped_finalises_and_the_heal_bills_it`).
                    // Retrying instead kept the seq in `missed` forever for a
                    // run with no row — every later event on the session
                    // re-ran a heal, and `up_to_date` never came true. Not
                    // billed here: the stamp is the bill's idempotence guard
                    // and it has not landed — the pass that lands it bills.
                    tracing::debug!(
                        session = ?id,
                        seq = rec.seq,
                        run_id = %run_id,
                        "projector: run-meta has no assistant row in range; nothing to stamp"
                    );
                    Projected::Nothing
                }
                Ok(StampOutcome::Stamped) => {
                    // The spend landed with the stamp (or there was none):
                    // one store operation, so a replay reads `AlreadyStamped`
                    // and never reaches this arm again.
                    if if_stamped == BillOutcome::Unfoldable {
                        tracing::warn!(
                            session = ?id,
                            run_id = %run_id,
                            "projector: run meta stamped, but its spend cannot be folded \
                             (no event log, or no RunStarted before it); not accumulated"
                        );
                    }
                    Projected::Stamped { bill: if_stamped }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "projector: stamp-and-bill of run-meta failed; nothing landed, retried");
                    Projected::Retry
                }
            }
        }
        other => {
            let Some(row) = project_row(other) else {
                return Projected::Nothing;
            };
            let record = MessageRecord {
                id: row_id(&key, rec.seq),
                role: row.role,
                content: row.text,
                timestamp: rec.created_at_ms,
                // String-valued, matching every other key in this
                // bag (`agent_instance::build_message_metadata`);
                // the history handler reads them all with `as_str`.
                metadata: row
                    .author_user_id
                    .as_ref()
                    .map(|u| serde_json::json!({ "author_user_id": u })),
                input_tokens: 0,
                output_tokens: 0,
                tool_call_id: row.tool_call_id,
                tool_name: row.tool_name,
            };
            // Decided before the move, published only after the write
            // succeeds: this frame's contract is "the transcript gained
            // this row", so announcing a failed append would put a bubble
            // on screen that no reload can reproduce.
            let echo = peer_echo_frame(
                &key,
                rec.seq,
                other,
                row.author_user_id.as_deref(),
                &record,
                ctx.bus.is_some(),
            );
            if let Err(e) = ctx.store.append_message(id, record).await {
                tracing::warn!(error = %e, "projector append failed");
                return Projected::Retry;
            }
            if let (Some(bus), Some(frame)) = (ctx.bus, echo) {
                let _ = bus.publish_frame(&frame);
            }
            Projected::Row
        }
    }
}

/// What the usage fold says a run owes, read BEFORE its stamp is written.
/// `Owe` and `Nothing` carry the seq the fold anchored on — the last
/// `RunStarted` in the slice, from [`crate::session::usage_fold::run_usage_totals`]
/// — so a caller that stamps by range can bound it from the SAME anchor the
/// fold used rather than re-deriving one (I2, criterion #12): a stale-low
/// `ctx.run_start` (for example 0, after a drain respawn) must not let the
/// stamp's lower bound reach further back than the fold's did, or it can pick
/// up the PRIOR run's row instead of this run's.
enum FoldedBill {
    Owe(RunBill, EventSeq),
    Nothing(EventSeq),
    /// No anchor at all — no event log, or no `RunStarted` in the slice (a
    /// retired opener, or a legacy log). [`FoldedBill::plan`] falls back to
    /// the caller's own lower bound for this variant only.
    Unfoldable,
    /// The slice could not be read. Nothing may be written: the stamp is the
    /// bill's idempotence guard, so stamping now would forfeit the bill.
    ReadFailed,
}

impl FoldedBill {
    /// The bill to hand the store, the outcome to report if the stamp lands,
    /// and the seq to stamp FROM. `None` = the fold failed and the caller
    /// writes nothing. `fallback` is the caller's own lower bound
    /// (`ctx.run_start` or a span's `start`), used only for `Unfoldable`,
    /// which has no anchor to report.
    fn plan(self, fallback: EventSeq) -> Option<(Option<RunBill>, BillOutcome, EventSeq)> {
        match self {
            Self::Owe(bill, anchor) => Some((Some(bill), BillOutcome::Billed, anchor)),
            Self::Nothing(anchor) => Some((None, BillOutcome::NothingToBill, anchor)),
            Self::Unfoldable => Some((None, BillOutcome::Unfoldable, fallback)),
            Self::ReadFailed => None,
        }
    }
}

/// Fold the run's spend from the log slice `[ctx.run_start, meta_seq)`,
/// anchored on the last `RunStarted` in it
/// ([`crate::session::usage_fold::run_usage_totals`]). The cost and model are
/// the meta's own — `None` from [`synthesize_missing_stamps`], which has no
/// meta and passes the run's `RunFinished` seq as `meta_seq`. Read BEFORE the
/// stamp (F10): a read that fails leaves nothing half-written. The slice is
/// NOT fixed once the meta exists — a `/compact` can retire the opener later,
/// which is the `Unfoldable` case below; it is harmless because the replay by
/// then reads `AlreadyStamped`.
///
/// `ctx.run_start == 0` ⇒ the slice starts at the log head and the fold
/// anchors on the last `RunStarted` it finds — the restarted-drain case,
/// where this process never saw the marker go by. That is also the shape I2
/// fixes: `ctx.run_start` may be stale-low, but the anchor this function
/// returns is never stale — it is POSITIONAL, the last live `RunStarted`
/// before `meta_seq`, not "this run's own opener" by construction. That is
/// this run's own opener on a log written after `execute()` began holding
/// the run slot through the meta append; on the historical
/// meta-after-next-opener shape (`run_span.rs`'s `collect_run_spans` doc),
/// it is the NEXT run's opener.
async fn fold_run_bill(
    id: &SessionId,
    meta_seq: EventSeq,
    ctx: &ProjectionCtx<'_>,
    run_id: &str,
    cost_usd: Option<f64>,
    model: Option<&str>,
    provider: Option<&str>,
) -> FoldedBill {
    let Some(events) = ctx.events else {
        return FoldedBill::Unfoldable;
    };
    let slice = match events
        .load_events_range(id, Some(ctx.run_start), Some(meta_seq))
        .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(session = ?id, run_id, error = %e, "projector: usage fold read failed; nothing written, retried");
            return FoldedBill::ReadFailed;
        }
    };
    let Some((anchor, totals)) = crate::session::usage_fold::run_usage_totals(&slice) else {
        return FoldedBill::Unfoldable;
    };
    if totals.without_usage > 0 {
        tracing::debug!(
            session = ?id,
            run_id,
            with_usage = totals.with_usage,
            without_usage = totals.without_usage,
            "projector: run bill is a floor; some assistant messages carried no usage"
        );
    }
    if totals.input == 0 && totals.output == 0 && cost_usd.is_none() {
        return FoldedBill::Nothing(anchor);
    }
    FoldedBill::Owe(
        RunBill {
            input_tokens: i64::try_from(totals.input).unwrap_or(i64::MAX),
            output_tokens: i64::try_from(totals.output).unwrap_or(i64::MAX),
            cost_usd: cost_usd.unwrap_or(0.0),
            model: model.map(str::to_string),
            model_provider: provider.map(str::to_string),
        },
        anchor,
    )
}

impl SessionEventObserver for MessageProjector {
    fn on_appended(&self, id: &SessionId, record: &SessionEventRecord) {
        // Busy-lane wake edge, taken here rather than inside the drain task
        // below: a full queue defers the event to a heal pass, and a deferred
        // wake would put a backpressure-deferred steer back on the 30 s
        // fallback tick — the exact staleness this edge exists to remove.
        //
        // This observer is the gateway's one "an event was appended" seam, so
        // it sees every producer of an assistant turn (harness run, fast path,
        // simple engine). The predicate for which events matter belongs to
        // steering, next to the count it resets; everything below the
        // `matches!` costs nothing for the events that are not assistant turns.
        crate::gateway::execution_engine::wake_lane_if_burst_drained(
            &id.to_key_string(),
            &record.event,
        );
        let msg = ProjectorMsg::Event(id.clone(), Box::new(record.clone()));
        let sent = self
            .tx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .try_send(msg);
        match sent {
            Ok(()) => {}
            // Back-pressure. The event stays in the SSOT log and its seq is
            // remembered here, so the next heal on this session re-reads it
            // from the log and writes the row. Nothing is lost while this
            // process lives; a crash before the heal leaves it to the next
            // boot's activity-window repair.
            // One `warn!` per deferral is the observable — a counter with no
            // reader was cut on 2026-09-03 (the real-machine burst stage counts
            // these lines instead).
            Err(mpsc::error::TrySendError::Full(_)) => {
                lock_missed(&self.missed).record(id, record.seq);
                tracing::warn!(
                    session = ?id,
                    seq = record.seq,
                    "projector queue full; seq deferred to the heal pass"
                );
            }
            // The drain task has stopped/panicked — a real incident, not routine
            // back-pressure. Record the seq, then respawn the writer.
            Err(mpsc::error::TrySendError::Closed(_)) => {
                lock_missed(&self.missed).record(id, record.seq);
                tracing::error!(
                    session = ?id,
                    seq = record.seq,
                    "projector drain task stopped; seq deferred and drain restarting"
                );
                self.ensure_drain();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
    use crate::gateway::session_projector::projector_sub::run_span::{collect_run_spans, RunSpan};
    use crate::orchestrator::dispatch::TokenBreakdown;
    use crate::session::events::{Durability, MessageContent, Retire, ToolOutput, TurnId};
    use crate::session::service::SessionError;
    use crate::session::store::{RetiredAnchorKind, RetiredRunAnchor};
    use tempfile::tempdir;

    /// See `session::store::tests::the_accessor_exposes_this_handle_to_the_roster`
    /// for why this asserts through the accessor rather than the static.
    #[test]
    fn the_accessor_exposes_this_handle_to_the_roster() {
        let slot = message_projector_slot();
        assert_eq!(slot.id(), "gateway/message-projector");
        assert!(matches!(slot.missing(), MissingSemantics::ConsumerDecides));
    }

    fn rec(seq: EventSeq, ev: SessionEvent) -> SessionEventRecord {
        SessionEventRecord {
            seq,
            event: ev,
            created_at_ms: 0,
        }
    }

    fn msg_content(text: &str) -> MessageContent {
        MessageContent {
            text: text.into(),
            blocks: vec![],
            thinking: None,
            thinking_signature: None,
        }
    }

    /// The drain's context: nothing already present, no run open, no log.
    fn live_ctx<'a>(
        store: &'a Arc<dyn SessionStore>,
        never: &'a (dyn Fn(EventSeq) -> bool + Send + Sync),
        bus: Option<&'a Arc<GatewayEventBus>>,
    ) -> ProjectionCtx<'a> {
        ProjectionCtx {
            store,
            events: None,
            present: never,
            run_start: 0,
            bus,
        }
    }

    /// A user event as a room member's message: attributed, real, non-empty.
    fn room_msg(author: Option<&str>, synthetic: bool) -> SessionEvent {
        SessionEvent::UserMessage {
            turn_id: uuid::Uuid::new_v4(),
            content: msg_content("where did we land on the migration?"),
            at: 0,
            synthetic,
            author_user_id: author.map(String::from),
        }
    }

    /// The row `project_event` is about to append for [`room_msg`].
    fn row_for(text: &str) -> MessageRecord {
        MessageRecord {
            id: "agent:main:main:7".into(),
            role: "user".into(),
            content: text.into(),
            timestamp: 1_762_000_000_000,
            metadata: None,
            input_tokens: 0,
            output_tokens: 0,
            tool_call_id: None,
            tool_name: None,
        }
    }

    /// The happy path: another member's message, live, becomes a frame whose
    /// text and timestamp are the row's — not the request's, not a re-format.
    #[test]
    fn peer_echo_announces_an_attributed_room_message() {
        let row = row_for("where did we land on the migration?");
        let frame = peer_echo_frame(
            "agent:main:main",
            7,
            &room_msg(Some("u-alice"), false),
            Some("u-alice"),
            &row,
            true,
        )
        .expect("an attributed live user row must be announced");
        let GatewayEventFrame::SessionUserMessage {
            session_key,
            author_user_id,
            content,
            timestamp,
            seq,
        } = frame
        else {
            panic!("wrong frame variant");
        };
        assert_eq!(session_key, "agent:main:main");
        assert_eq!(author_user_id, "u-alice");
        assert_eq!(content, row.content);
        assert_eq!(seq, 7);
        // The record's accessor, so a live bubble and its reloaded twin sort
        // identically. Formatting `created_at_ms` by hand here would diverge on
        // whichever backend stores seconds.
        assert_eq!(timestamp, row.rfc3339());
    }

    /// A heal re-projects rows a dead process never flushed. Those messages are
    /// old; announcing them would replay a finished conversation into every
    /// Panel open at boot.
    #[test]
    fn peer_echo_is_silent_on_the_heal_path() {
        assert!(peer_echo_frame(
            "agent:main:main",
            7,
            &room_msg(Some("u-alice"), false),
            Some("u-alice"),
            &row_for("hi"),
            false,
        )
        .is_none());
    }

    /// No author ⇒ nobody can tell this from their own message, so there is no
    /// viewer it can be safely rendered to. This is every single-author session.
    #[test]
    fn peer_echo_is_silent_without_an_author() {
        assert!(
            peer_echo_frame(
                "agent:main:main",
                7,
                &room_msg(None, false),
                None,
                &row_for("hi"),
                true,
            )
            .is_none(),
            "an unattributed message must not be echoed"
        );
        assert!(
            peer_echo_frame(
                "agent:main:main",
                7,
                &room_msg(Some(""), false),
                Some(""),
                &row_for("hi"),
                true,
            )
            .is_none(),
            "an empty author id is an absent one, not a user named \"\""
        );
    }

    /// `synthetic` user events are the prompt builder's `<system-reminder>`
    /// scaffolding wearing a user role — nobody typed them.
    #[test]
    fn peer_echo_is_silent_for_synthetic_scaffolding() {
        assert!(peer_echo_frame(
            "agent:main:main",
            7,
            &room_msg(Some("u-alice"), true),
            Some("u-alice"),
            &row_for("hi"),
            true,
        )
        .is_none());
    }

    /// `hydrate_session_history` skips blank rows, so echoing one would show a
    /// bubble the next reload silently removes — the exact contradiction this
    /// frame exists to avoid.
    #[test]
    fn peer_echo_is_silent_for_a_row_the_panel_would_not_render() {
        assert!(peer_echo_frame(
            "agent:main:main",
            7,
            &room_msg(Some("u-alice"), false),
            Some("u-alice"),
            &row_for("   \n "),
            true,
        )
        .is_none());
    }

    fn user_msg(tid: TurnId) -> SessionEvent {
        SessionEvent::UserMessage {
            turn_id: tid,
            content: msg_content("hi"),
            at: 0,
            synthetic: false,
            author_user_id: None,
        }
    }

    fn assistant_msg(tid: TurnId) -> SessionEvent {
        SessionEvent::AssistantMessage {
            turn_id: tid,
            content: msg_content("hello"),
            usage: None,
            at: 0,
        }
    }

    fn assistant_msg_billed(tid: TurnId, input: u32, output: u32) -> SessionEvent {
        SessionEvent::AssistantMessage {
            turn_id: tid,
            content: msg_content("hello"),
            usage: Some(TokenBreakdown {
                input,
                output,
                ..Default::default()
            }),
            at: 0,
        }
    }

    /// The post-run stamp: no token counters — those are folded from the run's
    /// `AssistantMessage.usage` in the log the test pins.
    fn run_meta(tid: TurnId, run: &str) -> SessionEvent {
        SessionEvent::AssistantRunMeta {
            turn_id: tid,
            run_id: run.into(),
            context_tokens: Some(1234),
            context_window: Some(200_000),
            total_tokens: Some(70),
            cost_usd: Some(0.12),
            model: Some("claude".into()),
            model_provider: Some("anthropic".into()),
            at: 3,
        }
    }

    /// The stamp of a run that resolved no gauge and priced nothing — a
    /// usage-less provider, or a run with no Think at all.
    fn run_meta_without_gauge(tid: TurnId, run: &str) -> SessionEvent {
        SessionEvent::AssistantRunMeta {
            turn_id: tid,
            run_id: run.into(),
            context_tokens: None,
            context_window: None,
            total_tokens: None,
            cost_usd: None,
            model: None,
            model_provider: None,
            at: 3,
        }
    }

    fn run_started(run: &str) -> SessionEvent {
        SessionEvent::RunStarted {
            run_id: run.into(),
            at: 1,
            project_root: None,
            envelope: None,
        }
    }

    fn run_finished(run: &str) -> SessionEvent {
        SessionEvent::RunFinished {
            run_id: run.into(),
            outcome: crate::session::events::RunOutcome::Completed,
            at: 2,
        }
    }

    /// An event log this test owns, seeded with `events` — not the process-wide
    /// slot, which installs once per process and would let one test's fold read
    /// another's messages. The projector bills from THIS log, so a test that
    /// wants a bill must put the run's priced messages here.
    async fn own_event_log(
        id: &SessionId,
        events: &[(EventSeq, SessionEvent)],
    ) -> Arc<dyn SessionEventStore> {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::session::store::migrate_add_session_events(&conn).unwrap();
        let log: Arc<dyn SessionEventStore> =
            Arc::new(crate::session::store::SqliteEventStore::new(conn));
        for (seq, ev) in events {
            log.append(id, *seq, ev, 0).await.unwrap();
        }
        log
    }

    /// The marker id a log written BEFORE 2026-09-24 carries on its
    /// RunStarted / RunFinished: the bridge minted its own, never the engine
    /// id the meta carries (ruling A5, reversed by F1). Fixtures that pin
    /// the legacy shape derive the marker id here; the same-id shape every
    /// newer log carries has its own twins below (U1: both shapes stay
    /// readable).
    fn marker_of(run: &str) -> String {
        format!("marker-{run}")
    }

    /// One run, one priced message, its meta after `RunFinished` — the shape
    /// every billing test below reads. `(45, 25)` is what the fold must find.
    /// The markers carry [`marker_of`]`(run)`, the meta carries `run` — the
    /// legacy shape.
    fn one_billed_run(tid: TurnId, run: &str) -> Vec<(EventSeq, SessionEvent)> {
        vec![
            (1, run_started(&marker_of(run))),
            (2, assistant_msg_billed(tid, 45, 25)),
            (3, run_finished(&marker_of(run))),
            (4, run_meta(tid, run)),
        ]
    }

    fn tool_req(tid: TurnId) -> SessionEvent {
        SessionEvent::ToolCallRequested {
            turn_id: tid,
            call_id: "c1".into(),
            name: "bash_exec".into(),
            input: serde_json::json!({"cmd": "ls"}),
            at: 0,
        }
    }

    fn tool_res(tid: TurnId) -> SessionEvent {
        SessionEvent::ToolResult {
            turn_id: tid,
            call_id: "c1".into(),
            output: ToolOutput {
                value: serde_json::json!("ok"),
                metadata: Default::default(),
            },
            at: 0,
        }
    }

    fn sqlite_store(dir: &std::path::Path, name: &str) -> Arc<dyn SessionStore> {
        let config = SessionManagerConfig {
            db_path: dir.join(name),
            max_messages: 10_000,
            compaction_keep: 5_000,
            ..Default::default()
        };
        Arc::new(SessionManager::new(config).unwrap())
    }

    /// An event log whose retirement answer is an error — the third answer the
    /// projector used to spend as "retired" and drop the row for.
    struct UnreadableRetirement;

    #[async_trait::async_trait]
    impl SessionEventStore for UnreadableRetirement {
        async fn append_batch(
            &self,
            _id: &SessionId,
            _first_seq: EventSeq,
            _events: &[(SessionEvent, i64)],
            _retire: Option<Retire>,
            _durability: Durability,
        ) -> Result<(), SessionError> {
            Ok(())
        }
        async fn load_all_events(
            &self,
            _id: &SessionId,
        ) -> Result<Vec<SessionEventRecord>, SessionError> {
            Ok(Vec::new())
        }
        /// One projectable event, so a heal driven by this store has something
        /// to try — and therefore something to DEFER when `is_retired` below
        /// refuses to answer. The sibling test that calls `project_event`
        /// directly never reaches this method.
        async fn load_events_range(
            &self,
            _id: &SessionId,
            _from: Option<EventSeq>,
            _to: Option<EventSeq>,
        ) -> Result<Vec<SessionEventRecord>, SessionError> {
            Ok(vec![rec(1, user_msg(uuid::Uuid::new_v4()))])
        }
        async fn load_head_seq(&self, _id: &SessionId) -> Result<EventSeq, SessionError> {
            Ok(1)
        }
        async fn retire_from(
            &self,
            _id: &SessionId,
            _from_seq: EventSeq,
        ) -> Result<usize, SessionError> {
            Ok(0)
        }
        async fn is_retired(&self, _id: &SessionId, _seq: EventSeq) -> Result<bool, SessionError> {
            Err(SessionError::Storage("event log unreadable".into()))
        }
        async fn load_run_markers(
            &self,
        ) -> Result<Vec<(SessionId, crate::session::store::MarkerSlice)>, SessionError> {
            Ok(Vec::new())
        }
    }

    /// An event log whose retirement answer is "live" but whose range read
    /// fails — the fold's read, not the retirement check, is what breaks.
    struct UnreadableRange;

    #[async_trait::async_trait]
    impl SessionEventStore for UnreadableRange {
        async fn append_batch(
            &self,
            _id: &SessionId,
            _first_seq: EventSeq,
            _events: &[(SessionEvent, i64)],
            _retire: Option<Retire>,
            _durability: Durability,
        ) -> Result<(), SessionError> {
            Ok(())
        }
        async fn load_all_events(
            &self,
            _id: &SessionId,
        ) -> Result<Vec<SessionEventRecord>, SessionError> {
            Ok(Vec::new())
        }
        async fn load_events_range(
            &self,
            _id: &SessionId,
            _from: Option<EventSeq>,
            _to: Option<EventSeq>,
        ) -> Result<Vec<SessionEventRecord>, SessionError> {
            Err(SessionError::Storage("event log unreadable".into()))
        }
        async fn load_head_seq(&self, _id: &SessionId) -> Result<EventSeq, SessionError> {
            Ok(1)
        }
        async fn retire_from(
            &self,
            _id: &SessionId,
            _from_seq: EventSeq,
        ) -> Result<usize, SessionError> {
            Ok(0)
        }
        async fn is_retired(&self, _id: &SessionId, _seq: EventSeq) -> Result<bool, SessionError> {
            Ok(false)
        }
        async fn load_run_markers(
            &self,
        ) -> Result<Vec<(SessionId, crate::session::store::MarkerSlice)>, SessionError> {
            Ok(Vec::new())
        }
    }

    /// F10: the fold is read BEFORE the stamp. A read that fails writes
    /// nothing — the seq is retried — instead of landing a stamp whose bill
    /// can then never be retried.
    #[tokio::test]
    async fn an_unreadable_fold_is_retried_and_stamps_nothing() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "unreadable_fold.db");
        let id = SessionId::ephemeral("unreadable-fold");
        store.get_or_create(&id).await.unwrap();
        append_assistant_row(&store, &id, 2).await;
        let events: Arc<dyn SessionEventStore> = Arc::new(UnreadableRange);
        let never = |_: EventSeq| false;
        let ctx = ProjectionCtx {
            store: &store,
            events: Some(&events),
            present: &never,
            run_start: 1,
            bus: None,
        };
        let tid = uuid::Uuid::new_v4();
        assert_eq!(
            project_event(&id, &rec(4, run_meta(tid, "run_a")), &ctx).await,
            Projected::Retry
        );
        let row = store
            .get_history(&id, None)
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.role == "assistant")
            .expect("the row is there");
        assert!(
            row.metadata.is_none(),
            "nothing stamped: {:?}",
            row.metadata
        );
    }

    /// F10 through the projector: the store refuses the bill, the meta is
    /// retried with its row unstamped, and the retry bills it once.
    #[tokio::test]
    async fn a_refused_bill_is_retried_and_the_retry_bills_once() {
        let temp = tempdir().unwrap();
        let manager = Arc::new(
            SessionManager::new(SessionManagerConfig {
                db_path: temp.path().join("refused_bill.db"),
                max_messages: 10_000,
                compaction_keep: 5_000,
                ..Default::default()
            })
            .unwrap(),
        );
        let store: Arc<dyn SessionStore> = manager.clone();
        let id = SessionId::ephemeral("refused-bill");
        store.get_or_create(&id).await.unwrap();
        append_assistant_row(&store, &id, 2).await;
        let tid = uuid::Uuid::new_v4();
        let log = own_event_log(&id, &one_billed_run(tid, "run_a")).await;
        let never = |_: EventSeq| false;
        let ctx = ProjectionCtx {
            store: &store,
            events: Some(&log),
            present: &never,
            run_start: 1,
            bus: None,
        };
        manager
            .conn
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER fail_bill BEFORE UPDATE OF input_tokens ON sessions \
                 BEGIN SELECT RAISE(ABORT, 'injected bill failure'); END;",
            )
            .unwrap();
        let meta_rec = rec(4, run_meta(tid, "run_a"));
        assert_eq!(project_event(&id, &meta_rec, &ctx).await, Projected::Retry);

        manager
            .conn
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER fail_bill;")
            .unwrap();
        assert_eq!(
            project_event(&id, &meta_rec, &ctx).await,
            Projected::Stamped {
                bill: BillOutcome::Billed
            }
        );
        let meta = store.get_metadata(&id).await.unwrap().unwrap();
        assert_eq!((meta.input_tokens, meta.output_tokens), (45, 25));
    }

    /// Review Focus 4: a meta whose slice holds no `RunStarted` at all — a
    /// legacy log, the same shape a retired opener would leave — stamps its
    /// row and bills nothing, and the heal SAYS so: one stamp re-applied,
    /// zero rebilled, tokens untouched.
    #[tokio::test]
    async fn a_heal_counts_an_unfoldable_stamp_as_not_rebilled() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "unfoldable_heal.db");
        let id = SessionId::ephemeral("unfoldable-heal");
        store.get_or_create(&id).await.unwrap();
        let tid = uuid::Uuid::new_v4();
        let log = own_event_log(
            &id,
            &[
                (2, assistant_msg_billed(tid, 45, 25)),
                (4, run_meta(tid, "run_a")),
            ],
        )
        .await;
        let pinned: Option<Arc<dyn SessionEventStore>> = Some(log);
        let missed = Arc::new(StdMutex::new(MissedSeqs::default()));
        let mut run_start = HashMap::new();
        let report = heal_session(
            &store,
            &id,
            &missed,
            &pinned,
            &mut run_start,
            HealScope::WholeSession,
        )
        .await;
        assert_eq!(
            (
                report.holes_filled,
                report.stamps_reapplied,
                report.usage_rebilled
            ),
            (1, 1, 0),
            "{report:?}"
        );
        let meta = store.get_metadata(&id).await.unwrap().unwrap();
        assert_eq!((meta.input_tokens, meta.output_tokens), (0, 0));
    }

    /// I2 regression: the stamp's lower bound must be the SAME anchor the
    /// fold used, not `ctx.run_start` — which can go stale-low (0, after a
    /// drain respawn) while the fold still anchors correctly on this run's
    /// own `RunStarted`. Before the fix, a stale-low `run_start` let the
    /// stamp's range reach back past this run's own opener and pick up the
    /// PRIOR run's row instead — overwriting its stamp and billing the wrong
    /// run there.
    #[tokio::test]
    async fn a_stale_low_run_start_cannot_steal_the_prior_runs_row() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "stale_run_start.db");
        let id = SessionId::ephemeral("stale-run-start");
        store.get_or_create(&id).await.unwrap();

        let tid_a = uuid::Uuid::new_v4();
        let tid_b = uuid::Uuid::new_v4();
        let events = vec![
            (1, run_started("run_a")),
            (2, assistant_msg_billed(tid_a, 10, 5)),
            (3, run_finished("run_a")),
            (4, run_meta(tid_a, "run_a")),
            (5, run_started("run_b")),
            // B's row is deliberately never projected — the hole this
            // regression needs: the event is in the log (the fold can see
            // it) but no row for it exists in `messages`.
            (6, assistant_msg_billed(tid_b, 45, 25)),
            (7, run_finished("run_b")),
            (8, run_meta(tid_b, "run_b")),
        ];
        let log = own_event_log(&id, &events).await;
        let never = |_: EventSeq| false;

        // Run A: project its row, then its meta, with its own correct
        // `run_start`. This is the row a stale-low `run_start` must not
        // reach.
        let ctx_a = ProjectionCtx {
            store: &store,
            events: Some(&log),
            present: &never,
            run_start: 1,
            bus: None,
        };
        project_event(&id, &rec(2, assistant_msg_billed(tid_a, 10, 5)), &ctx_a).await;
        assert_eq!(
            project_event(&id, &rec(4, run_meta(tid_a, "run_a")), &ctx_a).await,
            Projected::Stamped {
                bill: BillOutcome::Billed
            }
        );

        // Run B: its row (seq 6) is never projected. Its meta is projected
        // with `run_start: 0` — the stale-low value a drain respawn leaves
        // behind (`ensure_drain`'s fresh, empty map).
        let ctx_b = ProjectionCtx {
            store: &store,
            events: Some(&log),
            present: &never,
            run_start: 0,
            bus: None,
        };
        assert_eq!(
            project_event(&id, &rec(8, run_meta(tid_b, "run_b")), &ctx_b).await,
            Projected::Nothing,
            "B's row is a hole, not A's — the anchor must exclude A's row \
             even though `run_start` is stale-low"
        );

        let row_a = store
            .get_history(&id, None)
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.role == "assistant")
            .expect("A's row is there");
        assert_eq!(
            row_a
                .metadata
                .as_ref()
                .and_then(|m| m.get("run_id"))
                .and_then(|v| v.as_str()),
            Some("run_a"),
            "A's row must still carry A's own stamp, not B's"
        );
        let meta = store.get_metadata(&id).await.unwrap().unwrap();
        assert_eq!(
            (meta.input_tokens, meta.output_tokens),
            (10, 5),
            "B must not have been billed onto A's row"
        );
    }

    /// An unreadable retirement flag is neither "retired" nor "live". The old
    /// code read it as "retired", dropped the row, and returned — which on a
    /// session that then finished cleanly lost the row for good.
    #[tokio::test]
    async fn an_unknown_retirement_answer_is_retried_not_dropped() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "retire_unknown.db");
        let id = SessionId::ephemeral("retire-unknown");
        store.get_or_create(&id).await.unwrap();

        let events: Arc<dyn SessionEventStore> = Arc::new(UnreadableRetirement);
        let never = |_: EventSeq| false;
        let ctx = ProjectionCtx {
            store: &store,
            events: Some(&events),
            present: &never,
            run_start: 0,
            bus: None,
        };
        let outcome = project_event(&id, &rec(1, user_msg(uuid::Uuid::new_v4())), &ctx).await;
        assert_eq!(
            outcome,
            Projected::Retry,
            "an unreadable retirement flag must keep the seq, not spend it as a decision"
        );
        assert!(
            store.get_history(&id, None).await.unwrap().is_empty(),
            "and it must not write the row on a guess either"
        );
    }

    /// A heal that deferred a row did not find the session whole — it failed to
    /// find out. `up_to_date` used to be computed from the three counters
    /// alone, and every deferral sets none of them, so a pass that wrote
    /// nothing BECAUSE it could not reported the same value as a pass that had
    /// nothing to do. `ProjectionReconciler` then counts the session under
    /// `skipped_up_to_date` and the boot line says it was whole.
    ///
    /// `errored` stays false on purpose: nothing was unreadable at the store
    /// level, one row was deferred. Folding the two together would report a run
    /// that produced no assistant row as a permanent failure.
    #[tokio::test]
    async fn a_heal_that_deferred_a_row_is_not_up_to_date() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "heal_deferred.db");
        let id = SessionId::ephemeral("heal-deferred");
        store.get_or_create(&id).await.unwrap();

        let events: Option<Arc<dyn SessionEventStore>> = Some(Arc::new(UnreadableRetirement));
        let missed = Arc::new(StdMutex::new(MissedSeqs::default()));
        let mut run_start = HashMap::new();
        let report = heal_session(
            &store,
            &id,
            &missed,
            &events,
            &mut run_start,
            HealScope::WholeSession,
        )
        .await;

        assert_eq!(report.holes_filled, 0, "the row could not be written");
        assert_eq!(report.stamps_reapplied, 0);
        assert!(
            !report.errored,
            "a deferral is not a read failure; the store answered every call it could"
        );
        assert!(
            !report.up_to_date,
            "a pass that deferred a row must not report the session as whole"
        );
    }

    /// The busy lane's burst-drain wake edge has to survive the seam it is
    /// fired from. Asserting the call would prove nothing — throw the notify
    /// away and a call-count guard stays green — so this asserts the effect:
    /// a waiter parked on the lane is released by an assistant turn arriving at
    /// the observer, and is NOT released by a user turn (which does not drain
    /// anything).
    #[tokio::test]
    async fn an_appended_assistant_turn_wakes_a_backpressure_deferred_waiter() {
        let temp = tempdir().unwrap();
        let config = SessionManagerConfig {
            db_path: temp.path().join("proj_wake.db"),
            ..Default::default()
        };
        let manager = SessionManager::new(config).unwrap();
        let id = SessionId::ephemeral("proj_wake");
        manager.get_or_create(&id).await.unwrap();
        let store: Arc<dyn SessionStore> = Arc::new(manager);
        let projector = MessageProjector::new(store, None);

        // The lane is keyed exactly as the observer will render this session.
        let key = id.to_key_string();
        let ticket = crate::gateway::busy_queue::register(&key, 8, "proj-wake-run")
            .expect("lane accepts the deferred steer");
        crate::gateway::busy_queue::mark_awaiting_burst_drain(&key, "proj-wake-run");
        let wake = ticket.wake_handle();
        let parked = wake.notified();
        tokio::pin!(parked);

        let tid = uuid::Uuid::new_v4();
        projector.on_appended(&id, &rec(1, user_msg(tid)));
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut parked)
                .await
                .is_err(),
            "a user turn adds to the burst; it cannot drain it"
        );

        projector.on_appended(&id, &rec(2, assistant_msg(tid)));
        tokio::time::timeout(Duration::from_millis(500), &mut parked)
            .await
            .expect("an assistant turn at the observer must reach the lane");
    }

    /// `flush` is the shutdown barrier: it must not answer until the events
    /// queued before it have become rows. Asserting "the call returned" would
    /// pass with an empty body, so this asserts the ROWS are there at the
    /// moment it returns — no polling.
    #[tokio::test]
    async fn flush_returns_only_after_the_queue_has_drained() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "flush.db");
        let id = SessionId::ephemeral("flush");
        store.get_or_create(&id).await.unwrap();
        let projector = MessageProjector::new(store.clone(), None);

        let tid = uuid::Uuid::new_v4();
        for seq in 1..=20u64 {
            projector.on_appended(&id, &rec(seq, user_msg(tid)));
        }
        projector
            .flush(Duration::from_secs(5))
            .await
            .expect("the drain must settle");
        assert_eq!(
            store.get_history(&id, None).await.unwrap().len(),
            20,
            "flush answered while rows were still queued"
        );
    }

    #[tokio::test]
    async fn projector_stamps_run_meta_on_assistant_row() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "proj_meta.db");
        let id = SessionId::ephemeral("proj_meta");
        store.get_or_create(&id).await.unwrap();

        let tid = uuid::Uuid::new_v4();
        let events = one_billed_run(tid, "run_xyz");
        let log = own_event_log(&id, &events).await;
        let projector = MessageProjector::with_event_store(store.clone(), None, Some(log));
        for (seq, ev) in &events {
            projector.on_appended(&id, &rec(*seq, ev.clone()));
        }
        projector.flush(Duration::from_secs(5)).await.unwrap();

        let msgs = store.get_history(&id, None).await.unwrap();
        let asst = msgs
            .into_iter()
            .find(|m| m.role == "assistant")
            .expect("assistant row must exist");
        let meta = asst.metadata.as_ref().expect("row must carry the stamp");
        assert_eq!(
            meta.get("run_id").and_then(|v| v.as_str()),
            Some("run_xyz"),
            "run_id mismatch"
        );
        // build_message_metadata stores occupancy values as strings.
        assert_eq!(
            meta.get("context_tokens").and_then(|v| v.as_str()),
            Some("1234"),
            "context_tokens mismatch"
        );
        assert_eq!(
            meta.get("context_window").and_then(|v| v.as_str()),
            Some("200000"),
            "context_window mismatch"
        );
    }

    /// The stamp is bounded by the run it reports on. Two runs in one session:
    /// the second run's meta must land on the SECOND assistant row, not on
    /// "the last assistant row in the table" (which is the same thing here only
    /// by accident) — and, crucially, the FIRST run's meta replayed afterwards
    /// must not steal the second row.
    #[tokio::test]
    async fn a_run_meta_stamps_the_row_inside_its_own_run() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "range.db");
        let id = SessionId::ephemeral("range");
        store.get_or_create(&id).await.unwrap();

        let tid = uuid::Uuid::new_v4();
        for (seq, role, text) in [(2u64, "assistant", "first"), (5, "assistant", "second")] {
            store
                .append_message(
                    &id,
                    MessageRecord {
                        id: row_id(&id.to_key_string(), seq),
                        role: role.into(),
                        content: text.into(),
                        timestamp: seq as i64,
                        metadata: None,
                        input_tokens: 0,
                        output_tokens: 0,
                        tool_call_id: None,
                        tool_name: None,
                    },
                )
                .await
                .unwrap();
        }

        // Run A opened at seq 1, its meta at seq 3 → row seq 2. No log is
        // pinned: this test is about WHERE the stamp lands; without a log
        // there is nothing to fold, which the outcome names (Unfoldable),
        // rather than reading as "billed nothing".
        let never = |_: EventSeq| false;
        let ctx_a = ProjectionCtx {
            store: &store,
            events: None,
            present: &never,
            run_start: 1,
            bus: None,
        };
        assert_eq!(
            project_event(&id, &rec(3, run_meta(tid, "run_a")), &ctx_a).await,
            Projected::Stamped {
                bill: BillOutcome::Unfoldable
            }
        );

        let rows = store.get_history(&id, None).await.unwrap();
        let first = rows.iter().find(|m| m.content == "first").unwrap();
        let second = rows.iter().find(|m| m.content == "second").unwrap();
        assert_eq!(
            first
                .metadata
                .as_ref()
                .and_then(|m| m.get("run_id"))
                .and_then(|v| v.as_str()),
            Some("run_a"),
            "run A's meta must stamp the row inside run A"
        );
        assert!(
            second.metadata.is_none(),
            "a row from a later run must not be stamped by an earlier run's meta"
        );
    }

    /// The dropped-row race, end to end. The drain dropped the assistant row
    /// (`missed` holds its seq) and then reached the meta: the meta finalises
    /// (`Nothing`) and bills nothing — the fold WOULD find (45, 25), the
    /// unlanded stamp is what stops it. The heal that fills the hole re-reads
    /// from the lowest missed seq to the end of the log, so it reaches the
    /// meta above the row, stamps it and bills — which is why the meta needs
    /// no retry of its own, and why finalising it is safe.
    #[tokio::test]
    async fn a_meta_whose_row_was_dropped_finalises_and_the_heal_bills_it() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "norow.db");
        let id = SessionId::ephemeral("norow");
        store.get_or_create(&id).await.unwrap();

        let tid = uuid::Uuid::new_v4();
        let events = one_billed_run(tid, "run_a");
        let log = own_event_log(&id, &events).await;
        let never = |_: EventSeq| false;
        let ctx = ProjectionCtx {
            store: &store,
            events: Some(&log),
            present: &never,
            run_start: 1,
            bus: None,
        };
        let out = project_event(&id, &rec(4, run_meta(tid, "run_a")), &ctx).await;
        assert_eq!(
            out,
            Projected::Nothing,
            "no row in range finalises the meta"
        );
        let meta = store.get_metadata(&id).await.unwrap().unwrap();
        assert_eq!(
            (meta.input_tokens, meta.output_tokens),
            (0, 0),
            "a stamp with no row must not bill the session"
        );

        // The row's seq is the hole the drain recorded; the heal starts there.
        let missed = Arc::new(StdMutex::new(MissedSeqs::default()));
        lock_missed(&missed).record(&id, 2);
        let pinned: Option<Arc<dyn SessionEventStore>> = Some(log.clone());
        let mut run_start = HashMap::from([(id.clone(), 1)]);
        let report = heal_session(
            &store,
            &id,
            &missed,
            &pinned,
            &mut run_start,
            HealScope::KnownGaps,
        )
        .await;
        assert_eq!(
            (
                report.holes_filled,
                report.stamps_reapplied,
                report.usage_rebilled
            ),
            (1, 1, 1),
            "the heal fills the row, then reaches the meta above it: {report:?}"
        );
        assert!(!report.errored, "{report:?}");
        assert!(
            !lock_missed(&missed).is_dirty(&id),
            "the hole is filled and the meta was not deferred: nothing stays in `missed`"
        );
        let meta = store.get_metadata(&id).await.unwrap().unwrap();
        assert_eq!(
            (meta.input_tokens, meta.output_tokens),
            (45, 25),
            "the heal that landed the stamp billed the run"
        );
    }

    /// T9's real defect, fixed at the executor: a run that produced no
    /// assistant row at all (a hook stopped it before its first Think) still
    /// emits a meta, and that meta has nowhere to land. It must finalise —
    /// not sit in `missed` re-running a heal on every later event with
    /// `up_to_date` never coming true.
    #[tokio::test]
    async fn a_meta_for_a_run_with_no_assistant_row_finalises() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "no_think.db");
        let id = SessionId::ephemeral("no-think");
        store.get_or_create(&id).await.unwrap();

        let tid = uuid::Uuid::new_v4();
        let events = vec![
            (1, run_started(&marker_of("run_a"))),
            (2, run_finished(&marker_of("run_a"))),
            (3, run_meta_without_gauge(tid, "run_a")),
        ];
        let log = own_event_log(&id, &events).await;
        let never = |_: EventSeq| false;
        let ctx = ProjectionCtx {
            store: &store,
            events: Some(&log),
            present: &never,
            run_start: 1,
            bus: None,
        };
        assert_eq!(
            project_event(&id, &rec(3, run_meta_without_gauge(tid, "run_a")), &ctx).await,
            Projected::Nothing
        );

        let missed = Arc::new(StdMutex::new(MissedSeqs::default()));
        let pinned: Option<Arc<dyn SessionEventStore>> = Some(log.clone());
        let mut run_start = HashMap::new();
        let report = heal_session(
            &store,
            &id,
            &missed,
            &pinned,
            &mut run_start,
            HealScope::WholeSession,
        )
        .await;
        assert!(
            report.up_to_date,
            "a run with no row is whole, not permanently deferred: {report:?}"
        );
        assert!(
            !lock_missed(&missed).is_dirty(&id),
            "nothing may stay in `missed`"
        );
        assert!(store.get_history(&id, None).await.unwrap().is_empty());
    }

    /// A gauge-less meta stamps the run_id alone: the gauge keys are absent
    /// from the row's metadata, so the Panel reads "unknown", never 0.
    #[tokio::test]
    async fn a_meta_without_a_gauge_stamps_the_run_id_alone() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "no_gauge.db");
        let id = SessionId::ephemeral("no-gauge");
        store.get_or_create(&id).await.unwrap();
        append_assistant_row(&store, &id, 2).await;

        let tid = uuid::Uuid::new_v4();
        let events = vec![
            (1, run_started(&marker_of("run_a"))),
            (2, assistant_msg(tid)),
            (3, run_finished(&marker_of("run_a"))),
            (4, run_meta_without_gauge(tid, "run_a")),
        ];
        let log = own_event_log(&id, &events).await;
        let never = |_: EventSeq| false;
        let ctx = ProjectionCtx {
            store: &store,
            events: Some(&log),
            present: &never,
            run_start: 1,
            bus: None,
        };
        assert_eq!(
            project_event(&id, &rec(4, run_meta_without_gauge(tid, "run_a")), &ctx).await,
            Projected::Stamped {
                bill: BillOutcome::NothingToBill
            },
            "no usage, no price: stamped, nothing to bill"
        );
        let row = store
            .get_history(&id, None)
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.role == "assistant")
            .expect("the row is there");
        let meta = row.metadata.as_ref().expect("the run_id stamp landed");
        assert_eq!(meta.get("run_id").and_then(|v| v.as_str()), Some("run_a"));
        for key in ["context_tokens", "context_window", "total_tokens"] {
            assert!(
                meta.get(key).is_none(),
                "{key} must be absent, not 0: {meta}"
            );
        }
    }

    /// The assistant row the tests below stamp, appended straight to the
    /// projection — the log and the projection are seeded independently so a
    /// test can hold one without the other.
    async fn append_assistant_row(store: &Arc<dyn SessionStore>, id: &SessionId, seq: EventSeq) {
        store
            .append_message(
                id,
                MessageRecord {
                    id: row_id(&id.to_key_string(), seq),
                    role: "assistant".into(),
                    content: "hello".into(),
                    timestamp: seq as i64,
                    metadata: None,
                    input_tokens: 0,
                    output_tokens: 0,
                    tool_call_id: None,
                    tool_name: None,
                },
            )
            .await
            .unwrap();
    }

    /// Replay is the normal case now — a heal re-reads the whole range every
    /// time. The stamp is the idempotence guard, so the second pass must find
    /// the row already carrying this run's id and bill nothing. The bill itself
    /// is the FOLD of the run's messages in the pinned log — the meta carries
    /// no counters to bill from.
    #[tokio::test]
    async fn replaying_one_run_meta_bills_once() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "rebill.db");
        let id = SessionId::ephemeral("rebill");
        store.get_or_create(&id).await.unwrap();
        append_assistant_row(&store, &id, 2).await;

        let tid = uuid::Uuid::new_v4();
        let events = one_billed_run(tid, "run_a");
        let log = own_event_log(&id, &events).await;
        let never = |_: EventSeq| false;
        let ctx = ProjectionCtx {
            store: &store,
            events: Some(&log),
            present: &never,
            run_start: 1,
            bus: None,
        };
        let meta_rec = rec(4, run_meta(tid, "run_a"));
        assert_eq!(
            project_event(&id, &meta_rec, &ctx).await,
            Projected::Stamped {
                bill: BillOutcome::Billed
            }
        );
        assert_eq!(
            project_event(&id, &meta_rec, &ctx).await,
            Projected::Nothing,
            "the second pass must recognise its own stamp"
        );

        let meta = store.get_metadata(&id).await.unwrap().unwrap();
        assert_eq!(
            (meta.input_tokens, meta.output_tokens),
            (45, 25),
            "one run, one bill"
        );
        assert_eq!(
            meta.model.as_deref(),
            Some("claude"),
            "cost and model still ride the meta"
        );
    }

    /// The fold is anchored on the run's `RunStarted`. When the slice
    /// `[run_start, meta)` holds none — a log whose run marker was retired, or
    /// a legacy log without one — the meta still stamps the row (the gauge is
    /// the meta's own fact) but the session is NOT billed: an unanchored fold
    /// would charge everything in the window to this one run. The counters are
    /// read back from the store, not inferred from the outcome word.
    #[tokio::test]
    async fn an_unanchored_meta_stamps_but_does_not_bill() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "unanchored.db");
        let id = SessionId::ephemeral("unanchored");
        store.get_or_create(&id).await.unwrap();
        append_assistant_row(&store, &id, 2).await;

        let tid = uuid::Uuid::new_v4();
        // The priced message is in the log; the RunStarted that would anchor it
        // is not.
        let events = vec![
            (2, assistant_msg_billed(tid, 45, 25)),
            (4, run_meta(tid, "run_a")),
        ];
        let log = own_event_log(&id, &events).await;
        let never = |_: EventSeq| false;
        let ctx = ProjectionCtx {
            store: &store,
            events: Some(&log),
            present: &never,
            run_start: 0,
            bus: None,
        };
        assert_eq!(
            project_event(&id, &rec(4, run_meta(tid, "run_a")), &ctx).await,
            Projected::Stamped {
                bill: BillOutcome::Unfoldable
            }
        );

        let row = store
            .get_history(&id, None)
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.role == "assistant")
            .expect("the row is there");
        assert_eq!(
            row.metadata
                .as_ref()
                .and_then(|m| m.get("run_id"))
                .and_then(|v| v.as_str()),
            Some("run_a"),
            "the stamp lands regardless — it is the meta's own fact"
        );
        let meta = store.get_metadata(&id).await.unwrap().unwrap();
        assert_eq!(
            (meta.input_tokens, meta.output_tokens),
            (0, 0),
            "an unanchored fold must not bill the session"
        );
    }

    /// `synthesis_end` of every span in `events`, in log order — over the
    /// live rows alone (nothing retired).
    fn synthesis_ends(events: &[(EventSeq, SessionEvent)]) -> Vec<Option<EventSeq>> {
        synthesis_ends_with_retired(events, &[])
    }

    /// [`synthesis_ends`] with the retired openers / metas a rewind left
    /// behind, `(seq, kind)` in seq order.
    fn synthesis_ends_with_retired(
        events: &[(EventSeq, SessionEvent)],
        retired: &[(EventSeq, RetiredAnchorKind)],
    ) -> Vec<Option<EventSeq>> {
        let log: Vec<SessionEventRecord> = events.iter().map(|(s, e)| rec(*s, e.clone())).collect();
        let retired: Vec<RetiredRunAnchor> = retired
            .iter()
            .map(|(seq, kind)| RetiredRunAnchor {
                seq: *seq,
                kind: *kind,
            })
            .collect();
        collect_run_spans(&log, &retired)
            .iter()
            .map(RunSpan::synthesis_end)
            .collect()
    }

    /// The retired half of the positional join (final review I1), on the
    /// pure fold. A rewind that cut a finished run's meta away leaves the
    /// live rows `[RunStarted, AssistantMessage, RunFinished{Cancelled}]`;
    /// with the retired meta merged in by seq the run has its meta and is
    /// NOT synthesized. A retired opener between the live opener and the
    /// retired meta moves the anchor: that meta was a later run's, and the
    /// surviving earlier run — never billed — IS synthesized.
    #[test]
    fn a_retired_meta_marks_the_span_it_landed_in_and_a_retired_opener_moves_the_anchor() {
        use RetiredAnchorKind::{RunMeta, RunStarted};
        let tid = uuid::Uuid::new_v4();
        // Live log after `retire_from(3)` + closer: the meta at 5 is retired.
        let rewound = [
            (1, run_started("marker-a")),
            (2, assistant_msg_billed(tid, 45, 25)),
            (6, run_finished("marker-a")),
        ];
        assert_eq!(
            synthesis_ends(&rewound),
            vec![Some(6)],
            "control: over the live rows alone the run reads finished-without-meta"
        );
        assert_eq!(
            synthesis_ends_with_retired(&rewound, &[(5, RunMeta)]),
            vec![None],
            "the retired meta landed inside this span: it was billed live, nothing to synthesize"
        );
        assert_eq!(
            synthesis_ends_with_retired(&rewound, &[(3, RunStarted), (5, RunMeta)]),
            vec![Some(6)],
            "a retired opener at 3 moved the anchor: the meta at 5 was run b's, run a stays \
             finished-without-meta and is synthesized once"
        );
        // A retired opener after the live rows (a wholly retired later run)
        // cannot make the surviving run's closer close nothing.
        let earlier_billed = [
            (1, run_started("marker-a")),
            (2, assistant_msg_billed(tid, 45, 25)),
            (3, run_finished("marker-a")),
            (4, run_meta(tid, "engine-a")),
        ];
        assert_eq!(
            synthesis_ends_with_retired(&earlier_billed, &[(5, RunStarted), (8, RunMeta)]),
            vec![None],
            "run b retired whole: run a keeps its own live meta"
        );
        // A retired opener BEFORE live rows (a rewind while the run was in
        // flight cut its own opener): the live rows belong to a run the fold
        // cannot see, so no span is opened for them and nothing is synthesized.
        let opener_cut = [
            (2, assistant_msg_billed(tid, 45, 25)),
            (3, run_finished("marker-a")),
            (4, run_meta(tid, "engine-a")),
        ];
        assert_eq!(
            synthesis_ends_with_retired(&opener_cut, &[(1, RunStarted)]),
            Vec::<Option<EventSeq>>::new(),
        );
    }

    /// The shape a stamp is synthesized for, clause by clause, on the pure
    /// fold. The "has a meta" clause is pinned separately, on the production
    /// id shape, by `a_meta_marks_the_span_it_follows_even_though_its_id_is_
    /// not_the_markers` and its twin; a meta whose own stamp landed makes that
    /// clause unobservable at the store (the synthesized stamp reads
    /// `AlreadyStamped` either way), and the store-level pin of the same fact
    /// is `a_live_run_with_its_meta_is_billed_once_and_two_repairs_add_nothing`.
    #[test]
    fn a_stamp_is_synthesized_only_for_a_finished_run_with_rows_and_no_meta() {
        let tid = uuid::Uuid::new_v4();
        assert_eq!(
            synthesis_ends(&[
                (1, run_started("a")),
                (2, assistant_msg(tid)),
                (3, run_finished("a")),
            ]),
            vec![Some(3)],
            "finished, one row, no meta: stamp up to the RunFinished"
        );
        assert_eq!(
            synthesis_ends(&one_billed_run(tid, "a")),
            vec![None],
            "its meta is in the log: that meta owns the stamp, landed or deferred"
        );
        assert_eq!(
            synthesis_ends(&[(1, run_started("a")), (2, assistant_msg(tid))]),
            vec![None],
            "still open: the meta may still come"
        );
        assert_eq!(
            synthesis_ends(&[(1, run_started("a")), (2, run_finished("a"))]),
            vec![None],
            "no assistant message: no row to stamp"
        );
        assert_eq!(
            synthesis_ends(&[
                (1, run_started("a")),
                (2, assistant_msg(tid)),
                (3, run_started("b")),
                (4, assistant_msg(tid)),
                (5, run_finished("b")),
            ]),
            vec![None, Some(5)],
            "the closer closes the newest opener, so a span never holds a \
             second RunStarted — the fold's anchor is the span's own start"
        );
    }

    /// The legacy id shape, on the pure fold: in a log written before the
    /// markers carried the engine id, the meta still marks the span it
    /// follows — by position, the way the projector anchored it — or every
    /// finished run reads meta-less at boot and is stamped and billed again
    /// on every restart (the `holes` stage's 44 → 88 → 176). Reddens if the
    /// join goes back to comparing ids.
    #[test]
    fn a_meta_marks_the_span_it_follows_even_though_its_id_is_not_the_markers() {
        let tid = uuid::Uuid::new_v4();
        assert_eq!(
            synthesis_ends(&[
                (1, run_started("marker-A")),
                (2, assistant_msg_billed(tid, 45, 25)),
                (3, run_meta(tid, "engine-X")),
                (4, run_finished("marker-A")),
            ]),
            vec![None],
            "the meta names an id the markers never carry, and still marks \
             the span it landed inside: nothing to synthesize"
        );
    }

    /// The shape every log written since 2026-09-24 carries: the markers and
    /// the meta share the engine's run id. Same answer as the legacy twin
    /// above — the join is positional, so it never depended on the ids.
    #[test]
    fn a_meta_under_its_markers_own_id_marks_the_span_it_follows() {
        let tid = uuid::Uuid::new_v4();
        assert_eq!(
            synthesis_ends(&[
                (1, run_started("engine-X")),
                (2, assistant_msg_billed(tid, 45, 25)),
                (3, run_finished("engine-X")),
                (4, run_meta(tid, "engine-X")),
            ]),
            vec![None],
        );
    }

    /// The twin of the test above, same ids, meta absent: the span is
    /// finished with a row and no meta, so it IS synthesized up to its
    /// closer. Together the pair pins that the meta — and only the meta —
    /// is what turns synthesis off.
    #[test]
    fn a_finished_span_with_no_meta_is_synthesized_whatever_its_marker_id() {
        let tid = uuid::Uuid::new_v4();
        assert_eq!(
            synthesis_ends(&[
                (1, run_started("marker-A")),
                (2, assistant_msg_billed(tid, 45, 25)),
                (4, run_finished("marker-A")),
            ]),
            vec![Some(4)],
            "no meta after the opener: stamp up to the RunFinished"
        );
    }

    /// The historical shape: run a's meta landed AFTER run b's opener (the
    /// live path no longer writes this — `execute()` holds the run slot
    /// through the meta append, pinned in `execution_engine::tests` — but
    /// logs written before that fix carry it). This is also a log written
    /// before the markers carried the engine id — the `marker_of` fixture ids
    /// throughout — so the same positional join applies. The walk anchors the
    /// meta on b's opener, finds no assistant row in `(4, 5]`, finalises it
    /// `NoRowInRange` and bills nothing; the positional fold reads the same
    /// way — the meta marks span b — so span a is finished, has a row and no
    /// meta, and IS synthesized: stamped with its marker id and billed ONCE
    /// from its own messages. One bill for run a, none from its misanchored
    /// meta, and a second pass adds nothing. (What the shape still costs is
    /// the projector's, not the fold's: had run b already produced a row
    /// before seq 5, a's meta would stamp and bill THAT row under a's id —
    /// the T24 known limit; the heal does not second-guess a landed meta.)
    #[tokio::test]
    async fn a_historical_meta_that_landed_after_the_next_opener_leaves_the_older_run_synthesized()
    {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "meta_next_range.db");
        let id = SessionId::ephemeral("meta-next-range");
        store.get_or_create(&id).await.unwrap();

        let tid = uuid::Uuid::new_v4();
        let log = own_event_log(
            &id,
            &[
                (1, run_started(&marker_of("a"))),
                (2, assistant_msg_billed(tid, 45, 25)),
                (3, run_finished(&marker_of("a"))),
                (4, run_started(&marker_of("b"))),
                (5, run_meta(tid, "a")),
            ],
        )
        .await;
        let pinned: Option<Arc<dyn SessionEventStore>> = Some(log.clone());
        let missed = Arc::new(StdMutex::new(MissedSeqs::default()));
        let mut run_start = HashMap::new();

        let whole = heal_session(
            &store,
            &id,
            &missed,
            &pinned,
            &mut run_start,
            HealScope::WholeSession,
        )
        .await;
        assert_eq!(
            (
                whole.holes_filled,
                whole.stamps_reapplied,
                whole.stamps_synthesized,
                whole.usage_rebilled,
            ),
            (1, 0, 1, 1),
            "the row is filled; the meta, anchored on run b's opener, stamps \
             nothing; span a reads meta-less and is synthesized and billed: {whole:?}"
        );
        let row = store
            .get_history(&id, None)
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.role == "assistant")
            .expect("run a's row was projected");
        assert_eq!(
            row.metadata
                .as_ref()
                .and_then(|m| m.get("run_id"))
                .and_then(|v| v.as_str()),
            Some(marker_of("a").as_str()),
            "the synthesized stamp carries the marker id — the meta never \
             reached this row"
        );
        let meta = store.get_metadata(&id).await.unwrap().unwrap();
        assert_eq!(
            (meta.input_tokens, meta.output_tokens),
            (45, 25),
            "run a is billed exactly once, from its own messages"
        );

        let again = heal_session(
            &store,
            &id,
            &missed,
            &pinned,
            &mut run_start,
            HealScope::WholeSession,
        )
        .await;
        assert!(
            again.up_to_date,
            "a second pass finds the stamp and adds nothing: {again:?}"
        );
        assert_eq!(
            store.get_metadata(&id).await.unwrap().unwrap().input_tokens,
            45,
            "…and the totals are unchanged"
        );
    }

    /// The shape `qa/resume_boundary` `holes` measures, at the store: a run
    /// that finished normally on the LIVE path — in a log written before the
    /// markers carried the engine id, drained live so the meta's own stamp
    /// landed and billed — is billed exactly once, and two whole-session repairs
    /// (the boot reconciler's pass, twice, as two restarts would run it) add
    /// nothing. With the fold joining meta to span by id equality this went
    /// `(45, 25)` → `(90, 50)` → `(180, 100)`: every pass read the run as
    /// meta-less, synthesized a stamp under the marker id, billed, and the
    /// next pass found the meta's engine id "missing" from the row and
    /// re-applied that too.
    #[tokio::test]
    async fn a_live_run_with_its_meta_is_billed_once_and_two_repairs_add_nothing() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "holes_once.db");
        let id = SessionId::ephemeral("holes-once");
        store.get_or_create(&id).await.unwrap();

        let tid = uuid::Uuid::new_v4();
        let events = one_billed_run(tid, "engine-x");
        let log = own_event_log(&id, &events).await;
        let projector = MessageProjector::with_event_store(store.clone(), None, Some(log));
        for (seq, ev) in &events {
            projector.on_appended(&id, &rec(*seq, ev.clone()));
        }
        projector.flush(Duration::from_secs(5)).await.unwrap();
        let billed = |m: &crate::gateway::session_store::types::SessionMetadata| {
            (m.input_tokens, m.output_tokens)
        };
        assert_eq!(
            billed(&store.get_metadata(&id).await.unwrap().unwrap()),
            (45, 25),
            "the live drain billed the run once, from the meta"
        );

        for pass in 1..=2 {
            let repair = projector.request_repair(&id).await;
            assert_eq!(
                (
                    repair.stamps_reapplied,
                    repair.stamps_synthesized,
                    repair.usage_rebilled,
                ),
                (0, 0, 0),
                "repair pass {pass} stamped nothing and billed nothing: {repair:?}"
            );
            assert!(repair.up_to_date, "repair pass {pass}: {repair:?}");
            assert_eq!(
                billed(&store.get_metadata(&id).await.unwrap().unwrap()),
                (45, 25),
                "repair pass {pass} added no tokens"
            );
        }
        let row = store
            .get_history(&id, None)
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.role == "assistant")
            .expect("the row is there");
        assert_eq!(
            row.metadata
                .as_ref()
                .and_then(|m| m.get("run_id"))
                .and_then(|v| v.as_str()),
            Some("engine-x"),
            "the row still carries the meta's id — no synthesized stamp overwrote it"
        );
    }

    /// F1, at the store: `request_repair` runs in the one-append window between
    /// a run's `RunFinished` and its meta, synthesizes the stamp and bills the
    /// run's tokens; the meta then lands under the SAME id, reads
    /// `AlreadyStamped` and bills nothing. Tokens once. Before the markers
    /// carried the engine id, the meta read the synthesized stamp as "another
    /// run's", overwrote it and billed a second time. The cost and model the
    /// meta carried are lost — ruling U5, asserted so a change to it is seen.
    #[tokio::test]
    async fn a_synthesized_stamp_then_its_own_meta_bills_the_run_once() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "synth_then_meta.db");
        let id = SessionId::ephemeral("synth-then-meta");
        store.get_or_create(&id).await.unwrap();

        let tid = uuid::Uuid::new_v4();
        let run = [
            (1, run_started("engine-r")),
            (2, assistant_msg_billed(tid, 45, 25)),
            (3, run_finished("engine-r")),
        ];
        let log = own_event_log(&id, &run).await;
        let projector = MessageProjector::with_event_store(store.clone(), None, Some(log.clone()));
        for (seq, ev) in &run {
            projector.on_appended(&id, &rec(*seq, ev.clone()));
        }
        projector.flush(Duration::from_secs(5)).await.unwrap();
        let repair = projector.request_repair(&id).await;
        assert_eq!(
            (repair.stamps_synthesized, repair.usage_rebilled),
            (1, 1),
            "the repair ran inside the window and synthesized: {repair:?}"
        );

        let meta_ev = run_meta(tid, "engine-r");
        log.append(&id, 4, &meta_ev, 0).await.unwrap();
        projector.on_appended(&id, &rec(4, meta_ev));
        projector.flush(Duration::from_secs(5)).await.unwrap();

        let meta = store.get_metadata(&id).await.unwrap().unwrap();
        assert_eq!(
            (meta.input_tokens, meta.output_tokens),
            (45, 25),
            "tokens once"
        );
        assert_eq!(
            (meta.estimated_cost_usd, meta.model.as_deref()),
            (0.0, None),
            "U5: the late meta's cost and model are not accumulated"
        );
    }

    /// One engine run whose first attempt failed and was retried under the
    /// same run id: two brackets, one meta after the last. The meta bills the
    /// last bracket; the first — finished, with a row, no meta — is
    /// synthesized and billed once from its own messages; a second repair adds
    /// nothing. The provider billed both attempts, so both are counted.
    #[tokio::test]
    async fn a_retried_run_bills_each_bracket_once() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "retried_run.db");
        let id = SessionId::ephemeral("retried-run");
        store.get_or_create(&id).await.unwrap();

        let tid = uuid::Uuid::new_v4();
        let events = vec![
            (1, run_started("engine-r")),
            (2, assistant_msg_billed(tid, 10, 5)),
            (
                3,
                SessionEvent::RunFinished {
                    run_id: "engine-r".into(),
                    outcome: crate::session::events::RunOutcome::Errored,
                    at: 2,
                },
            ),
            (4, run_started("engine-r")),
            (5, assistant_msg_billed(tid, 45, 25)),
            (6, run_finished("engine-r")),
            (7, run_meta(tid, "engine-r")),
        ];
        let log = own_event_log(&id, &events).await;
        let projector = MessageProjector::with_event_store(store.clone(), None, Some(log));
        for (seq, ev) in &events {
            projector.on_appended(&id, &rec(*seq, ev.clone()));
        }
        projector.flush(Duration::from_secs(5)).await.unwrap();
        let tokens = |m: &crate::gateway::session_store::types::SessionMetadata| {
            (m.input_tokens, m.output_tokens)
        };
        assert_eq!(
            tokens(&store.get_metadata(&id).await.unwrap().unwrap()),
            (45, 25),
            "the live meta billed the bracket it closes"
        );

        let first = projector.request_repair(&id).await;
        assert_eq!(
            (first.stamps_synthesized, first.usage_rebilled),
            (1, 1),
            "{first:?}"
        );
        let second = projector.request_repair(&id).await;
        assert!(second.up_to_date, "{second:?}");
        assert_eq!(
            tokens(&store.get_metadata(&id).await.unwrap().unwrap()),
            (55, 30),
            "each bracket billed exactly once"
        );
    }

    /// A session that straddles the upgrade: run a was written before the
    /// markers carried the engine id (marker ≠ meta id), run b after (same id).
    /// Both are billed once live, and two repairs add nothing.
    #[tokio::test]
    async fn a_legacy_run_and_a_same_id_run_in_one_log_are_each_billed_once() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "mixed_shapes.db");
        let id = SessionId::ephemeral("mixed-shapes");
        store.get_or_create(&id).await.unwrap();

        let tid = uuid::Uuid::new_v4();
        let mut events = one_billed_run(tid, "a");
        events.extend([
            (5, run_started("b")),
            (6, assistant_msg_billed(tid, 10, 5)),
            (7, run_finished("b")),
            (8, run_meta(tid, "b")),
        ]);
        let log = own_event_log(&id, &events).await;
        let projector = MessageProjector::with_event_store(store.clone(), None, Some(log));
        for (seq, ev) in &events {
            projector.on_appended(&id, &rec(*seq, ev.clone()));
        }
        projector.flush(Duration::from_secs(5)).await.unwrap();
        for pass in 1..=2 {
            let repair = projector.request_repair(&id).await;
            assert!(repair.up_to_date, "repair {pass}: {repair:?}");
        }
        let meta = store.get_metadata(&id).await.unwrap().unwrap();
        assert_eq!((meta.input_tokens, meta.output_tokens), (55, 30));
        let ids: Vec<Option<String>> = store
            .get_history(&id, None)
            .await
            .unwrap()
            .into_iter()
            .filter(|m| m.role == "assistant")
            .map(|m| {
                m.metadata
                    .as_ref()
                    .and_then(|v| v.get("run_id"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
            .collect();
        assert_eq!(ids, vec![Some("a".to_string()), Some("b".to_string())]);
    }

    /// Final review I1, at the store: a `chat.rewind` / `session.truncate` /
    /// `/undo` whose cut lands inside a finished run the live drain already
    /// billed retires the run's meta and, in the same batch, appends the
    /// closer production's own [`open_run_after_retire`] derives for that cut
    /// — `RunFinished { Cancelled }` when the surviving prefix leaves the run
    /// open (cuts (i) and (iii)), nothing when it already ends in the run's
    /// `RunFinished` (cut (ii): a retire-only batch), exactly as
    /// `retire_from_and_close_run` writes it — and realigns the transcript.
    /// The fixture calls the helper rather than spelling a closer per cut so
    /// it cannot drift from the balancer. The next whole-session heal must
    /// then bill NOTHING, wherever the cut fell: (i) at the second message —
    /// the stamped row is retired and the first message survives unstamped
    /// (before the fix: synthesized under the marker id and billed again,
    /// 55/30 → 100/55); (ii) at the meta — the stamped row survives carrying
    /// the engine id (before: `already_stamped_by` read the marker id as a
    /// different run, overwrote the stamp and billed again, → 110/60);
    /// (iii) at the first message — no assistant row survives, so nothing
    /// was ever synthesizable; kept as the controller's named case and as
    /// the control that the cut itself is not what changes the totals.
    /// Reddens if the fold stops reading the retired meta.
    #[tokio::test]
    async fn a_rewind_inside_a_billed_run_does_not_bill_it_again() {
        let billed = |m: &crate::gateway::session_store::types::SessionMetadata| {
            (m.input_tokens, m.output_tokens)
        };
        for (cut, case) in [
            (
                3,
                "cut at the second message: stamped row retired, first row survives unstamped",
            ),
            (
                5,
                "cut at the meta: the stamped row survives with the engine id",
            ),
            (2, "cut at the first message: no assistant row survives"),
        ] {
            let temp = tempdir().unwrap();
            let store = sqlite_store(temp.path(), &format!("rewind_billed_{cut}.db"));
            let id = SessionId::ephemeral("rewind-billed");
            store.get_or_create(&id).await.unwrap();

            let tid = uuid::Uuid::new_v4();
            let events = vec![
                (1, run_started(&marker_of("a"))),
                (2, assistant_msg_billed(tid, 45, 25)),
                (3, assistant_msg_billed(tid, 10, 5)),
                (4, run_finished(&marker_of("a"))),
                (5, run_meta(tid, "a")),
            ];
            let log = own_event_log(&id, &events).await;
            let projector =
                MessageProjector::with_event_store(store.clone(), None, Some(log.clone()));
            for (seq, ev) in &events {
                projector.on_appended(&id, &rec(*seq, ev.clone()));
            }
            projector.flush(Duration::from_secs(5)).await.unwrap();
            assert_eq!(
                billed(&store.get_metadata(&id).await.unwrap().unwrap()),
                (55, 30),
                "{case}: the live drain billed the run once, from both messages"
            );

            // The rewind: retire from `cut` and close the run the surviving
            // prefix leaves open, in ONE batch; then realign the projection
            // by source seq, as `chat.rewind` does. The closer is whatever
            // the balancer's own builder derives from the live log at this
            // cut — possibly none — never a closer hand-picked per arm.
            let live = log.load_all_events(&id).await.unwrap();
            let closer: Vec<(SessionEvent, i64)> =
                crate::session::open_run_after_retire(&live, cut)
                    .expect("the fixture log reduces")
                    .into_iter()
                    .map(|run_id| {
                        (
                            SessionEvent::RunFinished {
                                run_id,
                                outcome: crate::session::events::RunOutcome::Cancelled,
                                at: 9,
                            },
                            0,
                        )
                    })
                    .collect();
            log.append_batch(&id, 6, &closer, Some(Retire::From(cut)), Durability::Normal)
                .await
                .unwrap();
            for (ev, _) in &closer {
                projector.on_appended(&id, &rec(6, ev.clone()));
            }
            projector.flush(Duration::from_secs(5)).await.unwrap();
            store.delete_messages_from_seq(&id, cut).await.unwrap();

            // One assertion per pass, so a red carries the report AND the
            // totals it doubled in the same panic.
            for pass in 1..=2 {
                let repair = projector.request_repair(&id).await;
                assert_eq!(
                    (
                        repair.stamps_synthesized,
                        repair.usage_rebilled,
                        repair.errored,
                        billed(&store.get_metadata(&id).await.unwrap().unwrap()),
                    ),
                    (0, 0, false, (55, 30)),
                    "{case}: whole-session pass {pass} must synthesize nothing and leave the \
                     totals as the live drain wrote them — the run's meta landed before the \
                     cut: {repair:?}"
                );
            }
            if cut == 5 {
                let row = store
                    .get_history(&id, None)
                    .await
                    .unwrap()
                    .into_iter()
                    .rfind(|m| m.role == "assistant")
                    .expect("both rows survive a cut at the meta");
                assert_eq!(
                    row.metadata
                        .as_ref()
                        .and_then(|m| m.get("run_id"))
                        .and_then(|v| v.as_str()),
                    Some("a"),
                    "{case}: the surviving stamped row keeps the meta's engine id"
                );
            }
        }
    }

    /// Same log, two scopes: a drain-triggered pass fills the row and leaves
    /// the finished run unstamped and unbilled — it runs on live sessions,
    /// inside the window between a `RunFinished` and its meta as a matter of
    /// course — and the whole-session pass that follows synthesizes the stamp
    /// and bills the run from its own messages.
    #[tokio::test]
    async fn a_known_gaps_heal_synthesizes_nothing_and_a_whole_session_heal_does() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "nometa_scope.db");
        let id = SessionId::ephemeral("nometa-scope");
        store.get_or_create(&id).await.unwrap();

        let tid = uuid::Uuid::new_v4();
        let log = own_event_log(
            &id,
            &[
                (1, run_started("r1")),
                (2, assistant_msg_billed(tid, 45, 25)),
                (3, run_finished("r1")),
            ],
        )
        .await;
        let pinned: Option<Arc<dyn SessionEventStore>> = Some(log.clone());
        let missed = Arc::new(StdMutex::new(MissedSeqs::default()));
        let mut run_start = HashMap::new();

        // Nothing recorded, so the floor is 1: this pass reads exactly the log
        // the whole-session pass below reads. The scope is the only difference.
        let known = heal_session(
            &store,
            &id,
            &missed,
            &pinned,
            &mut run_start,
            HealScope::KnownGaps,
        )
        .await;
        assert_eq!(
            (
                known.holes_filled,
                known.stamps_synthesized,
                known.usage_rebilled
            ),
            (1, 0, 0),
            "the row is filled, the stamp is not synthesized: {known:?}"
        );
        assert_eq!(
            store.get_metadata(&id).await.unwrap().unwrap().input_tokens,
            0,
            "a drain-triggered pass bills nothing for a run with no meta"
        );

        let whole = heal_session(
            &store,
            &id,
            &missed,
            &pinned,
            &mut run_start,
            HealScope::WholeSession,
        )
        .await;
        assert_eq!(
            (
                whole.holes_filled,
                whole.stamps_synthesized,
                whole.usage_rebilled
            ),
            (0, 1, 1),
            "{whole:?}"
        );
        assert!(
            !whole.up_to_date,
            "a pass that wrote a stamp did not find the session whole: {whole:?}"
        );
        let meta = store.get_metadata(&id).await.unwrap().unwrap();
        assert_eq!((meta.input_tokens, meta.output_tokens), (45, 25));
    }

    #[tokio::test]
    async fn projector_materializes_events_into_store_with_tokens() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "proj.db");
        let id = SessionId::ephemeral("proj");
        store.get_or_create(&id).await.unwrap();

        // Two Think steps — two LLM calls, two assistant rows — then the run's
        // stamp. The session's bill is the FOLD of the two rows (40 / 25): a
        // retry-discarded call is not in the log and is not counted here — that
        // is the per-call `SpendLedger`'s fact, not this one's.
        let tid = uuid::Uuid::new_v4();
        let events: [(EventSeq, SessionEvent); 7] = [
            (1, run_started(&marker_of("run_1"))),
            (2, user_msg(tid)),
            (3, assistant_msg_billed(tid, 10, 20)),
            (4, tool_req(tid)),
            (5, tool_res(tid)),
            (6, assistant_msg_billed(tid, 30, 5)),
            (7, run_meta(tid, "run_1")),
        ];
        let log = own_event_log(&id, &events).await;
        let projector = MessageProjector::with_event_store(store.clone(), None, Some(log));
        for (seq, ev) in events {
            projector.on_appended(&id, &rec(seq, ev));
        }
        projector.flush(Duration::from_secs(5)).await.unwrap();

        let msgs = store.get_history(&id, None).await.unwrap();
        assert_eq!(
            msgs.iter().filter(|m| m.role == "user").count(),
            1,
            "expected exactly 1 user row"
        );

        // Each assistant row carries the tokens of the ONE call that produced
        // it — not the turn's sum, and not zero.
        let asst: Vec<_> = msgs.iter().filter(|m| m.role == "assistant").collect();
        assert_eq!(asst.len(), 2, "expected one row per Think step");
        assert_eq!((asst[0].input_tokens, asst[0].output_tokens), (10, 20));
        assert_eq!((asst[1].input_tokens, asst[1].output_tokens), (30, 5));

        let meta = store
            .get_metadata(&id)
            .await
            .unwrap()
            .expect("missing session metadata");
        // The fold at the stamp is the session's ONLY token writer: the sum of
        // the two rows, once — not the rows added again by `add_message_full`.
        assert_eq!(meta.input_tokens, 40, "session input_tokens double-counted");
        assert_eq!(
            meta.output_tokens, 25,
            "session output_tokens double-counted"
        );
        assert_eq!(
            meta.model.as_deref(),
            Some("claude"),
            "sessions.model must be written from the run's report"
        );
        assert_eq!(meta.model_provider.as_deref(), Some("anthropic"));

        assert!(
            msgs.iter()
                .any(|m| m.role == "tool" && m.tool_name.as_deref() == Some("bash_exec")),
            "missing tool row with tool_name=bash_exec"
        );
    }

    /// A turn is appended to the SSOT, the user clears it, and only *then* does
    /// the async drain reach those events. Before the write-time retirement
    /// check they were written into `messages` anyway — the clear silently
    /// un-clearing itself in the transcript milliseconds later.
    #[tokio::test]
    async fn clear_before_the_drain_lands_writes_no_rows() {
        let events: Arc<dyn SessionEventStore> = crate::session::store::install_test_event_store();
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "clear_race.db");
        let id = SessionId::ephemeral("clear-race");
        store.get_or_create(&id).await.unwrap();

        let tid = uuid::Uuid::new_v4();
        let turn: [(EventSeq, SessionEvent); 2] = [(1, user_msg(tid)), (2, assistant_msg(tid))];
        for (seq, ev) in &turn {
            events.append(&id, *seq, ev, 0).await.unwrap();
        }

        // `chat.clear` retires the log while the events are still queued.
        events.retire_from(&id, 1).await.unwrap();

        // The drain now reaches them.
        let never = |_: EventSeq| false;
        let ctx = ProjectionCtx {
            store: &store,
            events: Some(&events),
            present: &never,
            run_start: 0,
            bus: None,
        };
        for (seq, ev) in &turn {
            project_event(&id, &rec(*seq, ev.clone()), &ctx).await;
        }

        assert!(
            store.get_history(&id, None).await.unwrap().is_empty(),
            "a retired event must not be materialised by a late drain"
        );
    }

    /// The receipt has to reach the surface clients actually read.
    ///
    /// `chat.history` serves the `messages` projection, not the event log, so
    /// persisting the block reason in `session_events` alone would leave a
    /// reloading tab exactly where it was: an unanswered user message and no
    /// explanation.
    #[tokio::test]
    async fn a_refusal_receipt_is_served_to_a_reattaching_client() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "refusal.db");
        let id = SessionId::ephemeral("refusal");
        store.get_or_create(&id).await.unwrap();

        let tid = uuid::Uuid::new_v4();
        let never = |_: EventSeq| false;
        let ctx = live_ctx(&store, &never, None);
        project_event(&id, &rec(1, user_msg(tid)), &ctx).await;
        project_event(
            &id,
            &rec(
                2,
                SessionEvent::Error {
                    turn_id: Some(tid),
                    kind: crate::session::events::ErrorKind::Guardrail,
                    message: "blocked by pii guardrail".into(),
                    recoverable: false,
                    at: 0,
                },
            ),
            &ctx,
        )
        .await;

        let rows = store.get_history(&id, None).await.unwrap();
        assert_eq!(rows.len(), 2, "the receipt must be a row of its own");
        // `system` is what the Panel renders as a centred notice rather than a
        // bubble attributed to somebody — nobody said this, the run did.
        assert_eq!(rows[1].role, "system");
        assert!(
            rows[1].content.contains("blocked by pii guardrail"),
            "the reason must survive the projection, got {:?}",
            rows[1].content
        );
    }

    /// The seq-set predicate, which is what replaced the watermark: a row that
    /// is already there is not written again, and a HOLE BELOW IT still is.
    /// A watermark answers the first question and gets the second one wrong.
    #[tokio::test]
    async fn a_hole_below_the_newest_row_is_still_a_hole() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "hole.db");
        let id = SessionId::ephemeral("hole");
        store.get_or_create(&id).await.unwrap();

        // seqs 11 and 12 are materialised; 10 is the gap.
        let present_set: HashSet<EventSeq> = [11u64, 12].into_iter().collect();
        let present = |s: EventSeq| present_set.contains(&s);
        let ctx = ProjectionCtx {
            store: &store,
            events: None,
            present: &present,
            run_start: 0,
            bus: None,
        };
        let tid = uuid::Uuid::new_v4();
        assert_eq!(
            project_event(&id, &rec(11, user_msg(tid)), &ctx).await,
            Projected::Nothing,
            "a present seq must not be written twice"
        );
        assert_eq!(
            project_event(&id, &rec(10, user_msg(tid)), &ctx).await,
            Projected::Row,
            "a gap below the newest row must be filled"
        );
        let rows = store.get_history(&id, None).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, row_id(&id.to_key_string(), 10));
    }

    /// The wire itself, not the decision that feeds it.
    ///
    /// [`peer_echo_frame`]'s own tests all stay green if the `publish_frame`
    /// call is deleted — they assert what the frame WOULD be, which is the
    /// classic "guards the origin, not the connection" hole. This one fails if
    /// the publish is removed, if it moves ahead of the append, or if it stops
    /// carrying the row's text.
    #[tokio::test]
    async fn an_attributed_user_row_is_announced_on_the_bus_as_it_is_written() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "echo.db");
        let id = SessionId::ephemeral("echo");
        store.get_or_create(&id).await.unwrap();

        let bus = Arc::new(crate::gateway::event_bus::GatewayEventBus::new());
        let mut rx = bus.subscribe_typed();

        let never = |_: EventSeq| false;
        let ctx = live_ctx(&store, &never, Some(&bus));
        project_event(&id, &rec(1, room_msg(Some("u-alice"), false)), &ctx).await;

        let frame = rx.try_recv().expect("the appended row must be announced");
        let GatewayEventFrame::SessionUserMessage {
            session_key,
            author_user_id,
            content,
            seq,
            ..
        } = frame
        else {
            panic!("wrong frame variant on the bus");
        };
        assert_eq!(session_key, id.to_key_string());
        assert_eq!(author_user_id, "u-alice");
        assert_eq!(seq, 1);

        // The announced text is the row's text — the property that makes a
        // live bubble and its reloaded twin the same bubble.
        let rows = store.get_history(&id, None).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(content, rows[0].content);
    }

    /// Same call, unattributed: a row is still written, and nothing is said.
    /// This is every single-author session, i.e. the default deployment.
    #[tokio::test]
    async fn an_unattributed_user_row_is_written_but_not_announced() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "echo_solo.db");
        let id = SessionId::ephemeral("echo_solo");
        store.get_or_create(&id).await.unwrap();

        let bus = Arc::new(crate::gateway::event_bus::GatewayEventBus::new());
        let mut rx = bus.subscribe_typed();

        let never = |_: EventSeq| false;
        let ctx = live_ctx(&store, &never, Some(&bus));
        project_event(&id, &rec(1, room_msg(None, false)), &ctx).await;

        assert_eq!(
            store.get_history(&id, None).await.unwrap().len(),
            1,
            "the row must still be materialised"
        );
        assert!(
            rx.try_recv().is_err(),
            "nothing to announce without an author"
        );
    }

    /// A dead drain used to be the end of the projection for the life of the
    /// process: `on_appended` logged "event lost" and returned. Now the seq is
    /// remembered, the writer is respawned, and a repair puts the row in.
    ///
    /// The event log is the shared in-process test store, so the events this
    /// test appends to it are the ones the heal re-reads.
    #[tokio::test]
    async fn a_dead_drain_is_restarted_and_the_seqs_it_missed_are_healed() {
        let events = crate::session::store::install_test_event_store();
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "restart.db");
        let id = SessionId::ephemeral("drain-restart");
        store.get_or_create(&id).await.unwrap();
        let projector = MessageProjector::new(store.clone(), None);

        let tid = uuid::Uuid::new_v4();
        let turn: [(EventSeq, SessionEvent); 2] = [(1, user_msg(tid)), (2, assistant_msg(tid))];
        for (seq, ev) in &turn {
            events.append(&id, *seq, ev, *seq as i64).await.unwrap();
        }

        projector.kill_drain();

        for (seq, ev) in &turn {
            projector.on_appended(&id, &rec(*seq, ev.clone()));
        }
        // Only seq 1 is missed: the `Closed` send RECORDS it and restarts the
        // drain in the same breath, so seq 2 goes down the fresh channel and is
        // projected live. That asymmetry IS the restart working — the old code
        // logged "event lost" for both and never wrote either.
        assert_eq!(
            projector.missed_seqs(&id),
            [1u64].into_iter().collect::<BTreeSet<_>>(),
            "a closed channel must record the seq it could not deliver"
        );

        let report = projector.request_repair(&id).await;
        assert!(!report.errored, "the repair ran on a restarted drain");
        // The EFFECT, not the report: the restarted drain notices the recorded
        // seq while handling seq 2 and heals it inline, so by the time an
        // explicit repair is answered the work may already be done. Counting
        // the explicit pass's `holes_filled` would then read 0 and call that a
        // failure — what has to be true is that both rows exist.
        let rows = store.get_history(&id, None).await.unwrap();
        assert_eq!(
            rows.len(),
            2,
            "the row the dead drain never wrote must be there too: {rows:?}"
        );
        assert_eq!(rows.iter().filter(|m| m.role == "user").count(), 1);
        assert_eq!(rows.iter().filter(|m| m.role == "assistant").count(), 1);
        assert!(
            projector.missed_seqs(&id).is_empty(),
            "a healed seq must leave the missed set"
        );
    }

    /// A repair on a session with no gaps must say so rather than re-writing
    /// the transcript: `present` is a set built from the transcript's own row
    /// ids, so every seq is already accounted for.
    #[tokio::test]
    async fn repairing_a_whole_session_writes_nothing_and_says_up_to_date() {
        let events = crate::session::store::install_test_event_store();
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "whole.db");
        let id = SessionId::ephemeral("whole");
        store.get_or_create(&id).await.unwrap();
        let projector = MessageProjector::new(store.clone(), None);

        let tid = uuid::Uuid::new_v4();
        for (seq, ev) in [(1u64, user_msg(tid)), (2, assistant_msg(tid))] {
            events.append(&id, seq, &ev, seq as i64).await.unwrap();
            projector.on_appended(&id, &rec(seq, ev));
        }
        projector.flush(Duration::from_secs(5)).await.unwrap();
        let before = store.get_history(&id, None).await.unwrap().len();
        assert_eq!(before, 2);

        let report = projector.request_repair(&id).await;
        assert!(report.up_to_date, "nothing was missing: {report:?}");
        assert_eq!(report.holes_filled, 0);
        assert_eq!(
            store.get_history(&id, None).await.unwrap().len(),
            2,
            "a repair must not duplicate rows"
        );
    }

    /// A transcript with no projector seq ids (foreign / pre-SSOT content)
    /// cannot be told apart from a fully-holed one, so a repair leaves it
    /// alone and SAYS it left it alone.
    #[tokio::test]
    async fn a_legacy_transcript_is_named_not_duplicated() {
        let _events = crate::session::store::install_test_event_store();
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "legacy.db");
        let id = SessionId::ephemeral("legacy");
        store.get_or_create(&id).await.unwrap();
        store
            .append_message(
                &id,
                MessageRecord {
                    id: "legacy-row-1".into(),
                    role: "user".into(),
                    content: "old".into(),
                    timestamp: 0,
                    metadata: None,
                    input_tokens: 0,
                    output_tokens: 0,
                    tool_call_id: None,
                    tool_name: None,
                },
            )
            .await
            .unwrap();

        let projector = MessageProjector::new(store.clone(), None);
        let report = projector.request_repair(&id).await;
        assert!(
            report.legacy,
            "a seq-less transcript must be named: {report:?}"
        );
        assert_eq!(report.holes_filled, 0);
        assert_eq!(store.get_history(&id, None).await.unwrap().len(), 1);
    }

    /// A requested repair sweeps the WHOLE session, not the part above the
    /// lowest seq this process happens to have recorded.
    ///
    /// Both callers of `request_repair` — the boot reconciler and the
    /// `core/projection-holes` doctor check — are asking about holes left by
    /// ANOTHER process, which by construction left no in-process record of
    /// them. Taking the floor from `missed` starts the pass above those holes
    /// and answers "filled 0" for a session the doctor's own unbounded
    /// comparison just measured as holed: a no-op that reports success.
    #[tokio::test]
    async fn a_requested_repair_sweeps_below_the_seqs_this_process_missed() {
        let events = crate::session::store::install_test_event_store();
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "floor.db");
        let id = SessionId::ephemeral("floor");
        store.get_or_create(&id).await.unwrap();
        let projector = MessageProjector::new(store.clone(), None);

        let tid = uuid::Uuid::new_v4();
        let log: [(EventSeq, SessionEvent); 5] = [
            (1, user_msg(tid)),
            (2, assistant_msg(tid)),
            (3, user_msg(tid)),
            (4, assistant_msg(tid)),
            (5, user_msg(tid)),
        ];
        for (seq, ev) in &log {
            events.append(&id, *seq, ev, *seq as i64).await.unwrap();
        }

        // Seqs 1 and 2 are the previous process's hole: durable in the log,
        // absent from the transcript, and unknown to anything in this one.
        for (seq, ev) in &log[2..4] {
            projector.on_appended(&id, &rec(*seq, ev.clone()));
        }
        projector.flush(Duration::from_secs(5)).await.unwrap();
        assert_eq!(store.get_history(&id, None).await.unwrap().len(), 2);

        // This process then records a miss of its own, ABOVE that hole.
        projector.kill_drain();
        projector.on_appended(&id, &rec(5, log[4].1.clone()));
        assert_eq!(
            projector.missed_seqs(&id),
            [5u64].into_iter().collect::<BTreeSet<_>>(),
            "the closed channel must record the seq it could not deliver"
        );

        let report = projector.request_repair(&id).await;
        assert!(!report.errored, "the repair ran: {report:?}");

        let key = id.to_key_string();
        let projected: BTreeSet<EventSeq> = store
            .get_history(&id, None)
            .await
            .unwrap()
            .iter()
            .filter_map(|m| parse_source_seq(&m.id, &key))
            .collect();
        assert_eq!(
            projected,
            [1u64, 2, 3, 4, 5].into_iter().collect::<BTreeSet<_>>(),
            "a requested repair must reach the holes below this process's own \
             missed seqs, not start at them: {projected:?}"
        );
    }

    /// SSOT (P1): `session_events` is the log and this projector is the ONLY
    /// production writer of the `messages` projection. Derived from the tree,
    /// equality on the set of files: a second writer bypasses the log — its
    /// rows are invisible to `request_repair`, to the resume pass and to
    /// `chat.rewind`'s retire — and must route through the event log instead.
    ///
    /// Measured at `5e85060b8` the set was `{orphan_notice.rs,
    /// agent_instance.rs, session_projector.rs}`: T16 folded the first into the
    /// resume arm, T17 deleted the second's writers (zero production callers).
    /// The store's own tree (`gateway/session_store/`) defines and forwards the
    /// method and originates no row, so it is out of scope by construction.
    /// Mutation (T17): restore `orphan_notice.rs` from `5e85060b8` and its
    /// `pub mod` line ⇒ red naming the file.
    ///
    /// One spelling only: `.append_message(`. The verb has two other faces —
    /// `SessionManager::add_message` / `add_message_full`
    /// (`session_manager/ops/crud.rs`), which the sqlite `append_message`
    /// forwards into — and this census does not look for them. Grep
    /// (2026-09-13): zero production callers of `.add_message(`, and
    /// `.add_message_full(`'s only callers are `add_message` itself and the
    /// sqlite `append_message` impl, so the SSOT claim holds through them
    /// too; a production caller of either would be a second writer this
    /// test cannot see.
    #[test]
    fn the_projector_is_the_only_production_writer_of_the_messages_table() {
        use crate::utils::source_scan::{production_text, rust_sources_under, strip_comment_lines};
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let writers: BTreeSet<String> = rust_sources_under(&root.join("src"))
            .into_iter()
            .filter(|(rel, _)| !rel.starts_with("src/gateway/session_store/"))
            .filter(|(rel, text)| {
                strip_comment_lines(&production_text(std::path::Path::new(rel), text))
                    .contains(".append_message(")
            })
            .map(|(rel, _)| rel)
            .collect();
        assert_eq!(
            writers,
            BTreeSet::from(["src/gateway/session_projector.rs".to_string()]),
            "a second `messages` writer bypasses the SSOT: route it through the event log \
             and `request_repair`"
        );
    }
}
