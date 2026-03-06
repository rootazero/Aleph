//! ArenaHandle — permission-guarded access to SharedArena.
//!
//! Each agent receives an `ArenaHandle` that wraps `Arc<RwLock<SharedArena>>`
//! and enforces permission checks before delegating to the underlying arena.

use crate::domain::Entity;
use crate::sync_primitives::{Arc, RwLock};

use super::arena::SharedArena;
use super::types::*;

/// Permission-guarded handle for an agent to interact with a SharedArena.
///
/// Each participant holds its own `ArenaHandle` which checks permissions
/// before forwarding operations to the shared arena behind `Arc<RwLock<>>`.
pub struct ArenaHandle {
    arena: Arc<RwLock<SharedArena>>,
    agent_id: AgentId,
    role: ParticipantRole,
    permissions: ArenaPermissions,
}

impl ArenaHandle {
    /// Create a new handle for the given agent with the specified role and permissions.
    pub fn new(
        arena: Arc<RwLock<SharedArena>>,
        agent_id: AgentId,
        role: ParticipantRole,
        permissions: ArenaPermissions,
    ) -> Self {
        Self {
            arena,
            agent_id,
            role,
            permissions,
        }
    }

    /// Returns the agent ID this handle belongs to.
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    /// Returns the participant role for this handle.
    pub fn role(&self) -> &ParticipantRole {
        &self.role
    }

    /// Put an artifact into this agent's own slot.
    ///
    /// Requires `can_write_own_slot` permission.
    pub fn put_artifact(&self, artifact: Artifact) -> Result<ArtifactId, String> {
        if !self.permissions.can_write_own_slot {
            return Err(format!(
                "Agent '{}' ({:?}) does not have write permission",
                self.agent_id, self.role
            ));
        }

        let artifact_id = artifact.id.clone();
        let mut arena = self.arena.write().unwrap_or_else(|e| e.into_inner());
        arena.put_artifact(&self.agent_id, artifact)?;
        Ok(artifact_id)
    }

    /// List artifacts for a target agent's slot.
    ///
    /// Reading own slot is always allowed. Reading other slots requires
    /// `can_read_other_slots` permission.
    pub fn list_artifacts(&self, target_agent_id: &AgentId) -> Result<Vec<Artifact>, String> {
        if *target_agent_id != self.agent_id && !self.permissions.can_read_other_slots {
            return Err(format!(
                "Agent '{}' ({:?}) cannot read other agents' slots",
                self.agent_id, self.role
            ));
        }

        let arena = self.arena.read().unwrap_or_else(|e| e.into_inner());
        Ok(arena.get_artifacts(target_agent_id).to_vec())
    }

    /// Report progress for this agent.
    pub fn report_progress(
        &self,
        current: Option<String>,
        completed: Option<usize>,
    ) -> Result<(), String> {
        let mut arena = self.arena.write().unwrap_or_else(|e| e.into_inner());
        arena.report_progress(&self.agent_id, current, completed);
        Ok(())
    }

    /// Get the current arena progress (cloned snapshot).
    pub fn get_progress(&self) -> ArenaProgress {
        let arena = self.arena.read().unwrap_or_else(|e| e.into_inner());
        arena.progress().clone()
    }

    /// Add a shared fact to the arena.
    ///
    /// Requires `can_write_shared_memory` permission.
    pub fn add_shared_fact(&self, fact: SharedFact) -> Result<(), String> {
        if !self.permissions.can_write_shared_memory {
            return Err(format!(
                "Agent '{}' ({:?}) does not have shared memory write permission",
                self.agent_id, self.role
            ));
        }

        let mut arena = self.arena.write().unwrap_or_else(|e| e.into_inner());
        arena.add_shared_fact(fact)
    }

    /// Create a snapshot of the arena state suitable for swarm context injection.
    ///
    /// Returns `(arena_id, goal, active_agents, completed_steps, total_steps, latest_artifacts)`.
    pub fn snapshot_for_context(
        &self,
    ) -> (String, String, Vec<String>, usize, usize, Vec<String>) {
        let arena = self.arena.read().unwrap_or_else(|e| e.into_inner());
        let arena_id = arena.id().to_string();
        let goal = arena.manifest().goal.clone();
        let active_agents: Vec<String> = arena.slots().keys().cloned().collect();
        let completed = arena.progress().completed_steps;
        let total = arena.progress().total_steps;
        let artifacts: Vec<String> = arena
            .slots()
            .values()
            .flat_map(|s| s.artifacts.iter())
            .map(|a| format!("{:?}: {}", a.kind, a.id))
            .take(5) // limit to 5 most recent
            .collect();
        (arena_id, goal, active_agents, completed, total, artifacts)
    }

    /// Begin the settling phase (coordinator only).
    ///
    /// Requires `can_merge` permission.
    pub fn begin_settling(&self) -> Result<(), String> {
        if !self.permissions.can_merge {
            return Err(format!(
                "Agent '{}' ({:?}) does not have merge permission (coordinator only)",
                self.agent_id, self.role
            ));
        }

        let mut arena = self.arena.write().unwrap_or_else(|e| e.into_inner());
        arena.begin_settling()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::collections::HashMap;

    /// Helper: create a manifest with coordinator + workers, activate arena, wrap in Arc<RwLock>.
    fn setup_arena(agents: &[(&str, ParticipantRole)]) -> Arc<RwLock<SharedArena>> {
        let participants: Vec<Participant> = agents
            .iter()
            .map(|(id, role)| Participant {
                agent_id: id.to_string(),
                role: *role,
                permissions: ArenaPermissions::from_role(*role),
            })
            .collect();

        let coordinator_id = agents[0].0.to_string();
        let manifest = ArenaManifest {
            goal: "Test goal".to_string(),
            strategy: CoordinationStrategy::Peer {
                coordinator: coordinator_id.clone(),
            },
            participants,
            created_by: coordinator_id,
            created_at: Utc::now(),
        };

        let mut arena = SharedArena::new(manifest);
        arena.activate().expect("activate should succeed");
        Arc::new(RwLock::new(arena))
    }

    fn make_handle(
        arena: &Arc<RwLock<SharedArena>>,
        agent_id: &str,
        role: ParticipantRole,
    ) -> ArenaHandle {
        ArenaHandle::new(
            Arc::clone(arena),
            agent_id.to_string(),
            role,
            ArenaPermissions::from_role(role),
        )
    }

    fn test_artifact() -> Artifact {
        Artifact {
            id: ArtifactId::new(),
            kind: ArtifactKind::Text,
            content: ArtifactContent::Inline("test content".to_string()),
            metadata: HashMap::new(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn worker_can_put_artifact_to_own_slot() {
        let arena = setup_arena(&[
            ("coordinator", ParticipantRole::Coordinator),
            ("worker-1", ParticipantRole::Worker),
        ]);
        let handle = make_handle(&arena, "worker-1", ParticipantRole::Worker);

        let result = handle.put_artifact(test_artifact());
        assert!(result.is_ok(), "Worker should be able to put artifact to own slot");

        let artifacts = handle.list_artifacts(&"worker-1".to_string()).unwrap();
        assert_eq!(artifacts.len(), 1);
    }

    #[test]
    fn observer_cannot_put_artifact() {
        let arena = setup_arena(&[
            ("coordinator", ParticipantRole::Coordinator),
            ("observer-1", ParticipantRole::Observer),
        ]);
        let handle = make_handle(&arena, "observer-1", ParticipantRole::Observer);

        let result = handle.put_artifact(test_artifact());
        assert!(result.is_err(), "Observer should not be able to put artifacts");
        assert!(result.unwrap_err().contains("does not have write permission"));
    }

    #[test]
    fn worker_can_read_other_slots() {
        let arena = setup_arena(&[
            ("coordinator", ParticipantRole::Coordinator),
            ("worker-1", ParticipantRole::Worker),
        ]);

        // Coordinator puts an artifact
        let coord_handle = make_handle(&arena, "coordinator", ParticipantRole::Coordinator);
        coord_handle.put_artifact(test_artifact()).unwrap();

        // Worker reads coordinator's slot
        let worker_handle = make_handle(&arena, "worker-1", ParticipantRole::Worker);
        let artifacts = worker_handle
            .list_artifacts(&"coordinator".to_string())
            .expect("Worker should be able to read other slots");
        assert_eq!(artifacts.len(), 1);
    }

    #[test]
    fn only_coordinator_can_begin_settling() {
        let arena = setup_arena(&[
            ("coordinator", ParticipantRole::Coordinator),
            ("worker-1", ParticipantRole::Worker),
        ]);

        // Worker cannot begin settling
        let worker_handle = make_handle(&arena, "worker-1", ParticipantRole::Worker);
        let result = worker_handle.begin_settling();
        assert!(result.is_err(), "Worker should not be able to begin settling");
        assert!(result.unwrap_err().contains("does not have merge permission"));

        // Coordinator can begin settling
        let coord_handle = make_handle(&arena, "coordinator", ParticipantRole::Coordinator);
        let result = coord_handle.begin_settling();
        assert!(result.is_ok(), "Coordinator should be able to begin settling");
    }

    #[test]
    fn snapshot_for_context_returns_correct_data() {
        let arena = setup_arena(&[
            ("coordinator", ParticipantRole::Coordinator),
            ("worker-1", ParticipantRole::Worker),
        ]);

        // Put an artifact via the coordinator handle
        let coord_handle = make_handle(&arena, "coordinator", ParticipantRole::Coordinator);
        coord_handle.put_artifact(test_artifact()).unwrap();

        // Report some progress
        coord_handle.report_progress(Some("analyzing".to_string()), Some(1)).unwrap();

        let worker_handle = make_handle(&arena, "worker-1", ParticipantRole::Worker);
        let (arena_id, goal, active_agents, completed, total, artifacts) =
            worker_handle.snapshot_for_context();

        assert!(!arena_id.is_empty());
        assert_eq!(goal, "Test goal");
        assert_eq!(active_agents.len(), 2);
        assert!(active_agents.contains(&"coordinator".to_string()));
        assert!(active_agents.contains(&"worker-1".to_string()));
        assert_eq!(completed, 1);
        assert_eq!(total, 0); // total_steps was never set
        assert_eq!(artifacts.len(), 1);
        assert!(artifacts[0].contains("Text"));
    }
}
