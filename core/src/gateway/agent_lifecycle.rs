//! Agent lifecycle events.
//!
//! Emitted via GatewayEventBus when agents are registered, started, or stopped.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Agent lifecycle event types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentLifecycleEvent {
    /// Agent was registered in the registry
    Registered {
        agent_id: String,
        workspace: PathBuf,
        model: String,
    },
    /// Agent execution started
    Started {
        agent_id: String,
        run_id: String,
    },
    /// Agent execution completed
    Completed {
        agent_id: String,
        run_id: String,
        success: bool,
    },
    /// Agent was unregistered (e.g., during config reload)
    Unregistered {
        agent_id: String,
    },
    /// Active agent was switched for a session
    Switched {
        agent_id: String,
        channel: String,
        peer_id: String,
        previous_agent_id: String,
    },
    /// Agent was deleted and its workspace archived
    Deleted {
        agent_id: String,
        workspace_archived: bool,
    },
    /// A sub-agent was spawned by a parent agent
    SubagentSpawned {
        parent_agent_id: String,
        child_run_id: String,
        task: String,
    },
    /// A sub-agent completed its task
    SubagentCompleted {
        child_run_id: String,
        outcome: String,
    },
}

impl AgentLifecycleEvent {
    /// Get the event topic string for EventBus routing.
    pub fn topic(&self) -> &'static str {
        match self {
            Self::Registered { .. } => "agent.lifecycle.registered",
            Self::Started { .. } => "agent.lifecycle.started",
            Self::Completed { .. } => "agent.lifecycle.completed",
            Self::Unregistered { .. } => "agent.lifecycle.unregistered",
            Self::Switched { .. } => "agent.lifecycle.switched",
            Self::Deleted { .. } => "agent.lifecycle.deleted",
            Self::SubagentSpawned { .. } => "agent.lifecycle.subagent_spawned",
            Self::SubagentCompleted { .. } => "agent.lifecycle.subagent_completed",
        }
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

        let started = AgentLifecycleEvent::Started {
            agent_id: "main".to_string(),
            run_id: "run-1".to_string(),
        };
        assert_eq!(started.topic(), "agent.lifecycle.started");
    }
}
