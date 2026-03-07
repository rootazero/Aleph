//! Core domain types for the SharedArena multi-agent collaboration system.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::domain::ValueObject;

// =============================================================================
// Identity Types
// =============================================================================

/// Unique identifier for an Arena instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArenaId(String);

impl ArenaId {
    /// Create a new random ArenaId.
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    /// Create from an existing string.
    pub fn from_string(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Borrow the inner string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ArenaId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ArenaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for an Artifact.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArtifactId(String);

impl ArtifactId {
    /// Create a new random ArtifactId.
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    /// Create from an existing string.
    pub fn from_string(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Borrow the inner string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ArtifactId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for ArtifactId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Agent identifier — a plain string alias used throughout the codebase.
pub type AgentId = String;

// =============================================================================
// Arena Status (ValueObject)
// =============================================================================

/// Lifecycle status of an Arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ArenaStatus {
    /// Arena has been created but not yet started.
    Created,
    /// Arena is actively running with agents collaborating.
    Active,
    /// Arena is settling — persisting facts and archiving artifacts.
    Settling,
    /// Arena has been archived and is read-only.
    Archived,
}

impl ValueObject for ArenaStatus {}

// =============================================================================
// Coordination Strategy (ValueObject)
// =============================================================================

/// How agents coordinate within an Arena.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoordinationStrategy {
    /// Peer-based coordination with a designated coordinator agent.
    Peer {
        /// The agent responsible for coordination decisions.
        coordinator: AgentId,
    },
    /// Pipeline coordination with ordered stages.
    Pipeline {
        /// The ordered stages of the pipeline.
        stages: Vec<StageSpec>,
    },
}

impl ValueObject for CoordinationStrategy {}

/// A single stage in a pipeline coordination strategy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageSpec {
    /// The agent assigned to this stage.
    pub agent_id: AgentId,
    /// Human-readable description of this stage's purpose.
    pub description: String,
    /// Agents that must complete before this stage can begin.
    pub depends_on: Vec<AgentId>,
}

// =============================================================================
// Participants
// =============================================================================

/// Role a participant plays within an Arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ParticipantRole {
    /// Coordinates work across agents, can merge results.
    Coordinator,
    /// Performs assigned work within its slot.
    Worker,
    /// Read-only access to arena state.
    Observer,
}

/// Permission set for an arena participant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArenaPermissions {
    /// Whether the participant can write to its own slot.
    pub can_write_own_slot: bool,
    /// Whether the participant can read other agents' slots.
    pub can_read_other_slots: bool,
    /// Whether the participant can write to shared memory.
    pub can_write_shared_memory: bool,
    /// Whether the participant can merge artifacts across slots.
    pub can_merge: bool,
}

impl ArenaPermissions {
    /// Create permissions appropriate for the given role.
    pub fn from_role(role: ParticipantRole) -> Self {
        match role {
            ParticipantRole::Coordinator => Self {
                can_write_own_slot: true,
                can_read_other_slots: true,
                can_write_shared_memory: true,
                can_merge: true,
            },
            ParticipantRole::Worker => Self {
                can_write_own_slot: true,
                can_read_other_slots: true,
                can_write_shared_memory: true,
                can_merge: false,
            },
            ParticipantRole::Observer => Self {
                can_write_own_slot: false,
                can_read_other_slots: true,
                can_write_shared_memory: false,
                can_merge: false,
            },
        }
    }
}

/// A participant in an Arena.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Participant {
    /// The agent's identifier.
    pub agent_id: AgentId,
    /// The role this agent plays.
    pub role: ParticipantRole,
    /// Permissions granted to this agent.
    pub permissions: ArenaPermissions,
}

// =============================================================================
// Manifest
// =============================================================================

/// The initial configuration / manifest for creating an Arena.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArenaManifest {
    /// The high-level goal this arena is trying to achieve.
    pub goal: String,
    /// How agents should coordinate.
    pub strategy: CoordinationStrategy,
    /// The set of participants.
    pub participants: Vec<Participant>,
    /// The agent that created this arena.
    pub created_by: AgentId,
    /// When this arena was created.
    pub created_at: DateTime<Utc>,
}

impl ArenaManifest {
    /// Build a manifest from raw parameters.
    ///
    /// Centralizes the strategy parsing + participant building logic used by
    /// AlephTool, RPC handler, and CollaborativeExecutor.
    ///
    /// - `strategy_str`: "peer" or "pipeline"
    /// - `agent_ids`: participant agent IDs (first is coordinator for peer strategy)
    /// - `coordinator`: explicit coordinator override (defaults to first agent)
    /// - `stages`: explicit pipeline stages (if None for pipeline, auto-generated from agent_ids)
    pub fn build(
        goal: String,
        strategy_str: &str,
        agent_ids: &[String],
        coordinator: Option<String>,
        stages: Option<Vec<StageSpec>>,
    ) -> Result<Self, String> {
        if agent_ids.is_empty() {
            return Err("At least one participant is required".to_string());
        }

        let coord = coordinator.unwrap_or_else(|| agent_ids[0].clone());

        let strategy = match strategy_str {
            "peer" => CoordinationStrategy::Peer {
                coordinator: coord.clone(),
            },
            "pipeline" => {
                let stages = stages.unwrap_or_else(|| {
                    agent_ids
                        .iter()
                        .enumerate()
                        .map(|(i, agent_id)| StageSpec {
                            agent_id: agent_id.clone(),
                            description: format!("Stage {}", i + 1),
                            depends_on: if i > 0 {
                                vec![agent_ids[i - 1].clone()]
                            } else {
                                vec![]
                            },
                        })
                        .collect()
                });
                CoordinationStrategy::Pipeline { stages }
            }
            other => {
                return Err(format!(
                    "Unknown strategy '{}': expected 'peer' or 'pipeline'",
                    other
                ));
            }
        };

        let participants: Vec<Participant> = agent_ids
            .iter()
            .map(|id| {
                let role = if *id == coord {
                    ParticipantRole::Coordinator
                } else {
                    ParticipantRole::Worker
                };
                Participant {
                    agent_id: id.clone(),
                    role,
                    permissions: ArenaPermissions::from_role(role),
                }
            })
            .collect();

        Ok(Self {
            goal,
            strategy,
            participants,
            created_by: coord,
            created_at: Utc::now(),
        })
    }
}

// =============================================================================
// Artifacts
// =============================================================================

/// The kind of content an artifact represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ArtifactKind {
    /// Plain text content.
    Text,
    /// Source code.
    Code,
    /// A file reference.
    File,
    /// Structured data (JSON, TOML, etc.).
    StructuredData,
}

impl ValueObject for ArtifactKind {}

/// How artifact content is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactContent {
    /// Content is stored inline as a string.
    Inline(String),
    /// Content is referenced by a file path.
    Reference(PathBuf),
}

impl ValueObject for ArtifactContent {}

/// A concrete output produced by an agent within a slot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    /// Unique artifact identifier.
    pub id: ArtifactId,
    /// The kind of artifact.
    pub kind: ArtifactKind,
    /// The artifact's content.
    pub content: ArtifactContent,
    /// Arbitrary metadata key-value pairs.
    pub metadata: HashMap<String, Value>,
    /// When this artifact was created.
    pub created_at: DateTime<Utc>,
}

// =============================================================================
// Slots
// =============================================================================

/// Status of an agent's slot within the Arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SlotStatus {
    /// Slot is idle, no work in progress.
    Idle,
    /// Agent is actively working.
    Working,
    /// Agent has completed its work.
    Done,
    /// Agent encountered a failure.
    Failed,
}

impl ValueObject for SlotStatus {}

/// An agent's workspace slot within the Arena.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArenaSlot {
    /// The agent that owns this slot.
    pub agent_id: AgentId,
    /// Artifacts produced by this agent.
    pub artifacts: Vec<Artifact>,
    /// Current status of this slot.
    pub status: SlotStatus,
    /// When this slot was last updated.
    pub updated_at: DateTime<Utc>,
}

impl ArenaSlot {
    /// Create a new idle slot for the given agent.
    pub fn new(agent_id: AgentId) -> Self {
        Self {
            agent_id,
            artifacts: Vec::new(),
            status: SlotStatus::Idle,
            updated_at: Utc::now(),
        }
    }
}

// =============================================================================
// Progress
// =============================================================================

/// Progress tracking for a single agent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentProgress {
    /// Number of tasks assigned to this agent.
    pub assigned: usize,
    /// Number of tasks completed.
    pub completed: usize,
    /// Description of current work, if any.
    pub current: Option<String>,
}

/// Overall progress of an Arena.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ArenaProgress {
    /// Total number of steps in the arena.
    pub total_steps: usize,
    /// Number of completed steps.
    pub completed_steps: usize,
    /// Per-agent progress breakdown.
    pub agent_progress: HashMap<AgentId, AgentProgress>,
}

// =============================================================================
// Shared Facts
// =============================================================================

/// A fact contributed to the arena's shared knowledge base.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedFact {
    /// The content of the fact.
    pub content: String,
    /// Which agent contributed this fact.
    pub source_agent: AgentId,
    /// Confidence score in [0.0, 1.0].
    pub confidence: f32,
    /// Categorization tags.
    pub tags: Vec<String>,
    /// When this fact was created.
    pub created_at: DateTime<Utc>,
}

// =============================================================================
// Settle Report
// =============================================================================

/// Report generated when an Arena settles (transitions to Archived).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettleReport {
    /// The arena that was settled.
    pub arena_id: ArenaId,
    /// Number of facts persisted to long-term memory.
    pub facts_persisted: usize,
    /// Number of artifacts archived.
    pub artifacts_archived: usize,
    /// Number of events cleared.
    pub events_cleared: usize,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arena_id_display_and_eq() {
        let id1 = ArenaId::new();
        let id2 = ArenaId::new();
        // Two new IDs should be unique
        assert_ne!(id1, id2);

        // Clone should be equal
        let id1_clone = id1.clone();
        assert_eq!(id1, id1_clone);

        // Display should not be empty
        let display = format!("{}", id1);
        assert!(!display.is_empty());
    }

    #[test]
    fn arena_status_is_value_object() {
        let status = ArenaStatus::Active;
        let cloned = status.clone();
        assert_eq!(status, cloned);

        // Different variants are not equal
        assert_ne!(ArenaStatus::Created, ArenaStatus::Active);
        assert_ne!(ArenaStatus::Settling, ArenaStatus::Archived);
    }

    #[test]
    fn participant_role_default_permissions() {
        // Coordinator can merge
        let coord_perms = ArenaPermissions::from_role(ParticipantRole::Coordinator);
        assert!(coord_perms.can_write_own_slot);
        assert!(coord_perms.can_read_other_slots);
        assert!(coord_perms.can_write_shared_memory);
        assert!(coord_perms.can_merge);

        // Worker cannot merge
        let worker_perms = ArenaPermissions::from_role(ParticipantRole::Worker);
        assert!(worker_perms.can_write_own_slot);
        assert!(worker_perms.can_read_other_slots);
        assert!(worker_perms.can_write_shared_memory);
        assert!(!worker_perms.can_merge);

        // Observer is read-only
        let observer_perms = ArenaPermissions::from_role(ParticipantRole::Observer);
        assert!(!observer_perms.can_write_own_slot);
        assert!(observer_perms.can_read_other_slots);
        assert!(!observer_perms.can_write_shared_memory);
        assert!(!observer_perms.can_merge);
    }

    #[test]
    fn coordination_strategy_clone_eq() {
        let strategy = CoordinationStrategy::Peer {
            coordinator: "agent-alpha".to_string(),
        };
        let cloned = strategy.clone();
        assert_eq!(strategy, cloned);
    }

    #[test]
    fn artifact_content_variants() {
        let inline = ArtifactContent::Inline("hello world".to_string());
        let reference = ArtifactContent::Reference(PathBuf::from("/tmp/output.txt"));

        // Pattern matching works on both variants
        match &inline {
            ArtifactContent::Inline(s) => assert_eq!(s, "hello world"),
            ArtifactContent::Reference(_) => panic!("expected Inline"),
        }

        match &reference {
            ArtifactContent::Reference(p) => assert_eq!(p, &PathBuf::from("/tmp/output.txt")),
            ArtifactContent::Inline(_) => panic!("expected Reference"),
        }
    }
}
