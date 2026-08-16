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

use crate::sync_primitives::Arc;
use once_cell::sync::OnceCell;

/// Process-global graph store. Initialized once at daemon boot
/// (`constructor.rs`); `None` until then so tests / early-boot read as
/// "no graph subsystem" (fail-soft, mirrors `goal::global`).
static GLOBAL: OnceCell<Arc<LoopGraphStore>> = OnceCell::new();

/// Process-global topology event bus. Initialized once at daemon boot, next
/// to [`init_global`]. `None` until then — publishing into an absent bus is a
/// no-op, so tests and early boot need no special casing.
static EVENT_BUS: OnceCell<TopologyEventBus> = OnceCell::new();

/// Install the global store at boot. Idempotent: a second call is ignored.
pub fn init_global(store: Arc<LoopGraphStore>) {
    let _ = GLOBAL.set(store);
}

/// Read the global store, if initialized.
pub fn global() -> Option<Arc<LoopGraphStore>> {
    GLOBAL.get().cloned()
}

/// Install the global topology event bus at boot. Idempotent.
pub fn init_event_bus(bus: TopologyEventBus) {
    let _ = EVENT_BUS.set(bus);
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
/// task holds no lock across yield points.
pub fn spawn_event_persister(
    bus: TopologyEventBus,
    store: Arc<SnapshotStore>,
) -> tokio::task::JoinHandle<()> {
    // Subscribe BEFORE spawning: events published between the subscribe and
    // the task's first `recv` sit in the bounded channel, not in the void.
    let mut rx = bus.subscribe();
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
                    tracing::warn!(dropped = n,
                        "loop_graph: audit persister lagged — events dropped (audit may lose, never block)");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

#[cfg(test)]
pub fn set_global_for_test(store: Arc<LoopGraphStore>) {
    let _ = GLOBAL.set(store);
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
}
