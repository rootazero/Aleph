//! Arena Tools — AlephTool implementations for SharedArena interaction.
//!
//! Provides three tools for agents to interact with the SharedArena system:
//! - `arena_create` — Create a new collaboration arena
//! - `arena_query` — Query arena status and slot details
//! - `arena_settle` — Settle an arena (archive and persist facts)

use async_trait::async_trait;
use chrono::Utc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::arena::{
    ArenaId, ArenaManager, ArenaManifest, ArenaPermissions, CoordinationStrategy, Participant,
    ParticipantRole, StageSpec,
};
use crate::error::Result;
use crate::sync_primitives::{Arc, RwLock};
use crate::tools::AlephTool;

// =============================================================================
// ArenaCreateTool
// =============================================================================

/// Arguments for creating a new collaboration arena.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ArenaCreateArgs {
    /// Goal description for the collaboration
    pub goal: String,
    /// Coordination strategy: "peer" or "pipeline"
    pub strategy: String,
    /// Agent IDs to participate
    pub participants: Vec<String>,
    /// For peer strategy: which agent is coordinator (default: first participant)
    #[serde(default)]
    pub coordinator: Option<String>,
}

/// Output from arena creation.
#[derive(Debug, Clone, Serialize)]
pub struct ArenaCreateOutput {
    /// The unique ID of the newly created arena
    pub arena_id: String,
    /// Current status of the arena
    pub status: String,
    /// Number of participants enrolled
    pub participants_count: usize,
}

/// Tool that creates a new SharedArena for multi-agent collaboration.
#[derive(Clone)]
pub struct ArenaCreateTool {
    manager: Arc<RwLock<ArenaManager>>,
}

impl ArenaCreateTool {
    pub fn new(manager: Arc<RwLock<ArenaManager>>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for ArenaCreateTool {
    const NAME: &'static str = "arena_create";
    const DESCRIPTION: &'static str =
        "Create a new collaboration arena for multi-agent coordination. \
         Specify a goal, coordination strategy (peer or pipeline), and participant agent IDs.";

    type Args = ArenaCreateArgs;
    type Output = ArenaCreateOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "arena_create(goal='Research and summarize AI papers', strategy='peer', participants=['researcher', 'summarizer'], coordinator='researcher')".to_string(),
            "arena_create(goal='Build and test feature', strategy='pipeline', participants=['coder', 'reviewer'])".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(
            goal = %args.goal,
            strategy = %args.strategy,
            participants = ?args.participants,
            "Arena creation requested"
        );

        if args.participants.is_empty() {
            return Err(crate::error::AlephError::other(
                "At least one participant is required",
            ));
        }

        let created_by = args
            .coordinator
            .clone()
            .unwrap_or_else(|| args.participants[0].clone());

        // Parse strategy
        let strategy = match args.strategy.as_str() {
            "peer" => {
                let coordinator = args
                    .coordinator
                    .unwrap_or_else(|| args.participants[0].clone());
                CoordinationStrategy::Peer { coordinator }
            }
            "pipeline" => {
                let stages = args
                    .participants
                    .iter()
                    .enumerate()
                    .map(|(i, agent_id)| StageSpec {
                        agent_id: agent_id.clone(),
                        description: format!("Stage {}", i + 1),
                        depends_on: if i > 0 {
                            vec![args.participants[i - 1].clone()]
                        } else {
                            vec![]
                        },
                    })
                    .collect();
                CoordinationStrategy::Pipeline { stages }
            }
            other => {
                return Err(crate::error::AlephError::other(format!(
                    "Unknown strategy '{}': expected 'peer' or 'pipeline'",
                    other
                )));
            }
        };

        // Build participants list
        let participants: Vec<Participant> = args
            .participants
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let role = if i == 0 {
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

        let participants_count = participants.len();

        let manifest = ArenaManifest {
            goal: args.goal,
            strategy,
            participants,
            created_by,
            created_at: Utc::now(),
        };

        let mut manager = self.manager.write().unwrap_or_else(|e| e.into_inner());
        let (arena_id, _handles) = manager
            .create_arena(manifest)
            .map_err(|e| crate::error::AlephError::other(e))?;

        Ok(ArenaCreateOutput {
            arena_id: arena_id.to_string(),
            status: "Active".to_string(),
            participants_count,
        })
    }
}

// =============================================================================
// ArenaQueryTool
// =============================================================================

/// Arguments for querying an arena's status.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ArenaQueryArgs {
    /// Arena ID to query
    pub arena_id: String,
    /// Optional: specific agent's slot to inspect
    #[serde(default)]
    pub agent_id: Option<String>,
}

/// Summary of a single agent's slot.
#[derive(Debug, Clone, Serialize)]
pub struct SlotSummary {
    /// Agent ID that owns this slot
    pub agent_id: String,
    /// Current slot status
    pub status: String,
    /// Number of artifacts in this slot
    pub artifact_count: usize,
}

/// Output from an arena query.
#[derive(Debug, Clone, Serialize)]
pub struct ArenaQueryOutput {
    /// The arena ID queried
    pub arena_id: String,
    /// The arena's goal
    pub goal: String,
    /// Current arena status
    pub status: String,
    /// Number of completed steps
    pub completed_steps: usize,
    /// Total number of steps
    pub total_steps: usize,
    /// Summaries of participant slots
    pub slots: Vec<SlotSummary>,
}

/// Tool that queries a SharedArena's current status and slot details.
#[derive(Clone)]
pub struct ArenaQueryTool {
    manager: Arc<RwLock<ArenaManager>>,
}

impl ArenaQueryTool {
    pub fn new(manager: Arc<RwLock<ArenaManager>>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for ArenaQueryTool {
    const NAME: &'static str = "arena_query";
    const DESCRIPTION: &'static str =
        "Query the status of a collaboration arena. Returns goal, status, progress, \
         and per-agent slot summaries. Optionally filter to a specific agent's slot.";

    type Args = ArenaQueryArgs;
    type Output = ArenaQueryOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "arena_query(arena_id='abc-123')".to_string(),
            "arena_query(arena_id='abc-123', agent_id='researcher')".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(
            arena_id = %args.arena_id,
            agent_id = ?args.agent_id,
            "Arena query requested"
        );

        let arena_id = ArenaId::from_string(&args.arena_id);
        let manager = self.manager.read().unwrap_or_else(|e| e.into_inner());

        // If agent_id was provided, use it directly
        if let Some(ref agent_id_str) = args.agent_id {
            let handle = manager
                .get_handle(&arena_id, agent_id_str)
                .map_err(|e| crate::error::AlephError::other(e))?;

            // Use snapshot_for_context to read arena state through the handle
            let (_, goal, active_agents, completed_steps, total_steps, _) =
                handle.snapshot_for_context();
            let progress = handle.get_progress();

            // Build slot summaries from each known agent
            let mut slot_summaries: Vec<SlotSummary> = Vec::new();
            for agent in &active_agents {
                let artifacts = handle.list_artifacts(agent).unwrap_or_default();
                slot_summaries.push(SlotSummary {
                    agent_id: agent.clone(),
                    status: if artifacts.is_empty() {
                        "Idle".to_string()
                    } else {
                        "Working".to_string()
                    },
                    artifact_count: artifacts.len(),
                });
            }

            // Determine arena status from progress
            let status = if completed_steps > 0 && completed_steps >= total_steps && total_steps > 0
            {
                "Settling".to_string()
            } else {
                "Active".to_string()
            };

            return Ok(ArenaQueryOutput {
                arena_id: args.arena_id,
                goal,
                status,
                completed_steps: progress.completed_steps,
                total_steps: progress.total_steps,
                slots: slot_summaries,
            });
        }

        // No agent_id provided — the caller must provide their own agent_id
        Err(crate::error::AlephError::other(
            "agent_id is required to query arena state (provide your own agent ID)",
        ))
    }
}

// =============================================================================
// ArenaSettleTool
// =============================================================================

/// Arguments for settling an arena.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ArenaSettleArgs {
    /// Arena ID to settle
    pub arena_id: String,
}

/// Output from settling an arena.
#[derive(Debug, Clone, Serialize)]
pub struct ArenaSettleOutput {
    /// The arena ID that was settled
    pub arena_id: String,
    /// Number of facts persisted to long-term memory
    pub facts_persisted: usize,
    /// Number of artifacts archived
    pub artifacts_archived: usize,
    /// Final status after settling
    pub status: String,
}

/// Tool that settles a SharedArena, archiving artifacts and persisting facts.
#[derive(Clone)]
pub struct ArenaSettleTool {
    manager: Arc<RwLock<ArenaManager>>,
}

impl ArenaSettleTool {
    pub fn new(manager: Arc<RwLock<ArenaManager>>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl AlephTool for ArenaSettleTool {
    const NAME: &'static str = "arena_settle";
    const DESCRIPTION: &'static str =
        "Settle a collaboration arena. Transitions the arena to Archived state, \
         persists shared facts, and archives all artifacts. This is a terminal action.";

    type Args = ArenaSettleArgs;
    type Output = ArenaSettleOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "arena_settle(arena_id='abc-123')".to_string(),
        ])
    }

    fn requires_confirmation(&self) -> bool {
        true
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        info!(
            arena_id = %args.arena_id,
            "Arena settle requested"
        );

        let arena_id = ArenaId::from_string(&args.arena_id);
        let mut manager = self.manager.write().unwrap_or_else(|e| e.into_inner());

        let (report, _facts) = manager
            .settle_with_facts(&arena_id)
            .map_err(|e| crate::error::AlephError::other(e))?;

        // Note: The caller (agent loop / dispatcher) is responsible for
        // persisting the returned facts to MemoryStore. The tool reports
        // how many facts were drained.

        Ok(ArenaSettleOutput {
            arena_id: args.arena_id,
            facts_persisted: report.facts_persisted,
            artifacts_archived: report.artifacts_archived,
            status: "Archived".to_string(),
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::AlephTool;

    fn make_manager() -> Arc<RwLock<ArenaManager>> {
        Arc::new(RwLock::new(ArenaManager::new()))
    }

    // ---- ArenaCreateTool ----

    #[test]
    fn test_create_tool_definition() {
        let manager = make_manager();
        let tool = ArenaCreateTool::new(manager);
        let def = AlephTool::definition(&tool);

        assert_eq!(def.name, "arena_create");
        assert!(!def.requires_confirmation);
        assert!(def.llm_context.is_some());
    }

    #[tokio::test]
    async fn test_arena_create_tool_peer() {
        let manager = make_manager();
        let tool = ArenaCreateTool::new(manager);

        let args = ArenaCreateArgs {
            goal: "Research AI papers".to_string(),
            strategy: "peer".to_string(),
            participants: vec!["agent-a".to_string(), "agent-b".to_string()],
            coordinator: Some("agent-a".to_string()),
        };

        let output = AlephTool::call(&tool, args).await.unwrap();

        assert!(!output.arena_id.is_empty());
        assert_eq!(output.status, "Active");
        assert_eq!(output.participants_count, 2);
    }

    #[tokio::test]
    async fn test_arena_create_tool_pipeline() {
        let manager = make_manager();
        let tool = ArenaCreateTool::new(manager);

        let args = ArenaCreateArgs {
            goal: "Build and deploy".to_string(),
            strategy: "pipeline".to_string(),
            participants: vec!["coder".to_string(), "reviewer".to_string()],
            coordinator: None,
        };

        let output = AlephTool::call(&tool, args).await.unwrap();

        assert!(!output.arena_id.is_empty());
        assert_eq!(output.status, "Active");
        assert_eq!(output.participants_count, 2);
    }

    #[tokio::test]
    async fn test_arena_create_tool_invalid_strategy() {
        let manager = make_manager();
        let tool = ArenaCreateTool::new(manager);

        let args = ArenaCreateArgs {
            goal: "Test".to_string(),
            strategy: "invalid".to_string(),
            participants: vec!["agent-a".to_string()],
            coordinator: None,
        };

        let result = AlephTool::call(&tool, args).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_arena_create_tool_empty_participants() {
        let manager = make_manager();
        let tool = ArenaCreateTool::new(manager);

        let args = ArenaCreateArgs {
            goal: "Test".to_string(),
            strategy: "peer".to_string(),
            participants: vec![],
            coordinator: None,
        };

        let result = AlephTool::call(&tool, args).await;
        assert!(result.is_err());
    }

    // ---- ArenaQueryTool ----

    #[test]
    fn test_query_tool_definition() {
        let manager = make_manager();
        let tool = ArenaQueryTool::new(manager);
        let def = AlephTool::definition(&tool);

        assert_eq!(def.name, "arena_query");
        assert!(!def.requires_confirmation);
        assert!(def.llm_context.is_some());
    }

    #[tokio::test]
    async fn test_arena_query_tool() {
        let manager = make_manager();

        // First, create an arena
        let create_tool = ArenaCreateTool::new(Arc::clone(&manager));
        let create_args = ArenaCreateArgs {
            goal: "Test query".to_string(),
            strategy: "peer".to_string(),
            participants: vec!["agent-a".to_string(), "agent-b".to_string()],
            coordinator: Some("agent-a".to_string()),
        };
        let create_output = AlephTool::call(&create_tool, create_args).await.unwrap();

        // Now query it
        let query_tool = ArenaQueryTool::new(Arc::clone(&manager));
        let query_args = ArenaQueryArgs {
            arena_id: create_output.arena_id.clone(),
            agent_id: Some("agent-a".to_string()),
        };
        let query_output = AlephTool::call(&query_tool, query_args).await.unwrap();

        assert_eq!(query_output.arena_id, create_output.arena_id);
        assert_eq!(query_output.goal, "Test query");
        assert_eq!(query_output.status, "Active");
        assert_eq!(query_output.slots.len(), 2);
    }

    #[tokio::test]
    async fn test_arena_query_tool_nonexistent() {
        let manager = make_manager();
        let tool = ArenaQueryTool::new(manager);

        let args = ArenaQueryArgs {
            arena_id: "nonexistent".to_string(),
            agent_id: Some("agent-a".to_string()),
        };

        let result = AlephTool::call(&tool, args).await;
        assert!(result.is_err());
    }

    // ---- ArenaSettleTool ----

    #[test]
    fn test_settle_tool_definition() {
        let manager = make_manager();
        let tool = ArenaSettleTool::new(manager);
        let def = AlephTool::definition(&tool);

        assert_eq!(def.name, "arena_settle");
        assert!(def.requires_confirmation);
        assert!(def.llm_context.is_some());
    }

    #[tokio::test]
    async fn test_arena_settle_tool() {
        let manager = make_manager();

        // Create an arena first
        let create_tool = ArenaCreateTool::new(Arc::clone(&manager));
        let create_args = ArenaCreateArgs {
            goal: "Test settle".to_string(),
            strategy: "peer".to_string(),
            participants: vec!["agent-a".to_string(), "agent-b".to_string()],
            coordinator: Some("agent-a".to_string()),
        };
        let create_output = AlephTool::call(&create_tool, create_args).await.unwrap();

        // Settle it
        let settle_tool = ArenaSettleTool::new(Arc::clone(&manager));
        let settle_args = ArenaSettleArgs {
            arena_id: create_output.arena_id.clone(),
        };
        let settle_output = AlephTool::call(&settle_tool, settle_args).await.unwrap();

        assert_eq!(settle_output.arena_id, create_output.arena_id);
        assert_eq!(settle_output.status, "Archived");
        assert_eq!(settle_output.facts_persisted, 0);
        assert_eq!(settle_output.artifacts_archived, 0);
    }

    #[tokio::test]
    async fn test_arena_settle_tool_nonexistent() {
        let manager = make_manager();
        let tool = ArenaSettleTool::new(manager);

        let args = ArenaSettleArgs {
            arena_id: "nonexistent".to_string(),
        };

        let result = AlephTool::call(&tool, args).await;
        assert!(result.is_err());
    }
}
