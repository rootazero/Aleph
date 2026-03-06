//! ArenaEvent types with tier classification for EventBus integration.

use serde::{Deserialize, Serialize};

use super::types::{AgentId, ArenaId, ArtifactId};

// =============================================================================
// Arena Events
// =============================================================================

/// Events emitted during Arena lifecycle and agent collaboration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ArenaEvent {
    /// An agent published an artifact to its slot.
    ArtifactPublished {
        arena_id: ArenaId,
        agent_id: AgentId,
        artifact_id: ArtifactId,
    },
    /// An agent completed its assigned stage.
    StageCompleted {
        arena_id: ArenaId,
        agent_id: AgentId,
    },
    /// An agent updated its progress description.
    ProgressUpdated {
        arena_id: ArenaId,
        agent_id: AgentId,
        current: String,
    },
    /// A coordinator requested a merge of agent outputs.
    MergeRequested {
        arena_id: ArenaId,
        coordinator: AgentId,
    },
    /// A conflict was detected between agent outputs.
    ConflictDetected {
        arena_id: ArenaId,
        description: String,
    },
    /// The arena has begun settling (persisting facts, archiving artifacts).
    SettlingStarted {
        arena_id: ArenaId,
    },
}

impl ArenaEvent {
    /// Convert to a swarm ImportantEvent for bus publishing.
    ///
    /// Provides a summary view of the arena event suitable for team awareness
    /// context injection during the Think phase.
    pub fn to_swarm_event(
        &self,
        goal: &str,
        active_agents: Vec<String>,
        completed_steps: usize,
        total_steps: usize,
        latest_artifacts: Vec<String>,
    ) -> crate::agents::swarm::events::ImportantEvent {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        crate::agents::swarm::events::ImportantEvent::ArenaStateUpdate {
            arena_id: self.arena_id().to_string(),
            goal: goal.to_string(),
            active_agents,
            completed_steps,
            total_steps,
            latest_artifacts,
            timestamp,
        }
    }

    /// Returns the tier classification for this event.
    pub fn tier(&self) -> ArenaEventTier {
        match self {
            ArenaEvent::ArtifactPublished { .. } | ArenaEvent::StageCompleted { .. } => {
                ArenaEventTier::Critical
            }
            _ => ArenaEventTier::Important,
        }
    }

    /// Returns a reference to the arena ID contained in any variant.
    pub fn arena_id(&self) -> &ArenaId {
        match self {
            ArenaEvent::ArtifactPublished { arena_id, .. }
            | ArenaEvent::StageCompleted { arena_id, .. }
            | ArenaEvent::ProgressUpdated { arena_id, .. }
            | ArenaEvent::MergeRequested { arena_id, .. }
            | ArenaEvent::ConflictDetected { arena_id, .. }
            | ArenaEvent::SettlingStarted { arena_id } => arena_id,
        }
    }
}

// =============================================================================
// Event Tier Classification
// =============================================================================

/// Classification tier for arena events, controlling dispatch priority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArenaEventTier {
    /// Events that require immediate processing (artifact published, stage completed).
    Critical,
    /// Events that are important but can tolerate slight delay.
    Important,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arena_event_serialization_roundtrip() {
        let event = ArenaEvent::ArtifactPublished {
            arena_id: ArenaId::from_string("arena-1"),
            agent_id: "agent-alpha".to_string(),
            artifact_id: ArtifactId::from_string("artifact-42"),
        };

        let json = serde_json::to_string(&event).expect("serialize");
        let deserialized: ArenaEvent = serde_json::from_str(&json).expect("deserialize");

        // Verify the roundtrip preserves the arena_id
        assert_eq!(deserialized.arena_id().as_str(), "arena-1");

        // Verify the JSON contains the tagged type field
        assert!(json.contains("\"type\":\"artifact_published\""));
    }

    #[test]
    fn arena_event_tier_classification() {
        let arena_id = ArenaId::from_string("arena-1");
        let agent_id = "agent-alpha".to_string();

        // Critical events
        let published = ArenaEvent::ArtifactPublished {
            arena_id: arena_id.clone(),
            agent_id: agent_id.clone(),
            artifact_id: ArtifactId::from_string("art-1"),
        };
        assert_eq!(published.tier(), ArenaEventTier::Critical);

        let completed = ArenaEvent::StageCompleted {
            arena_id: arena_id.clone(),
            agent_id: agent_id.clone(),
        };
        assert_eq!(completed.tier(), ArenaEventTier::Critical);

        // Important events
        let progress = ArenaEvent::ProgressUpdated {
            arena_id: arena_id.clone(),
            agent_id: agent_id.clone(),
            current: "working on it".to_string(),
        };
        assert_eq!(progress.tier(), ArenaEventTier::Important);

        let merge = ArenaEvent::MergeRequested {
            arena_id: arena_id.clone(),
            coordinator: agent_id.clone(),
        };
        assert_eq!(merge.tier(), ArenaEventTier::Important);

        let conflict = ArenaEvent::ConflictDetected {
            arena_id: arena_id.clone(),
            description: "overlapping outputs".to_string(),
        };
        assert_eq!(conflict.tier(), ArenaEventTier::Important);

        let settling = ArenaEvent::SettlingStarted {
            arena_id: arena_id.clone(),
        };
        assert_eq!(settling.tier(), ArenaEventTier::Important);
    }

    #[test]
    fn arena_event_to_swarm_event_conversion() {
        let event = ArenaEvent::ProgressUpdated {
            arena_id: ArenaId::from_string("arena-42"),
            agent_id: "agent-alpha".to_string(),
            current: "analyzing code".to_string(),
        };

        let swarm_event = event.to_swarm_event(
            "Fix auth bugs",
            vec!["agent-alpha".to_string(), "agent-beta".to_string()],
            3,
            10,
            vec!["Text: art-1".to_string()],
        );

        match swarm_event {
            crate::agents::swarm::events::ImportantEvent::ArenaStateUpdate {
                arena_id,
                goal,
                active_agents,
                completed_steps,
                total_steps,
                latest_artifacts,
                timestamp,
            } => {
                assert_eq!(arena_id, "arena-42");
                assert_eq!(goal, "Fix auth bugs");
                assert_eq!(active_agents.len(), 2);
                assert_eq!(completed_steps, 3);
                assert_eq!(total_steps, 10);
                assert_eq!(latest_artifacts.len(), 1);
                assert!(timestamp > 0);
            }
            _ => panic!("Expected ArenaStateUpdate variant"),
        }
    }
}
