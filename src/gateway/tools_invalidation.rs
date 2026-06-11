//! Push notification helper for tool-catalog mutations.
//!
//! When the set of available tools changes (skills hot-reload, plugins
//! installed/removed, MCP server connected/disconnected, etc.) the panel and
//! other clients currently have to re-poll `tools.catalog` on their own
//! schedule — they have no way to know the catalog moved underneath them.
//!
//! [`ToolsChangedSink`] closes that gap. Mutation paths build a sink at boot
//! and call [`ToolsChangedSink::publish`] right after the registry has been
//! reconciled. The helper bumps the gateway `config` state-version counter
//! (so identity probes show drift) and broadcasts a `tools.changed` topic
//! event over the existing event bus, where it flows through the per-client
//! subscription filter just like any other event.
//!
//! Clients listen for `tools.changed` and refresh their cached catalog —
//! eliminating the polling-vs-staleness trade-off that pull-only discovery
//! forces.

use crate::sync_primitives::Arc;

use super::event_bus::{GatewayEventBus, TopicEvent};
use super::state_version::StateVersionTracker;

/// Boot-constructed handle for emitting `tools.changed` events.
///
/// Holds shared references to the gateway's state-version tracker and event
/// bus; calling [`Self::publish`] is fire-and-forget — failures to publish
/// are logged but never propagated, because catalog churn must not stall
/// the underlying mutation (e.g. an extension reload).
#[derive(Clone)]
pub struct ToolsChangedSink {
    state_versions: Arc<StateVersionTracker>,
    event_bus: Arc<GatewayEventBus>,
}

impl ToolsChangedSink {
    /// Construct a new sink wired to the live trackers.
    pub const fn new(state_versions: Arc<StateVersionTracker>, event_bus: Arc<GatewayEventBus>) -> Self {
        Self {
            state_versions,
            event_bus,
        }
    }

    /// Bump the `config` version and broadcast a `tools.changed` event.
    ///
    /// `scope` identifies the mutation source (`"extension"`, `"mcp"`,
    /// `"agent"`, ...) so clients can scope their refresh; `detail` carries
    /// any additional structured context (changed paths, server id, ...).
    pub fn publish(&self, scope: &str, detail: serde_json::Value) {
        let new_version = self.state_versions.bump_config();
        let event = TopicEvent::new(
            "tools.changed",
            serde_json::json!({
                "scope": scope,
                "config_version": new_version,
                "detail": detail,
            }),
        );
        if let Err(e) = self.event_bus.publish_json(&event) {
            tracing::warn!(error = %e, scope, "failed to publish tools.changed event");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sink() -> (
        ToolsChangedSink,
        Arc<StateVersionTracker>,
        Arc<GatewayEventBus>,
    ) {
        let sv = Arc::new(StateVersionTracker::new());
        let eb = Arc::new(GatewayEventBus::new());
        (ToolsChangedSink::new(sv.clone(), eb.clone()), sv, eb)
    }

    #[test]
    fn publish_bumps_config_version() {
        let (sink, sv, _eb) = sink();
        assert_eq!(sv.snapshot().config, 0);
        sink.publish("extension", serde_json::json!({"kind": "skill"}));
        assert_eq!(sv.snapshot().config, 1);
        sink.publish("mcp", serde_json::json!({"server_id": "github"}));
        assert_eq!(sv.snapshot().config, 2);
    }

    #[tokio::test]
    async fn publish_emits_topic_event_subscribers_can_see() {
        let (sink, _sv, eb) = sink();
        let mut rx = eb.subscribe();
        sink.publish("extension", serde_json::json!({"kind": "plugin"}));

        let raw = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .expect("event arrived")
            .expect("recv ok");
        let value: serde_json::Value = serde_json::from_str(&raw).expect("valid json");
        assert_eq!(value["topic"], "tools.changed");
        assert_eq!(value["data"]["scope"], "extension");
        assert_eq!(value["data"]["config_version"], 1);
        assert_eq!(value["data"]["detail"]["kind"], "plugin");
    }
}
