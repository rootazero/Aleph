//! SharedArena aggregate root — the central entity managing multi-agent collaboration.

use std::collections::HashMap;

use chrono::Utc;

use super::types::*;
use crate::domain::{AggregateRoot, Entity};

/// The aggregate root for multi-agent collaboration.
///
/// A `SharedArena` manages participants, their working slots, shared facts,
/// and enforces lifecycle state transitions (Created → Active → Settling → Archived).
pub struct SharedArena {
    id: ArenaId,
    pub(crate) manifest: ArenaManifest,
    pub(crate) slots: HashMap<AgentId, ArenaSlot>,
    pub(crate) progress: ArenaProgress,
    pub(crate) status: ArenaStatus,
    shared_facts: Vec<SharedFact>,
}

impl Entity for SharedArena {
    type Id = ArenaId;

    fn id(&self) -> &Self::Id {
        &self.id
    }
}

impl AggregateRoot for SharedArena {}

impl SharedArena {
    /// Create a new arena from a manifest.
    ///
    /// Pre-populates one slot per participant and initializes progress tracking.
    pub fn new(manifest: ArenaManifest) -> Self {
        let mut slots = HashMap::new();
        let mut agent_progress = HashMap::new();

        for participant in &manifest.participants {
            slots.insert(
                participant.agent_id.clone(),
                ArenaSlot::new(participant.agent_id.clone()),
            );
            agent_progress.insert(participant.agent_id.clone(), AgentProgress::default());
        }

        Self {
            id: ArenaId::new(),
            manifest,
            slots,
            progress: ArenaProgress {
                total_steps: 0,
                completed_steps: 0,
                agent_progress,
            },
            status: ArenaStatus::Created,
            shared_facts: Vec::new(),
        }
    }

    /// Returns a reference to the arena manifest.
    pub fn manifest(&self) -> &ArenaManifest {
        &self.manifest
    }

    /// Returns the current arena status.
    pub fn status(&self) -> ArenaStatus {
        self.status
    }

    /// Returns a reference to the arena progress.
    pub fn progress(&self) -> &ArenaProgress {
        &self.progress
    }

    /// Returns a reference to the agent slots.
    pub fn slots(&self) -> &HashMap<AgentId, ArenaSlot> {
        &self.slots
    }

    // =========================================================================
    // State Transitions
    // =========================================================================

    /// Transition from Created to Active.
    pub fn activate(&mut self) -> Result<(), String> {
        if self.status != ArenaStatus::Created {
            return Err(format!(
                "Cannot activate arena: expected Created, got {:?}",
                self.status
            ));
        }
        self.status = ArenaStatus::Active;
        Ok(())
    }

    /// Transition from Active to Settling.
    pub fn begin_settling(&mut self) -> Result<(), String> {
        if self.status != ArenaStatus::Active {
            return Err(format!(
                "Cannot begin settling: expected Active, got {:?}",
                self.status
            ));
        }
        self.status = ArenaStatus::Settling;
        Ok(())
    }

    /// Transition from Settling to Archived.
    pub fn archive(&mut self) -> Result<(), String> {
        if self.status != ArenaStatus::Settling {
            return Err(format!(
                "Cannot archive arena: expected Settling, got {:?}",
                self.status
            ));
        }
        self.status = ArenaStatus::Archived;
        Ok(())
    }

    // =========================================================================
    // Artifact Operations
    // =========================================================================

    /// Add an artifact to the specified agent's slot, marking it as Working.
    pub fn put_artifact(
        &mut self,
        agent_id: &AgentId,
        artifact: Artifact,
    ) -> Result<(), String> {
        let slot = self
            .slots
            .get_mut(agent_id)
            .ok_or_else(|| format!("Unknown agent: {}", agent_id))?;
        slot.artifacts.push(artifact);
        slot.status = SlotStatus::Working;
        slot.updated_at = Utc::now();
        Ok(())
    }

    /// Get the artifacts for the specified agent.
    pub fn get_artifacts(&self, agent_id: &AgentId) -> &[Artifact] {
        self.slots
            .get(agent_id)
            .map(|s| s.artifacts.as_slice())
            .unwrap_or(&[])
    }

    // =========================================================================
    // Progress Tracking
    // =========================================================================

    /// Report progress for the specified agent.
    ///
    /// Updates the agent's current task description and completed count,
    /// and increments the arena-wide completed_steps.
    pub fn report_progress(
        &mut self,
        agent_id: &AgentId,
        current: Option<String>,
        completed: usize,
    ) {
        let agent_prog = self
            .progress
            .agent_progress
            .entry(agent_id.clone())
            .or_default();
        agent_prog.current = current;
        agent_prog.completed = completed;
        self.progress.completed_steps += 1;
    }

    // =========================================================================
    // Shared Facts
    // =========================================================================

    /// Add a shared fact to the arena's knowledge base.
    pub fn add_shared_fact(&mut self, fact: SharedFact) {
        self.shared_facts.push(fact);
    }

    /// Returns a reference to the shared facts.
    pub fn shared_facts(&self) -> &[SharedFact] {
        &self.shared_facts
    }

    /// Drain all shared facts, returning them and leaving the list empty.
    pub fn drain_shared_facts(&mut self) -> Vec<SharedFact> {
        std::mem::take(&mut self.shared_facts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a test manifest with the given participant agent IDs.
    fn test_manifest(agent_ids: &[&str]) -> ArenaManifest {
        let participants = agent_ids
            .iter()
            .enumerate()
            .map(|(i, id)| Participant {
                agent_id: id.to_string(),
                role: if i == 0 {
                    ParticipantRole::Coordinator
                } else {
                    ParticipantRole::Worker
                },
                permissions: ArenaPermissions::from_role(if i == 0 {
                    ParticipantRole::Coordinator
                } else {
                    ParticipantRole::Worker
                }),
            })
            .collect();

        ArenaManifest {
            goal: "Test goal".to_string(),
            strategy: CoordinationStrategy::Peer {
                coordinator: agent_ids[0].to_string(),
            },
            participants,
            created_by: agent_ids[0].to_string(),
            created_at: Utc::now(),
        }
    }

    fn test_artifact() -> Artifact {
        Artifact {
            id: ArtifactId::new(),
            kind: ArtifactKind::Text,
            content: ArtifactContent::Inline("test content".to_string()),
            metadata: Default::default(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn create_arena_initializes_slots_per_participant() {
        let manifest = test_manifest(&["agent-a", "agent-b"]);
        let arena = SharedArena::new(manifest);

        assert_eq!(arena.slots().len(), 2);
        assert!(arena.slots().contains_key("agent-a"));
        assert!(arena.slots().contains_key("agent-b"));
        assert_eq!(arena.status(), ArenaStatus::Created);
    }

    #[test]
    fn arena_implements_entity_and_aggregate_root() {
        let arena = SharedArena::new(test_manifest(&["agent-a"]));

        // Verify Entity trait
        let _id: &ArenaId = arena.id();

        // Verify AggregateRoot trait bound (compile-time check)
        fn assert_aggregate_root<T: AggregateRoot>(_: &T) {}
        assert_aggregate_root(&arena);
    }

    #[test]
    fn activate_transitions_from_created() {
        let mut arena = SharedArena::new(test_manifest(&["agent-a"]));
        assert_eq!(arena.status(), ArenaStatus::Created);

        assert!(arena.activate().is_ok());
        assert_eq!(arena.status(), ArenaStatus::Active);
    }

    #[test]
    fn activate_fails_if_not_created() {
        let mut arena = SharedArena::new(test_manifest(&["agent-a"]));
        arena.activate().unwrap();

        // Active → Active should fail
        let result = arena.activate();
        assert!(result.is_err());
    }

    #[test]
    fn put_artifact_to_own_slot() {
        let mut arena = SharedArena::new(test_manifest(&["agent-a"]));
        let artifact = test_artifact();

        assert!(arena.put_artifact(&"agent-a".to_string(), artifact).is_ok());
        assert_eq!(arena.get_artifacts(&"agent-a".to_string()).len(), 1);
        assert_eq!(
            arena.slots().get("agent-a").unwrap().status,
            SlotStatus::Working
        );
    }

    #[test]
    fn put_artifact_fails_for_unknown_agent() {
        let mut arena = SharedArena::new(test_manifest(&["agent-a"]));
        let artifact = test_artifact();

        let result = arena.put_artifact(&"unknown-agent".to_string(), artifact);
        assert!(result.is_err());
    }

    #[test]
    fn begin_settling_transitions_from_active() {
        let mut arena = SharedArena::new(test_manifest(&["agent-a"]));
        arena.activate().unwrap();

        assert!(arena.begin_settling().is_ok());
        assert_eq!(arena.status(), ArenaStatus::Settling);
    }

    #[test]
    fn archive_transitions_from_settling() {
        let mut arena = SharedArena::new(test_manifest(&["agent-a"]));
        arena.activate().unwrap();
        arena.begin_settling().unwrap();

        assert!(arena.archive().is_ok());
        assert_eq!(arena.status(), ArenaStatus::Archived);
    }

    #[test]
    fn shared_facts_accumulate_and_drain_empties_list() {
        let mut arena = SharedArena::new(test_manifest(&["agent-a"]));

        let fact = SharedFact {
            content: "The sky is blue".to_string(),
            source_agent: "agent-a".to_string(),
            confidence: 0.95,
            tags: vec!["observation".to_string()],
            created_at: Utc::now(),
        };

        arena.add_shared_fact(fact);
        assert_eq!(arena.shared_facts().len(), 1);
        assert_eq!(arena.shared_facts()[0].content, "The sky is blue");

        let drained = arena.drain_shared_facts();
        assert_eq!(drained.len(), 1);
        assert!(arena.shared_facts().is_empty());
    }
}
