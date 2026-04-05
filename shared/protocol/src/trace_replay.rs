//! Replay DTOs for querying persisted traces.
//!
//! Used by the gateway trace replay handler and consumed by
//! CLI / TUI / webchat frontends.

use serde::{Deserialize, Serialize};

use crate::events::AgentTraceEvent;

/// A single step in a trace replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTraceReplayEntry {
    pub step: u64,
    pub event: AgentTraceEvent,
}

/// Task metadata summary for replay display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTraceTaskSummary {
    pub task_id: String,
    pub session_id: String,
    pub agent_id: String,
    pub status: String,
    pub prompt_preview: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub started_at: Option<u64>,
    pub completed_at: Option<u64>,
    pub trace_count: usize,
    pub last_event_kind: Option<String>,
}

/// Full trace replay for a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTraceReplay {
    pub task: AgentTraceTaskSummary,
    pub traces: Vec<AgentTraceReplayEntry>,
}

/// Summary item for listing traces.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTraceReplayListItem {
    pub task_id: String,
    pub started_at: Option<String>,
    pub status: String,
    pub event_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_agent_trace_replay() {
        let replay = AgentTraceReplay {
            task: AgentTraceTaskSummary {
                task_id: "task-001".to_string(),
                session_id: "session-1".to_string(),
                agent_id: "agent-1".to_string(),
                status: "completed".to_string(),
                prompt_preview: "hello".to_string(),
                created_at: 100,
                updated_at: 200,
                started_at: Some(110),
                completed_at: Some(190),
                trace_count: 1,
                last_event_kind: Some("session_completed".to_string()),
            },
            traces: vec![AgentTraceReplayEntry {
                step: 0,
                event: AgentTraceEvent::TurnStarted { iteration: 1 },
            }],
        };

        let json = serde_json::to_string(&replay).expect("serialize");
        let deserialized: AgentTraceReplay = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(deserialized.task.task_id, "task-001");
        assert_eq!(deserialized.task.status, "completed");
        assert_eq!(deserialized.traces.len(), 1);
        assert_eq!(deserialized.traces[0].step, 0);
        assert_eq!(
            deserialized.traces[0].event,
            AgentTraceEvent::TurnStarted { iteration: 1 }
        );
    }
}
