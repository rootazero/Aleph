//! Agent Loop Events for Swarm Intelligence
//!
//! This module defines semantic events published by AgentLoop at key operation points.
//! These events are NOT tier-classified - SwarmCoordinator handles classification.

use serde::{Deserialize, Serialize};
use crate::agent_loop::decision::ActionResult;

/// Semantic events published by AgentLoop at key operation points
/// These events are NOT tier-classified - SwarmCoordinator handles classification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentLoopEvent {
    /// Tool execution started
    ActionInitiated {
        agent_id: String,
        action_type: String,
        target: Option<String>,  // File path, tool name, etc.
    },

    /// Tool execution completed
    ActionCompleted {
        agent_id: String,
        action_type: String,
        result: ActionResult,
        duration_ms: u64,
    },

    /// Agent made a decision about next action
    DecisionMade {
        agent_id: String,
        decision: String,  // "refactor Auth module", "fix dependency conflict"
        affected_files: Vec<String>,
    },

    /// Agent captured important insight (error, contradiction, discovery)
    InsightCaptured {
        agent_id: String,
        insight: String,
        severity: InsightSeverity,
    },
}

/// Severity level for insights
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum InsightSeverity {
    Info,
    Warning,
    Critical,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_initiated_creation() {
        let event = AgentLoopEvent::ActionInitiated {
            agent_id: "test-agent".to_string(),
            action_type: "tool_call".to_string(),
            target: Some("read_file".to_string()),
        };

        match event {
            AgentLoopEvent::ActionInitiated { agent_id, .. } => {
                assert_eq!(agent_id, "test-agent");
            }
            _ => panic!("Wrong event type"),
        }
    }

    #[test]
    fn test_decision_made_creation() {
        let event = AgentLoopEvent::DecisionMade {
            agent_id: "test-agent".to_string(),
            decision: "refactor Auth module".to_string(),
            affected_files: vec!["src/auth/mod.rs".to_string()],
        };

        match event {
            AgentLoopEvent::DecisionMade { decision, .. } => {
                assert_eq!(decision, "refactor Auth module");
            }
            _ => panic!("Wrong event type"),
        }
    }
}
