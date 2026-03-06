//! Event types for swarm communication
//!
//! Events are organized into three tiers based on priority and delivery method:
//! - Tier 1 (Critical): Interrupt-driven, immediate delivery
//! - Tier 2 (Important): Passive injection before Think phase
//! - Tier 3 (Info): On-demand query via tools

use serde::{Deserialize, Serialize};
use std::fmt;

/// Top-level event wrapper with tier classification
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "tier", content = "event")]
pub enum AgentEvent {
    /// Tier 1: Critical events requiring immediate attention
    #[serde(rename = "critical")]
    Critical(CriticalEvent),

    /// Tier 2: Important events for passive injection
    #[serde(rename = "important")]
    Important(ImportantEvent),

    /// Tier 3: Informational events for on-demand query
    #[serde(rename = "info")]
    Info(InfoEvent),
}

impl AgentEvent {
    /// Get the tier of this event
    pub fn tier(&self) -> EventTier {
        match self {
            Self::Critical(_) => EventTier::Critical,
            Self::Important(_) => EventTier::Important,
            Self::Info(_) => EventTier::Info,
        }
    }

    /// Get timestamp of this event
    pub fn timestamp(&self) -> u64 {
        match self {
            Self::Critical(e) => e.timestamp(),
            Self::Important(e) => e.timestamp(),
            Self::Info(e) => e.timestamp(),
        }
    }
}

/// Event tier classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventTier {
    /// Interrupt-driven delivery
    Critical,
    /// Passive injection delivery
    Important,
    /// On-demand query delivery
    Info,
}

impl fmt::Display for EventTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Critical => write!(f, "critical"),
            Self::Important => write!(f, "important"),
            Self::Info => write!(f, "info"),
        }
    }
}

/// Critical events requiring immediate attention
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CriticalEvent {
    /// Bug root cause has been identified
    BugRootCauseFound {
        location: String,
        description: String,
        #[serde(default = "current_timestamp")]
        timestamp: u64,
    },

    /// Task has been cancelled
    TaskCancelled {
        task_id: String,
        reason: String,
        #[serde(default = "current_timestamp")]
        timestamp: u64,
    },

    /// Global failure affecting all agents
    GlobalFailure {
        error: String,
        #[serde(default = "current_timestamp")]
        timestamp: u64,
    },

    /// Error detected during execution
    ErrorDetected {
        agent_id: String,
        error_message: String,
        #[serde(default = "current_timestamp")]
        timestamp: u64,
    },
}

impl CriticalEvent {
    /// Get timestamp of this event
    pub fn timestamp(&self) -> u64 {
        match self {
            Self::BugRootCauseFound { timestamp, .. } => *timestamp,
            Self::TaskCancelled { timestamp, .. } => *timestamp,
            Self::GlobalFailure { timestamp, .. } => *timestamp,
            Self::ErrorDetected { timestamp, .. } => *timestamp,
        }
    }
}

/// Important events for passive injection (semantically aggregated)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImportantEvent {
    /// Multiple agents working in same area (hotspot detected)
    Hotspot {
        area: String,
        agent_count: usize,
        activity: String,
        #[serde(default = "current_timestamp")]
        timestamp: u64,
    },

    /// High-confidence insight confirmed by multiple sources
    ConfirmedInsight {
        symbol: String,
        confidence: f32,
        sources: Vec<String>,
        #[serde(default = "current_timestamp")]
        timestamp: u64,
    },

    /// LLM-generated swarm state summary
    SwarmStateSummary {
        summary: String,
        #[serde(default = "current_timestamp")]
        timestamp: u64,
    },

    /// Tool executed by an agent
    ToolExecuted {
        agent_id: String,
        tool_name: String,
        result: String,
        duration_ms: u64,
        #[serde(default = "current_timestamp")]
        timestamp: u64,
    },

    /// Decision broadcast by an agent
    DecisionBroadcast {
        agent_id: String,
        decision: String,
        affected_files: Vec<String>,
        #[serde(default = "current_timestamp")]
        timestamp: u64,
    },

    /// Arena state update for team awareness
    ArenaStateUpdate {
        arena_id: String,
        goal: String,
        active_agents: Vec<String>,
        completed_steps: usize,
        total_steps: usize,
        /// Brief descriptions of recent artifacts
        latest_artifacts: Vec<String>,
        #[serde(default = "current_timestamp")]
        timestamp: u64,
    },
}

impl ImportantEvent {
    /// Get timestamp of this event
    pub fn timestamp(&self) -> u64 {
        match self {
            Self::Hotspot { timestamp, .. } => *timestamp,
            Self::ConfirmedInsight { timestamp, .. } => *timestamp,
            Self::SwarmStateSummary { timestamp, .. } => *timestamp,
            Self::ToolExecuted { timestamp, .. } => *timestamp,
            Self::DecisionBroadcast { timestamp, .. } => *timestamp,
            Self::ArenaStateUpdate { timestamp, .. } => *timestamp,
        }
    }
}

/// Informational events for on-demand query
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InfoEvent {
    /// Tool execution event
    ToolExecuted {
        agent_id: String,
        tool: String,
        path: Option<String>,
        #[serde(default = "current_timestamp")]
        timestamp: u64,
    },

    /// File access event
    FileAccessed {
        agent_id: String,
        path: String,
        operation: FileOperation,
        #[serde(default = "current_timestamp")]
        timestamp: u64,
    },

    /// Symbol search event
    SymbolSearched {
        agent_id: String,
        symbol: String,
        context: Option<String>,
        #[serde(default = "current_timestamp")]
        timestamp: u64,
    },

    /// Action started event
    ActionStarted {
        agent_id: String,
        action_type: String,
        target: Option<String>,
        #[serde(default = "current_timestamp")]
        timestamp: u64,
    },

    /// Insight captured event
    InsightCaptured {
        agent_id: String,
        insight: String,
        #[serde(default = "current_timestamp")]
        timestamp: u64,
    },
}

impl InfoEvent {
    /// Get timestamp of this event
    pub fn timestamp(&self) -> u64 {
        match self {
            Self::ToolExecuted { timestamp, .. } => *timestamp,
            Self::FileAccessed { timestamp, .. } => *timestamp,
            Self::SymbolSearched { timestamp, .. } => *timestamp,
            Self::ActionStarted { timestamp, .. } => *timestamp,
            Self::InsightCaptured { timestamp, .. } => *timestamp,
        }
    }
}

/// File operation types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileOperation {
    Read,
    Write,
    Delete,
    List,
}

/// Helper function to get current timestamp
fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_tier_classification() {
        let critical = AgentEvent::Critical(CriticalEvent::GlobalFailure {
            error: "test".into(),
            timestamp: 0,
        });
        assert_eq!(critical.tier(), EventTier::Critical);

        let important = AgentEvent::Important(ImportantEvent::Hotspot {
            area: "auth/".into(),
            agent_count: 3,
            activity: "analysis".into(),
            timestamp: 0,
        });
        assert_eq!(important.tier(), EventTier::Important);

        let info = AgentEvent::Info(InfoEvent::FileAccessed {
            agent_id: "agent_1".into(),
            path: "/test".into(),
            operation: FileOperation::Read,
            timestamp: 0,
        });
        assert_eq!(info.tier(), EventTier::Info);
    }

    #[test]
    fn test_event_serialization() {
        let event = AgentEvent::Important(ImportantEvent::SwarmStateSummary {
            summary: "Team analyzing auth module".into(),
            timestamp: 1234567890,
        });

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: AgentEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(event.tier(), deserialized.tier());
    }

    #[test]
    fn test_timestamp_extraction() {
        let event = AgentEvent::Info(InfoEvent::ToolExecuted {
            agent_id: "agent_1".into(),
            tool: "grep".into(),
            path: Some("/src".into()),
            timestamp: 9999,
        });

        assert_eq!(event.timestamp(), 9999);
    }

    #[test]
    fn test_arena_state_update_serialization() {
        let event = AgentEvent::Important(ImportantEvent::ArenaStateUpdate {
            arena_id: "arena-123".into(),
            goal: "Fix auth bugs".into(),
            active_agents: vec!["agent-a".into(), "agent-b".into()],
            completed_steps: 3,
            total_steps: 10,
            latest_artifacts: vec!["Text: art-1".into(), "Code: art-2".into()],
            timestamp: 1234567890,
        });

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: AgentEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(event.tier(), deserialized.tier());
        assert_eq!(deserialized.timestamp(), 1234567890);

        // Verify JSON contains expected fields
        assert!(json.contains("\"arena_state_update\""));
        assert!(json.contains("arena-123"));
        assert!(json.contains("Fix auth bugs"));
    }

    #[test]
    fn test_arena_state_update_timestamp() {
        let event = ImportantEvent::ArenaStateUpdate {
            arena_id: "arena-1".into(),
            goal: "test".into(),
            active_agents: vec![],
            completed_steps: 0,
            total_steps: 0,
            latest_artifacts: vec![],
            timestamp: 42,
        };

        assert_eq!(event.timestamp(), 42);
    }
}
