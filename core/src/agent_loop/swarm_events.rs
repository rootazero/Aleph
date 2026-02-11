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
