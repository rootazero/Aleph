//! ArenaManager — lifecycle management for SharedArena instances.
//!
//! Responsible for creating arenas, distributing handles to participants,
//! tracking active arenas per agent, and settling arenas when collaboration ends.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::domain::Entity;
use crate::sync_primitives::{Arc, RwLock};

use super::aggregate::SharedArena;
use super::handle::ArenaHandle;
use super::types::*;

/// Manages SharedArena creation, handle distribution, and settling lifecycle.
pub struct ArenaManager {
    arenas: HashMap<ArenaId, Arc<RwLock<SharedArena>>>,
}

impl ArenaManager {
    /// Create an empty ArenaManager.
    pub fn new() -> Self {
        Self {
            arenas: HashMap::new(),
        }
    }

    /// Create a new arena from the given manifest.
    ///
    /// Creates the SharedArena, activates it, wraps it in `Arc<RwLock>`,
    /// and returns handles for all participants.
    pub fn create_arena(
        &mut self,
        manifest: ArenaManifest,
    ) -> Result<(ArenaId, HashMap<AgentId, ArenaHandle>), String> {
        let mut arena = SharedArena::new(manifest.clone());
        arena.activate()?;

        let arena_id = arena.id().clone();
        let shared = Arc::new(RwLock::new(arena));

        let mut handles = HashMap::new();
        for participant in &manifest.participants {
            let handle = ArenaHandle::new(
                Arc::clone(&shared),
                participant.agent_id.clone(),
                participant.role,
                participant.permissions.clone(),
            );
            handles.insert(participant.agent_id.clone(), handle);
        }

        self.arenas.insert(arena_id.clone(), shared);
        Ok((arena_id, handles))
    }

    /// Get a handle for an existing participant in a specific arena.
    ///
    /// Looks up the arena, finds the participant in the manifest,
    /// and creates a handle with the correct role and permissions.
    pub fn get_handle(
        &self,
        arena_id: &ArenaId,
        agent_id: &AgentId,
    ) -> Result<ArenaHandle, String> {
        let shared = self
            .arenas
            .get(arena_id)
            .ok_or_else(|| format!("Arena not found: {}", arena_id))?;

        let arena = shared.read().unwrap_or_else(|e| e.into_inner());
        let participant = arena
            .manifest()
            .participants
            .iter()
            .find(|p| p.agent_id == *agent_id)
            .ok_or_else(|| {
                format!(
                    "Agent '{}' is not a participant in arena {}",
                    agent_id, arena_id
                )
            })?;

        let handle = ArenaHandle::new(
            Arc::clone(shared),
            participant.agent_id.clone(),
            participant.role,
            participant.permissions.clone(),
        );
        Ok(handle)
    }

    /// Returns arena IDs where the given agent is a participant and the arena is not Archived.
    pub fn active_arenas_for(&self, agent_id: &AgentId) -> Vec<ArenaId> {
        self.arenas
            .iter()
            .filter_map(|(arena_id, shared)| {
                let arena = shared.read().unwrap_or_else(|e| e.into_inner());
                if arena.status() == ArenaStatus::Archived {
                    return None;
                }
                let is_participant = arena
                    .manifest()
                    .participants
                    .iter()
                    .any(|p| p.agent_id == *agent_id);
                if is_participant {
                    Some(arena_id.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Query arena state as a JSON snapshot (for RPC handlers).
    ///
    /// Returns `None` if the arena does not exist.
    pub fn query_arena(&self, arena_id: &ArenaId) -> Option<Value> {
        let shared = self.arenas.get(arena_id)?;
        let arena = shared.read().unwrap_or_else(|e| e.into_inner());

        let slot_summaries: Vec<Value> = arena
            .slots()
            .values()
            .map(|slot| {
                json!({
                    "agent_id": slot.agent_id,
                    "status": format!("{:?}", slot.status),
                    "artifact_count": slot.artifacts.len(),
                    "updated_at": slot.updated_at.to_rfc3339(),
                })
            })
            .collect();

        let progress = arena.progress();

        Some(json!({
            "arena_id": arena.id().as_str(),
            "goal": arena.manifest().goal,
            "status": format!("{:?}", arena.status()),
            "progress": {
                "total_steps": progress.total_steps,
                "completed_steps": progress.completed_steps,
            },
            "slots": slot_summaries,
        }))
    }

    /// Settle an arena: drain shared facts, count artifacts, archive, and return a report.
    ///
    /// The caller is responsible for persisting the returned facts to MemoryStore.
    pub fn settle_with_facts(
        &mut self,
        arena_id: &ArenaId,
    ) -> Result<(SettleReport, Vec<SharedFact>), String> {
        let shared = self
            .arenas
            .get(arena_id)
            .ok_or_else(|| format!("Arena not found: {}", arena_id))?;

        let mut arena = shared.write().unwrap_or_else(|e| e.into_inner());

        // Ensure arena is in Settling state before proceeding
        match arena.status() {
            ArenaStatus::Active => {
                arena.begin_settling()?;
            }
            ArenaStatus::Settling => { /* already settling, proceed */ }
            other => {
                return Err(format!("Cannot settle arena in {:?} state", other));
            }
        }

        // Drain shared facts before archiving
        let facts = arena.drain_shared_facts();

        // Count artifacts across all slots
        let artifacts_archived: usize = arena.slots().values().map(|s| s.artifacts.len()).sum();

        // Transition to Archived
        arena.archive()?;

        let report = SettleReport {
            arena_id: arena_id.clone(),
            facts_persisted: facts.len(),
            artifacts_archived,
            events_cleared: 0,
        };

        Ok((report, facts))
    }
}

impl Default for ArenaManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

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

    #[test]
    fn create_arena_returns_handles_for_all_participants() {
        let mut manager = ArenaManager::new();
        let manifest = test_manifest(&["agent-a", "agent-b"]);

        let (arena_id, handles) = manager.create_arena(manifest).unwrap();

        assert!(!arena_id.as_str().is_empty());
        assert_eq!(handles.len(), 2);
        assert!(handles.contains_key("agent-a"));
        assert!(handles.contains_key("agent-b"));
    }

    #[test]
    fn get_handle_for_existing_participant() {
        let mut manager = ArenaManager::new();
        let manifest = test_manifest(&["agent-a", "agent-b"]);
        let (arena_id, _) = manager.create_arena(manifest).unwrap();

        let handle = manager
            .get_handle(&arena_id, &"agent-b".to_string())
            .unwrap();

        assert_eq!(handle.agent_id(), "agent-b");
        assert_eq!(*handle.role(), ParticipantRole::Worker);
    }

    #[test]
    fn get_handle_fails_for_nonexistent_arena() {
        let manager = ArenaManager::new();
        let fake_id = ArenaId::from_string("nonexistent");

        let result = manager.get_handle(&fake_id, &"agent-a".to_string());
        match result {
            Err(msg) => assert!(msg.contains("Arena not found"), "Unexpected error: {}", msg),
            Ok(_) => panic!("Expected error for nonexistent arena"),
        }
    }

    #[test]
    fn active_arenas_for_agent() {
        let mut manager = ArenaManager::new();

        let manifest = test_manifest(&["agent-a", "agent-b"]);
        let (arena_id, _) = manager.create_arena(manifest).unwrap();

        // agent-a is a participant
        let active = manager.active_arenas_for(&"agent-a".to_string());
        assert_eq!(active.len(), 1);
        assert_eq!(active[0], arena_id);

        // unknown agent is not a participant
        let active = manager.active_arenas_for(&"unknown".to_string());
        assert!(active.is_empty());
    }

    #[test]
    fn settle_drains_facts_and_archives() {
        let mut manager = ArenaManager::new();
        let manifest = test_manifest(&["agent-a", "agent-b"]);
        let (arena_id, handles) = manager.create_arena(manifest).unwrap();

        // Add a fact via a handle
        let handle_a = handles.get("agent-a").unwrap();
        let fact = SharedFact {
            content: "Important discovery".to_string(),
            source_agent: "agent-a".to_string(),
            confidence: 0.9,
            tags: vec!["test".to_string()],
            created_at: Utc::now(),
        };
        handle_a.add_shared_fact(fact).unwrap();

        // Settle the arena (settle_with_facts handles Active → Settling internally)
        let (report, facts) = manager.settle_with_facts(&arena_id).unwrap();

        assert_eq!(report.facts_persisted, 1);
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].content, "Important discovery");

        // Arena should now be Archived — no longer active
        let active = manager.active_arenas_for(&"agent-a".to_string());
        assert!(active.is_empty());
    }
}
