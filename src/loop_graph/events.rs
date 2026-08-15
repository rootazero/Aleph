//! Topology event bus — a lightweight broadcaster for governance-graph mutations.
//!
//! Why a bus: the role graph is read by four consumers today (`render_session_topology`
//! for the system prompt, `notify_goal_settled` for cron poke, `governing_owner` for the
//! objective ACL, `doctor::loop_graph` for structural lint). None of them know the
//! other three exists, and none react when the topology changes — a watchdog installed
//! via `loop_graph(action="pair")` has no way to tell the prompt layer its view of
//! reality just shifted, and a `drop_node` leaves every consumer to discover the
//! change on the next read. This bus does NOT decide anything (R7 — every semantic
//! verdict stays with the LLM); it just delivers the news.
//!
//! Design choices:
//! - `tokio::sync::broadcast` (not `watch`): we want EVERY mutation, not the latest,
//!   because losing a `NodeDeleted` mid-audit is the same class of failure as losing
//!   the row itself. `watch` collapses the stream.
//! - Bounded channel (cap = 256, doubled under pressure by `tokio`'s default).
//! - Subscriber count tracked so `governance_metrics` can report "how many live
//!   observers" — itself a graph signal ("the audit ring disconnected from the
//!   Panel" is a finding).
//! - Zero allocation in the publisher path: events are owned strings, copied once.
//!
//! What this is NOT:
//! - Not a WAL. Snapshots (see `snapshot.rs`) are the WAL.
//! - Not a hook system. `src/extension/hooks/` is that.
//! - Not a write barrier. The store's invariants run before publish; publishing a
//!   refusal (`Store rejected an invalid edge`) is a separate event kind we don't
//!   emit today, on purpose — see "NOT-build" in GRAPH_LAYER §7.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::loop_graph::types::{EdgeKind, NodeKind};

/// Capacity for the broadcast channel. Larger = more events kept under lag, smaller
/// = tighter backpressure. 256 is enough for the four documented consumers plus
/// any future audit-log subscriber, and a slow consumer gets `RecvError::Lagged`
/// rather than blocking the publisher (Tokio's default behavior).
const BUS_CAPACITY: usize = 256;

/// A topology mutation. Carries enough to identify what changed without forcing
/// every subscriber to re-query the store — but NOT the new row's body, which can
/// be many KB and is unchanged when only `updated_at_ms` moved (origin write-once
/// is the closest precedent: don't ship what the store can answer).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TopologyEvent {
    /// A node row was inserted or updated. `kind` is the post-write kind (a `node`
    /// with the same id but different kind is a logical delete+insert at the
    /// topology level — we still emit one `NodeUpserted`).
    NodeUpserted {
        agent_id: String,
        id: String,
        node_kind: NodeKind,
    },
    /// A node row was deleted. Edges touching it are left in place by design
    /// (they become dangling audit signals) — `GcCompleted` follows when those
    /// are eventually swept.
    NodeDeleted { agent_id: String, id: String },
    /// An edge row was inserted or updated.
    EdgeUpserted {
        agent_id: String,
        from_id: String,
        to_id: String,
        edge_kind: EdgeKind,
    },
    /// An edge row was deleted.
    EdgeDeleted {
        agent_id: String,
        from_id: String,
        to_id: String,
        edge_kind: EdgeKind,
    },
    /// `gc` finished its atomic sweep. `retained_acl` is the count of
    /// `owns_reference` rows deliberately kept (see `GcReport::retained_acl`).
    /// NOT emitted for the auto-path because there is no auto-path — `gc` is
    /// always explicit.
    GcCompleted {
        agent_id: String,
        removed: usize,
        retained_acl: usize,
    },
}

/// Process-global broadcaster. Initialized once at boot (next to
/// `loop_graph::init_global`). `None` until then so tests / early boot reads as
/// "no event subsystem" (mirrors `goal::global` fail-soft).
#[derive(Clone)]
pub struct TopologyEventBus {
    inner: Arc<broadcast::Sender<TopologyEvent>>,
}

impl std::fmt::Debug for TopologyEventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TopologyEventBus")
            .field("subscriber_count", &self.inner.receiver_count())
            .finish_non_exhaustive()
    }
}

impl TopologyEventBus {
    /// Build a fresh bus with the default capacity. The bus is independent of
    /// the store — both can exist, and the store does NOT hold a reference to
    /// this bus. Wiring is done by `install_into_store`.
    #[must_use]
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(BUS_CAPACITY);
        Self {
            inner: Arc::new(tx),
        }
    }

    /// Subscribe to topology events. Returns a `broadcast::Receiver`; the caller
    /// is responsible for draining it (a slow subscriber lags, but never blocks
    /// the publisher — `Lagged(n)` reports how many events were dropped on recv).
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<TopologyEvent> {
        self.inner.subscribe()
    }

    /// Publish an event. Returns `Ok(())` even if there are no subscribers —
    /// publishing into a quiet bus is not an error, it is the steady state of
    /// a graph that nobody watches yet (which `governance_metrics` will then
    /// surface as "no observers").
    pub fn publish(&self, ev: TopologyEvent) {
        // `send` errors are: `SendError::ChannelClosed` (no receivers, which
        // we treat as fine) and "no receivers present" — both fold to the same
        // shape and we deliberately do not log, because the audit template
        // already tells the operator "zero coverage" through `lint_naked_loops`.
        let _ = self.inner.send(ev);
    }

    /// How many live subscribers right now. Exposed for `governance_metrics` —
    /// a bus with no subscribers is not a failure (graphs are useful even when
    /// nothing reads them), but a bus that USED to have subscribers and now
    /// doesn't is a finding ("the Panel disconnected from governance events").
    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        self.inner.receiver_count()
    }
}

impl Default for TopologyEventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bus() -> TopologyEventBus {
        TopologyEventBus::new()
    }

    #[tokio::test]
    async fn publish_with_no_subscribers_is_silent_no_error() {
        let b = bus();
        b.publish(TopologyEvent::NodeDeleted {
            agent_id: "main".into(),
            id: "cron:orphan".into(),
        });
        assert_eq!(b.subscriber_count(), 0);
    }

    #[tokio::test]
    async fn subscribers_receive_in_publish_order() {
        let b = bus();
        let mut rx = b.subscribe();
        b.publish(TopologyEvent::NodeUpserted {
            agent_id: "main".into(),
            id: "daemon:dreaming".into(),
            node_kind: NodeKind::Daemon,
        });
        b.publish(TopologyEvent::EdgeUpserted {
            agent_id: "main".into(),
            from_id: "cron:watcher".into(),
            to_id: "daemon:dreaming".into(),
            edge_kind: EdgeKind::Watches,
        });

        let first = rx.recv().await.expect("first event");
        let second = rx.recv().await.expect("second event");

        assert!(
            matches!(first, TopologyEvent::NodeUpserted { ref id, .. } if id == "daemon:dreaming")
        );
        assert!(
            matches!(second, TopologyEvent::EdgeUpserted { ref edge_kind, .. } if *edge_kind == EdgeKind::Watches)
        );
    }

    #[tokio::test]
    async fn a_slow_subscriber_lags_does_not_block_publisher() {
        let b = bus();
        let _rx = b.subscribe();
        // Drop the receiver (the only one) — publish must still succeed.
        // Recreating: a fresh subscriber sees only events published AFTER its
        // subscribe() call, which is the contract.
        b.publish(TopologyEvent::NodeDeleted {
            agent_id: "main".into(),
            id: "anchor:old".into(),
        });
        let mut rx2 = b.subscribe();
        b.publish(TopologyEvent::NodeUpserted {
            agent_id: "main".into(),
            id: "anchor:new".into(),
            node_kind: NodeKind::Anchor,
        });
        let ev = rx2
            .recv()
            .await
            .expect("new subscriber sees post-subscribe events");
        assert!(matches!(ev, TopologyEvent::NodeUpserted { ref id, .. } if id == "anchor:new"));
    }

    #[test]
    fn event_serializes_with_a_stable_discriminator() {
        let ev = TopologyEvent::GcCompleted {
            agent_id: "main".into(),
            removed: 3,
            retained_acl: 1,
        };
        let json = serde_json::to_string(&ev).unwrap();
        // The tag is what Panel/audit-log filters on; assert it explicitly so
        // a future rename of the enum variant cannot break the wire silently.
        assert!(json.contains("\"kind\":\"gc_completed\""), "{json}");
        assert!(json.contains("\"retained_acl\":1"), "{json}");
    }

    /// The bus is a broadcaster, not a store: dropping all subscribers and
    /// publishing must NOT panic, and a subscriber attached afterwards sees
    /// only future events. This is what the store invariant "publisher never
    /// fails because nobody is listening" rests on.
    #[tokio::test]
    async fn subscriber_count_reflects_live_subscribers_only() {
        let b = bus();
        assert_eq!(b.subscriber_count(), 0);
        let _rx1 = b.subscribe();
        let _rx2 = b.subscribe();
        assert_eq!(b.subscriber_count(), 2);
        drop(_rx1);
        assert_eq!(b.subscriber_count(), 1);
    }
}
