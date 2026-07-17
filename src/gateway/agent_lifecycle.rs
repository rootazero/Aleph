//! Agent lifecycle events.
//!
//! Emitted via `GatewayEventBus` (wrapped in a `TopicEvent`, see
//! [`crate::gateway::agent_binding`]) when agents are registered, deleted, or
//! re-bound to a channel. Every variant here has a live producer; variants
//! without one (Started/Completed/Unregistered/Subagent*) were removed —
//! they never fired on the wire, so no consumer can miss them.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Agent lifecycle event types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentLifecycleEvent {
    /// Agent was registered in the runtime registry (boot or `agents.create`).
    Registered {
        agent_id: String,
        workspace: PathBuf,
        model: String,
    },
    /// Agent was deleted and its workspace archived
    Deleted {
        agent_id: String,
        workspace_archived: bool,
    },
    /// A channel's active agent was switched (re-bound) to `agent_id`.
    Bound {
        agent_id: String,
        channel: String,
        /// The agent previously bound to the channel, if any.
        previous_agent: Option<String>,
    },
    /// A channel's explicit agent binding was cleared (back to default routing).
    Unbound {
        channel: String,
        /// The agent that was bound before the clear, if any.
        previous_agent: Option<String>,
    },
}

impl AgentLifecycleEvent {
    /// Get the event topic string for `EventBus` routing.
    #[must_use]
    pub const fn topic(&self) -> &'static str {
        match self {
            Self::Registered { .. } => "agent.lifecycle.registered",
            Self::Deleted { .. } => "agent.lifecycle.deleted",
            Self::Bound { .. } => "agent.lifecycle.bound",
            Self::Unbound { .. } => "agent.lifecycle.unbound",
        }
    }

    /// Publish this event on the bus, wrapped in a [`TopicEvent`].
    ///
    /// The wrap is load-bearing: a bare `publish_json` serializes topic-less,
    /// the WS forwarder reads that as topic `""`, and every concrete
    /// subscription (e.g. `agent.lifecycle.*`) drops it. This is the single
    /// publish path for lifecycle events — do not hand-roll the wrap at call
    /// sites. Delivery failures are non-fatal (bus may have no subscribers).
    pub fn publish(&self, bus: &crate::gateway::event_bus::GatewayEventBus) {
        let _ = bus.publish_json(&crate::gateway::event_bus::TopicEvent::new(
            self.topic(),
            serde_json::to_value(self).unwrap_or_default(),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lifecycle_event_serialization() {
        let event = AgentLifecycleEvent::Registered {
            agent_id: "coding".to_string(),
            workspace: PathBuf::from("/tmp/ws"),
            model: "claude-opus-4-6".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"registered\""));
        assert!(json.contains("\"agent_id\":\"coding\""));
    }

    #[test]
    fn test_lifecycle_topics() {
        let reg = AgentLifecycleEvent::Registered {
            agent_id: "main".to_string(),
            workspace: PathBuf::from("/tmp"),
            model: "test".to_string(),
        };
        assert_eq!(reg.topic(), "agent.lifecycle.registered");

        let unbound = AgentLifecycleEvent::Unbound {
            channel: "telegram".to_string(),
            previous_agent: Some("trader".to_string()),
        };
        assert_eq!(unbound.topic(), "agent.lifecycle.unbound");
    }
}
