//! Loop-graph governance layer — the explicit topology over Aleph's
//! self-improvement loops (goal / cron / heartbeat / daemons).
//!
//! Design: docs/reference/GRAPH_LAYER.md (spec:
//! docs/superpowers/specs/2026-07-19-graph-engineering-loop-graph-layer-design.md).
//! The graph answers the four single-loop failures topologically — pairing
//! (`watches`), hierarchy (`owns_reference`), arbitration (`arbitrates`),
//! audit (`audits`) — and registers what no loop may argue with: `anchor`
//! (irrefutable measurements), `frozen` (rules enforced elsewhere), `root`
//! (human-supplied definitions of "better", origin=human enforced by the
//! store).
//!
//! Boundary (R7/R9/R10): this module is scaffolding only — topology storage,
//! structural lint, fact rendering. Every semantic verdict (is a win cheap?
//! is a reference wrong? which side of a conflict yields?) is an ordinary
//! LLM turn steered by `templates.rs` and the `loop-governance` skill. It
//! lives OUTSIDE `src/harness/` and adds zero per-turn cost when the graph
//! is empty. Dreaming and every other optimizer have no write path here —
//! the topology is the held-out layer that watches them.

pub mod events;
pub mod export;
pub mod inspector;
pub mod service;
pub mod snapshot;
pub mod store;
pub mod templates;
pub mod types;

pub use events::{TopologyEvent, TopologyEventBus};
pub use export::{to_dot, to_json};
pub use inspector::{ImpactReport, LoopGraphInspector, NodeSubgraph, TopologySummary};
pub use service::notify_team_settled;
pub use service::notify_workflow_settled;
pub use snapshot::{EventRecord, Snapshot, SnapshotStore, SnapshotSummary, TopologyDiff};
pub use store::LoopGraphStore;
pub use templates::AUDIT_NODE_BODY;
pub use types::{EdgeKind, GraphEdge, GraphNode, NodeKind, Origin};

use crate::capability::{CapabilitySlot, MissingSemantics, SlotStatus};
use crate::sync_primitives::Arc;

/// Process-global graph store. Initialized once at daemon boot
/// (`constructor.rs`); `None` until then so tests / early-boot read as
/// "no graph subsystem" (fail-soft, mirrors `goal::global`).
///
/// `FailsOpen`. Two of the three readers do not merely return a legal-looking
/// value — they take the branch that GRANTS, and that is read from the
/// consumer rather than asserted here:
///
/// - `service::governing_owner` returns `Ok(None)` when this slot is absent.
///   Its one consumer is `builtin_tools::goal::governing_owner_or_refuse`, and
///   all three of its call sites are
///   `if let Some(owner) = governing_owner_or_refuse(&session)? { return Err(..) }`
///   — the refusal lives inside the `Some` arm, so `Ok(None)` falls through
///   and the write to a governed objective (or its `gate_command`) PROCEEDS.
///   The `owns_reference` edges live in `loop_graph.db`, not in this handle, so
///   a session that really is governed reads as ungoverned whenever boot failed
///   to install the store. That is this variant's definition verbatim: a gate
///   silently stops gating.
/// - `service::notify_node_settled` returns `true` — the settle claim is EARNED
///   without any watcher having been poked, retiring that review for good on a
///   key that never moves again.
/// - The third (`render_session_topology`) just renders nothing.
///
/// **Not `IndistinguishableDefault`**, which is what this slot shipped as and
/// is a real reading: on an install with no graph at all, `Ok(None)` is both
/// the correct answer and indistinguishable from any other, exactly like
/// `tools/result-budget-ceiling`'s uncapped read. What separates the two
/// variants in this round is whether a caller keeps behaving as though a bound
/// were enforced — and here one does. The ACL's stated job is to stop a
/// governed loop rewriting its own reference, and `governing_owner_or_refuse`'s
/// own doc says "I could not find out" must land on the deny side; it routes
/// the `Err` there and then lets the absent-subsystem `Ok(None)` through the
/// permit door. `providers/pinnable-set` is the same shape
/// (`pinnable_providers()?` ⇒ no set ⇒ no refusal) and is classified
/// `FailsOpen`; classifying this one lower made the same shape carry two
/// answers.
///
/// **Not `ConsumerDecides`**: the consumers do not decide. Two of the three
/// fold absence into a positive answer with no branch that could say otherwise.
///
/// No `reads_as` sentence any more, because this variant carries none — the
/// operator-facing string was the one thing lost in the reclassification, and
/// what it said ("the objective ACL's permit answer") is the argument for the
/// higher severity rather than a description of a legal value.
static GLOBAL: CapabilitySlot<Arc<LoopGraphStore>> =
    CapabilitySlot::new("loop-graph/store", MissingSemantics::FailsOpen);

/// Process-global topology event bus. Initialized once at daemon boot, next
/// to [`init_global`]. `None` until then — publishing into an absent bus is a
/// no-op, so tests and early boot need no special casing.
///
/// `FailsClosed`: [`publish`] is `if let Some(bus)` with no `else`, so topology
/// events are dropped and nothing is granted; and `loop_graph_manage`'s audit
/// block omits its own liveness line when the bus is absent, so the diagnostic
/// that would have said "the audit trail is not being written" is the thing
/// that goes missing. Dead and silent, which is what this variant names.
static EVENT_BUS: CapabilitySlot<TopologyEventBus> =
    CapabilitySlot::new("loop-graph/event-bus", MissingSemantics::FailsClosed);

/// The handles above, type-erased for the roster — see
/// [`crate::spend::global_ledger_slot`] for why this shape.
pub(crate) const fn global_slot() -> &'static dyn SlotStatus {
    &GLOBAL
}

pub(crate) const fn event_bus_slot() -> &'static dyn SlotStatus {
    &EVENT_BUS
}

/// Install the global store at boot. Idempotent: a second call is ignored.
pub fn init_global(store: Arc<LoopGraphStore>) {
    let _ = GLOBAL.install(store);
}

/// Read the global store, if initialized.
pub fn global() -> Option<Arc<LoopGraphStore>> {
    GLOBAL.get().cloned()
}

/// Install the global topology event bus at boot. Idempotent.
pub fn init_event_bus(bus: TopologyEventBus) {
    let _ = EVENT_BUS.install(bus);
}

/// Read the global event bus, if initialized.
pub fn event_bus() -> Option<TopologyEventBus> {
    EVENT_BUS.get().cloned()
}

/// Publish a topology event to the global bus. No-op when the bus is absent
/// (tests, early boot) — the graph itself is durable, the event is advisory.
pub(crate) fn publish(ev: TopologyEvent) {
    if let Some(bus) = EVENT_BUS.get() {
        bus.publish(ev);
    }
}

/// Spawn the audit persister — the event bus's one real consumer today.
///
/// Subscribes to `bus` and appends every event to the snapshot store's
/// `events` table (the governance audit log). Discipline, matching the
/// bus's own contract: the audit trail may LOSE events but must never BLOCK
/// the publishers — a `Lagged` receiver warns and continues, a failed append
/// warns and continues, and a closed channel (every sender dropped) ends the
/// task quietly.
///
/// Lifecycle: deliberately NOT tracked anywhere. The daemon has no global
/// shutdown mechanism for boot-spawned tasks, so this one shares the
/// daemon's lifetime and exits with the process; the synchronous SQLite
/// append per event is microseconds and never crosses an `.await`, so the
/// task holds no lock across yield points. Known tech debt: a panic in the
/// task body goes unobserved because no `JoinHandle` is kept and no panic
/// hook is installed. Operators who need to monitor the audit persister
/// should add a supervisor keyed off the bus `subscriber_count()`.
pub fn spawn_event_persister(
    bus: TopologyEventBus,
    store: Arc<SnapshotStore>,
) -> tokio::task::JoinHandle<()> {
    // Subscribe BEFORE spawning: events published between the subscribe and
    // the task's first `recv` sit in the bounded channel, not in the void.
    let mut rx = bus.subscribe();
    // Clone ONLY the lag counter, not the whole bus — capturing the bus
    // would keep the Sender alive past every other send site and prevent the
    // bounded channel from closing once the publisher is gone (the test
    // `persister_exits_when_the_channel_closes` exercises this exact
    // invariant).
    let lag_counter = bus.lag_counter();
    tracing::debug!("loop_graph: audit persister starting");
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    if let Err(e) = store.append_event(&ev) {
                        tracing::warn!(error = %e,
                            "loop_graph: audit persister failed to append event — audit gap");
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    lag_counter.fetch_add(n, std::sync::atomic::Ordering::AcqRel);
                    tracing::warn!(dropped = n,
                        "loop_graph: audit persister lagged — events dropped (audit may lose, never block)");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    tracing::debug!("loop_graph: audit persister exiting (bus closed)");
                    break;
                }
            }
        }
    })
}

#[cfg(test)]
pub fn set_global_for_test(store: Arc<LoopGraphStore>) {
    let _ = GLOBAL.install(store);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_then_global_returns_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(LoopGraphStore::open(&dir.path().join("g.db")).unwrap());
        set_global_for_test(store);
        assert!(global().is_some());
    }

    /// End-to-end: an event published on the bus lands in the snapshot DB's
    /// `events` table via the persister — the whole point of the wiring.
    #[tokio::test]
    async fn persister_appends_published_events_to_the_audit_log() {
        let dir = tempfile::tempdir().unwrap();
        let snaps =
            Arc::new(SnapshotStore::open(&dir.path().join("s.db")).expect("snapshot store"));
        let bus = TopologyEventBus::new();
        let handle = spawn_event_persister(bus.clone(), snaps.clone());

        bus.publish(TopologyEvent::NodeDeleted {
            agent_id: "main".into(),
            id: "cron:orphan".into(),
        });

        // The append is asynchronous; poll briefly instead of asserting a race.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let rows = loop {
            let rows = snaps.list_events(10, None).expect("list events");
            if !rows.is_empty() {
                break rows;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "published event never reached the events table"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        };
        assert_eq!(rows[0].kind, "node_deleted");
        assert!(rows[0].payload_json.contains("cron:orphan"));

        handle.abort();
    }

    /// A closed broadcast channel ends the persister task quietly (it must not
    /// spin or panic once every sender is gone).
    #[tokio::test]
    async fn persister_exits_when_the_channel_closes() {
        let dir = tempfile::tempdir().unwrap();
        let snaps =
            Arc::new(SnapshotStore::open(&dir.path().join("s.db")).expect("snapshot store"));
        let handle = {
            let bus = TopologyEventBus::new();
            spawn_event_persister(bus, snaps)
            // `bus` dropped here — the only sender is gone.
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !handle.is_finished() {
            assert!(
                std::time::Instant::now() < deadline,
                "persister did not exit after the channel closed"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// Both handles reach the roster with the right failure semantics.
    ///
    /// Neither variant carries a `reads_as` sentence now: `GLOBAL` was
    /// reclassified from `IndistinguishableDefault` to `FailsOpen` once its
    /// consumer was read rather than described — see that static's doc.
    ///
    /// No runtime tie here on purpose: `set_global_for_test` installs `GLOBAL`
    /// from sibling tests in this very module, so an "uninstalled read still
    /// resolves to ..." assertion would pass or fail on libtest's scheduling.
    /// A flaky guard teaches people to re-run.
    #[test]
    fn the_accessors_expose_both_handles_to_the_roster() {
        assert_eq!(global_slot().id(), "loop-graph/store");
        assert_eq!(event_bus_slot().id(), "loop-graph/event-bus");
        assert!(
            matches!(global_slot().missing(), MissingSemantics::FailsOpen),
            "`governing_owner`'s absent-store `Ok(None)` reaches \
             `goal::governing_owner_or_refuse`, whose three call sites put the refusal \
             inside the `Some` arm — so absence PERMITS the write to a governed \
             objective. Downgrading this to a Warning-severity variant ranks a \
             silently-permitting ACL below a missing redaction engine; got {:?}",
            global_slot().missing()
        );
        assert!(matches!(
            event_bus_slot().missing(),
            MissingSemantics::FailsClosed
        ));
    }
}
