//! `MessageProjector` — a [`SessionEventObserver`] that materialises session
//! events into the `messages` table via a single ordered async drain task.
//!
//! Each assistant row carries the tokens of the single LLM call that produced
//! it, read straight off `AssistantMessage.usage` — the harness emits one
//! `AssistantMessage` per Think step, so calls and rows are 1:1.
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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::events::GatewayEventFrame;
use crate::gateway::session_store::types::MessageRecord;
use crate::gateway::session_store::{SessionStore, StampOutcome};
use crate::session::events::{EventSeq, SessionEvent, SessionEventRecord};
use crate::session::observer::SessionEventObserver;
use crate::session::projection::{parse_source_seq, project_row, row_id};
use crate::session::service::SessionId;
use crate::session::store::SessionEventStore;

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
    Event(SessionId, SessionEventRecord),
    /// Fill this session's holes and re-apply its stamps, then answer.
    Repair(SessionId, oneshot::Sender<RepairReport>),
    /// Answer once every message queued before this one has been handled.
    Flush(oneshot::Sender<()>),
}

/// What one `heal_session` pass did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RepairReport {
    /// Transcript rows that were absent and are now written.
    pub holes_filled: usize,
    /// `AssistantRunMeta` stamps that landed on a row that had none.
    pub stamps_reapplied: usize,
    /// Of those stamps, how many also accumulated the run's spend. A stamp
    /// that found the row already carrying this run's id bills nothing —
    /// that is what makes a replay non-double-billing.
    pub usage_rebilled: usize,
    /// Nothing was missing and nothing was re-stamped.
    pub up_to_date: bool,
    /// The transcript is non-empty and carries no projector seq ids (foreign
    /// or pre-SSOT content). Nothing was written: without seqs there is no way
    /// to tell a hole from a row this projector never wrote, and filling
    /// blindly would duplicate the conversation.
    pub legacy: bool,
    /// Something could not be read or written, so this report does NOT say the
    /// session is whole — it says the pass could not find out.
    pub errored: bool,
}

/// Seqs known to be absent from the projection, and which sessions have gained
/// one since the last heal.
///
/// `dirty` exists so a heal is triggered by NEW information only. Re-inserting
/// a seq that a pass could not resolve (a `RunMeta` whose run produced no
/// assistant row at all, say) must not re-arm the heal, or every subsequent
/// event on that session would pay for a full transcript read forever.
#[derive(Default)]
struct MissedSeqs {
    seqs: HashMap<SessionId, BTreeSet<EventSeq>>,
    dirty: HashSet<SessionId>,
}

impl MissedSeqs {
    /// A newly-discovered gap: remember it AND arm the next heal.
    fn record(&mut self, id: &SessionId, seq: EventSeq) {
        self.seqs.entry(id.clone()).or_default().insert(seq);
        self.dirty.insert(id.clone());
    }

    /// A gap this pass could not close: remember it WITHOUT arming a heal.
    fn restore(&mut self, id: &SessionId, seqs: BTreeSet<EventSeq>) {
        if seqs.is_empty() {
            return;
        }
        self.seqs.entry(id.clone()).or_default().extend(seqs);
    }

    /// Take this session's gaps for a heal pass to work on.
    fn take(&mut self, id: &SessionId) -> BTreeSet<EventSeq> {
        self.dirty.remove(id);
        self.seqs.remove(id).unwrap_or_default()
    }

    /// Has this session gained a gap since the last heal?
    fn is_dirty(&self, id: &SessionId) -> bool {
        self.dirty.contains(id)
    }
}

/// `flush` gave up waiting. Not "the drain is empty" — the caller does not know
/// what the drain still holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the projector drain did not settle within the flush timeout")]
pub struct FlushTimeout;

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
    /// Events the observer could not hand to the drain and that a heal has yet
    /// to pick up. Surfaced so the real-machine QA burst stage can report a
    /// number instead of an adjective.
    deferred: AtomicU64,
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
            deferred: AtomicU64::new(0),
        })
    }

    /// The projection this writes into — the doctor check reads the transcript
    /// through it rather than being handed a second handle to the same store.
    #[must_use]
    pub fn projection_store(&self) -> Arc<dyn SessionStore> {
        self.store.clone()
    }

    /// Events the observer could not enqueue and that no heal has yet picked
    /// up. Monotonic: it counts arrivals at the gap, not the current backlog.
    #[must_use]
    pub fn deferred_count(&self) -> u64 {
        self.deferred.load(Ordering::Relaxed)
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
            Projected::Stamped { billed } => {
                report.stamps_reapplied += 1;
                if billed {
                    report.usage_rebilled += 1;
                }
            }
            Projected::Nothing => {}
            Projected::Retry => {
                retry.insert(rec.seq);
            }
        }
    }
    lock_missed(missed).restore(id, retry);
    report.up_to_date =
        report.holes_filled == 0 && report.stamps_reapplied == 0 && !report.errored;
    report
}

/// The live peer-echo frame for a row that is about to be appended, or `None`
/// when this row is not one.
///
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
/// only the flag. `retire_from` (`chat.clear` / `chat.rewind`) means "erase" —
/// suppressing the row is the whole point. `retire_through` (manual `/compact`,
/// `context::compact::manual`) means "stop replaying, keep everything" — for it
/// the suppression is collateral: a compacted event that had not yet drained
/// loses its Panel row. Distinguishing the two would take a retirement *reason*
/// on the row; that is a schema change with no observed failure behind it. If
/// one ever shows up, this is the place.
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

/// What one projection step did — the drain's and the heal's shared vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Projected {
    /// A transcript row was appended.
    Row,
    /// An `AssistantRunMeta` stamp landed on a row that had none. `billed` says
    /// whether the run's spend was accumulated in the same step.
    Stamped { billed: bool },
    /// Nothing to do: not row-producing, already present, already stamped, or
    /// deliberately retired.
    Nothing,
    /// This seq must be tried again — its retirement state is unknown, a write
    /// failed, or the row a stamp needs is not in the projection yet.
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
            input_tokens,
            output_tokens,
            cost_usd,
            model,
            model_provider,
            ..
        } => {
            let occupancy = crate::gateway::execution_engine::helpers::RunContextOccupancy {
                context_tokens: *context_tokens,
                context_window: *context_window,
                total_tokens: *total_tokens,
                input_tokens: *input_tokens,
                output_tokens: *output_tokens,
                cost_usd: *cost_usd,
                model: model.clone(),
                model_provider: model_provider.clone(),
            };
            let Some(meta) = crate::gateway::agent_instance::build_message_metadata(
                Some(run_id),
                Some(occupancy),
            ) else {
                return Projected::Nothing;
            };
            // The row this run's numbers belong to is the last assistant row
            // BETWEEN the run's own `RunStarted` and this meta — not "the last
            // assistant row in the table", which on a session with two runs
            // (or with a later row already back-filled) is somebody else's.
            match ctx
                .store
                .stamp_assistant_metadata_in_range(id, ctx.run_start, rec.seq, &meta)
                .await
            {
                Ok(StampOutcome::AlreadyStamped) => Projected::Nothing,
                Ok(StampOutcome::NoRowInRange) => {
                    // The assistant row this stamp belongs to is not in the
                    // projection yet — a dropped row, or a heal that has not
                    // reached it. Billing here would charge a session for a run
                    // whose stamp will be applied (and billed) again later.
                    tracing::debug!(
                        session = ?id,
                        seq = rec.seq,
                        run_id = %run_id,
                        "projector: run-meta has no assistant row in range; deferring"
                    );
                    Projected::Retry
                }
                Ok(StampOutcome::Stamped) => {
                    // Accumulate this run's spend onto the session row, exactly
                    // once: the stamp above is the idempotence guard, because a
                    // replay of the same meta returns `AlreadyStamped` and never
                    // reaches this arm.
                    //
                    // THE run's report, not A report: `add_message_full` does not
                    // add each message row's tokens onto these same three
                    // columns. It did, silently, for as long as the rows carried
                    // zeros; the moment the rows became real that stopped being a
                    // harmless no-op and started double-billing the session.
                    let mut billed = false;
                    if *input_tokens > 0 || *output_tokens > 0 || cost_usd.is_some() {
                        match ctx
                            .store
                            .update_session_usage(
                                id,
                                i64::from(*input_tokens),
                                i64::from(*output_tokens),
                                cost_usd.unwrap_or(0.0),
                                model.as_deref(),
                                model_provider.as_deref(),
                            )
                            .await
                        {
                            Ok(()) => billed = true,
                            Err(e) => {
                                tracing::warn!(error = %e, "projector: session usage accumulation failed");
                            }
                        }
                    }
                    Projected::Stamped { billed }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "projector: stamp run-meta failed");
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
        let msg = ProjectorMsg::Event(id.clone(), record.clone());
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
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.deferred.fetch_add(1, Ordering::Relaxed);
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
                self.deferred.fetch_add(1, Ordering::Relaxed);
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
    use crate::orchestrator::dispatch::TokenBreakdown;
    use crate::session::events::{MessageContent, ToolOutput, TurnId};
    use crate::session::service::SessionError;
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

    fn run_meta(tid: TurnId, run: &str, input: u32, output: u32) -> SessionEvent {
        SessionEvent::AssistantRunMeta {
            turn_id: tid,
            run_id: run.into(),
            context_tokens: 1234,
            context_window: 200_000,
            total_tokens: u64::from(input + output),
            input_tokens: input,
            output_tokens: output,
            cost_usd: Some(0.12),
            model: Some("claude".into()),
            model_provider: Some("anthropic".into()),
            at: 3,
        }
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
        async fn append(
            &self,
            _id: &SessionId,
            _seq: EventSeq,
            _event: &SessionEvent,
            _created_at_ms: i64,
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
            Ok(Vec::new())
        }
        async fn load_head_seq(&self, _id: &SessionId) -> Result<EventSeq, SessionError> {
            Ok(0)
        }
        async fn retire_from(
            &self,
            _id: &SessionId,
            _from_seq: EventSeq,
        ) -> Result<usize, SessionError> {
            Ok(0)
        }
        async fn is_retired(
            &self,
            _id: &SessionId,
            _seq: EventSeq,
        ) -> Result<bool, SessionError> {
            Err(SessionError::Storage("event log unreadable".into()))
        }
        async fn load_run_markers(
            &self,
        ) -> Result<Vec<(SessionId, Vec<SessionEventRecord>)>, SessionError> {
            Ok(Vec::new())
        }
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
        let projector = MessageProjector::new(store.clone(), None);

        let tid = uuid::Uuid::new_v4();
        let events: &[(EventSeq, SessionEvent)] = &[
            (1, user_msg(tid)),
            (2, assistant_msg_billed(tid, 100, 50)),
            (3, run_meta(tid, "run_xyz", 4000, 1678)),
        ];
        for (seq, ev) in events {
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

        // Run A opened at seq 1, its meta at seq 3 → row seq 2.
        let never = |_: EventSeq| false;
        let ctx_a = ProjectionCtx {
            store: &store,
            events: None,
            present: &never,
            run_start: 1,
            bus: None,
        };
        assert_eq!(
            project_event(&id, &rec(3, run_meta(tid, "run_a", 10, 5)), &ctx_a).await,
            Projected::Stamped { billed: true }
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

    /// A run whose assistant row never made it into the projection must not be
    /// billed: the stamp has nowhere to land, so the meta is kept for a later
    /// heal instead of being spent.
    #[tokio::test]
    async fn a_run_meta_with_no_row_in_range_defers_and_does_not_bill() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "norow.db");
        let id = SessionId::ephemeral("norow");
        store.get_or_create(&id).await.unwrap();

        let never = |_: EventSeq| false;
        let ctx = ProjectionCtx {
            store: &store,
            events: None,
            present: &never,
            run_start: 1,
            bus: None,
        };
        let out = project_event(
            &id,
            &rec(3, run_meta(uuid::Uuid::new_v4(), "run_a", 40, 20)),
            &ctx,
        )
        .await;
        assert_eq!(out, Projected::Retry);
        let meta = store.get_metadata(&id).await.unwrap().unwrap();
        assert_eq!(
            (meta.input_tokens, meta.output_tokens),
            (0, 0),
            "a stamp with no row must not bill the session"
        );
    }

    /// Replay is the normal case now — a heal re-reads the whole range every
    /// time. The stamp is the idempotence guard, so the second pass must find
    /// the row already carrying this run's id and bill nothing.
    #[tokio::test]
    async fn replaying_one_run_meta_bills_once() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "rebill.db");
        let id = SessionId::ephemeral("rebill");
        store.get_or_create(&id).await.unwrap();
        store
            .append_message(
                &id,
                MessageRecord {
                    id: row_id(&id.to_key_string(), 2),
                    role: "assistant".into(),
                    content: "hello".into(),
                    timestamp: 2,
                    metadata: None,
                    input_tokens: 0,
                    output_tokens: 0,
                    tool_call_id: None,
                    tool_name: None,
                },
            )
            .await
            .unwrap();

        let tid = uuid::Uuid::new_v4();
        let never = |_: EventSeq| false;
        let ctx = ProjectionCtx {
            store: &store,
            events: None,
            present: &never,
            run_start: 1,
            bus: None,
        };
        let meta_rec = rec(3, run_meta(tid, "run_a", 45, 25));
        assert_eq!(
            project_event(&id, &meta_rec, &ctx).await,
            Projected::Stamped { billed: true }
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
    }

    #[tokio::test]
    async fn projector_materializes_events_into_store_with_tokens() {
        let temp = tempdir().unwrap();
        let store = sqlite_store(temp.path(), "proj.db");
        let id = SessionId::ephemeral("proj");
        store.get_or_create(&id).await.unwrap();
        let projector = MessageProjector::new(store.clone(), None);

        // Two Think steps — two LLM calls, two assistant rows — then the run's
        // one billing report.
        let tid = uuid::Uuid::new_v4();
        let events: [(EventSeq, SessionEvent); 7] = [
            (
                1,
                SessionEvent::RunStarted {
                    run_id: "run_1".into(),
                    at: 1,
                    project_root: None,
                    envelope: None,
                },
            ),
            (2, user_msg(tid)),
            (3, assistant_msg_billed(tid, 10, 20)),
            (4, tool_req(tid)),
            (5, tool_res(tid)),
            (6, assistant_msg_billed(tid, 30, 5)),
            // The run's billed total. Deliberately NOT 40/25 (the sum of the
            // two rows): a retry-discarded call is billed but never becomes a
            // message, so the session total is a superset of its rows.
            (7, run_meta(tid, "run_1", 45, 25)),
        ];
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
        // The run's report is the session's ONLY token writer.
        assert_eq!(meta.input_tokens, 45, "session input_tokens double-counted");
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
        let events: Arc<dyn SessionEventStore> =
            crate::session::store::install_test_event_store();
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
        assert!(report.legacy, "a seq-less transcript must be named: {report:?}");
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
}
