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
pub use snapshot::{Snapshot, SnapshotStore, SnapshotSummary, TopologyDiff};
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

/// Process-global snapshot store. Initialized once at daemon boot; `None`
/// until then reads as "snapshot subsystem absent" (tool degrades gracefully).
static SNAPSHOT_STORE: OnceCell<Arc<SnapshotStore>> = OnceCell::new();

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

/// Install the global snapshot store at boot. Idempotent.
pub fn init_snapshot_store(store: Arc<SnapshotStore>) {
    let _ = SNAPSHOT_STORE.set(store);
}

/// Read the global snapshot store, if initialized.
pub fn snapshot_store() -> Option<Arc<SnapshotStore>> {
    SNAPSHOT_STORE.get().cloned()
}

/// Publish a topology event to the global bus. No-op when the bus is absent
/// (tests, early boot) — the graph itself is durable, the event is advisory.
pub(crate) fn publish(ev: TopologyEvent) {
    if let Some(bus) = EVENT_BUS.get() {
        bus.publish(ev);
    }
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
}
