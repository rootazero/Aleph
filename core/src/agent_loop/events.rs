//! AgentLoop events for Swarm Intelligence integration
//!
//! These events represent key moments in the AgentLoop lifecycle that
//! are published to the SwarmCoordinator for collective intelligence.

use serde::{Deserialize, Serialize};

use super::decision::ActionResult;

/// Events emitted by AgentLoop for Swarm Intelligence
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentLoopEvent {
    /// Action initiated (before tool execution)
    ActionInitiated {
        agent_id: String,
        action_type: String,
        target: Option<String>,
    },

    /// Action completed (after tool execution)
    ActionCompleted {
        agent_id: String,
        action_type: String,
        result: ActionResult,
        duration_ms: u64,
    },

    /// Decision made (after thinking phase)
    DecisionMade {
        agent_id: String,
        decision: String,
        affected_files: Vec<String>,
    },

    /// Insight captured (from thinking or execution)
    InsightCaptured {
        agent_id: String,
        insight: String,
        severity: InsightSeverity,
    },
}

/// Severity level for insights
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
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
