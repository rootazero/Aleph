//! Subagent tree relay — forwards live `SubAgentTreeUpdate` events to panels.
//!
//! The agents layer broadcasts [`AlephEvent::SubAgentTreeUpdate`] on the
//! [`GlobalBus`] (scope = root session) at spawn / progress / settle. This relay
//! is the consuming half: it republishes each event onto the gateway event bus
//! under topic `run.subagent_tree`, which `forward_bus_to_client` fans out to
//! every connected panel. Each panel filters client-side (mirroring how
//! `run.agent_trace` reaches the trace view).
//!
//! Pure I/O (R4/R10): unlike `subagent_announce`, this drives **no** parent
//! turn — it only mirrors observability state the tracker already recorded.

use tracing::info;

use crate::event::{AlephEvent, EventFilter, EventType, GlobalBus, GlobalEvent};
use crate::gateway::event_bus::{GatewayEventBus, TopicEvent};
use crate::sync_primitives::Arc;

/// Panel/TUI-facing topic — the shared protocol constant, not a local literal,
/// so the producer and every client filter name one string.
const TREE_TOPIC: &str = aleph_protocol::subagent_tree::TOPIC;

/// Subscribe to `SubAgentTreeUpdate` global events and republish each onto the
/// gateway event bus for the panel's background sub-agent tree view.
/// Registration mirrors the `spawn_subagent_announce` pattern in `aleph-server`
/// startup.
pub fn spawn_subagent_tree_relay(event_bus: Arc<GatewayEventBus>) {
    tokio::spawn(async move {
        let _sub = GlobalBus::global()
            .subscribe_async(
                EventFilter::new(vec![EventType::SubAgentTreeUpdate]),
                move |global_event: GlobalEvent| {
                    relay_one(&event_bus, &global_event);
                },
            )
            .await;
        info!("Subagent tree relay registered (SubAgentTreeUpdate → run.subagent_tree)");
    });
}

/// Republish one tree event onto the gateway bus as a `run.subagent_tree`
/// topic notification. Non-tree events are ignored.
fn relay_one(event_bus: &GatewayEventBus, global_event: &GlobalEvent) {
    let AlephEvent::SubAgentTreeUpdate(tree_event) = &global_event.event else {
        return;
    };
    let data = serde_json::to_value(tree_event).unwrap_or(serde_json::Value::Null);
    let notification = TopicEvent::new(TREE_TOPIC, data).to_notification();
    if let Ok(json) = serde_json::to_string(&notification) {
        event_bus.publish(json);
    }
}
