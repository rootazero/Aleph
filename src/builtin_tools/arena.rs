//! Arena Tools — `AlephTool` implementations for `SharedArena` interaction.
//!
//! Provides three tools for agents to interact with the `SharedArena` system:
//! - `arena_create` — Create a new collaboration arena
//! - `arena_query` — Query arena status and slot details
//! - `arena_settle` — Settle an arena (archive and persist facts)

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::arena::{ArenaId, ArenaManager, ArenaManifest};
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

/// Tool that creates a new `SharedArena` for multi-agent collaboration.
#[derive(Clone)]
pub struct ArenaCreateTool {
    manager: Arc<RwLock<ArenaManager>>,
}

impl ArenaCreateTool {
    pub const fn new(manager: Arc<RwLock<ArenaManager>>) -> Self {
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

        let participants_count = args.participants.len();

        let manifest = ArenaManifest::build(
            args.goal,
            &args.strategy,
            &args.participants,
            args.coordinator,
            None,
        )
        .map_err(crate::error::AlephError::other)?;

        let mut manager = self.manager.write().map_err(|e| {
            crate::error::AlephError::other(format!("Arena manager lock poisoned: {e}"))
        })?;
        let (arena_id, _handles) = manager
            .create_arena(manifest)
            .map_err(crate::error::AlephError::other)?;

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

/// Tool that queries a `SharedArena`'s current status and slot details.
#[derive(Clone)]
pub struct ArenaQueryTool {
    manager: Arc<RwLock<ArenaManager>>,
}

impl ArenaQueryTool {
    pub const fn new(manager: Arc<RwLock<ArenaManager>>) -> Self {
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
        let manager = self.manager.read().map_err(|e| {
            crate::error::AlephError::other(format!("Arena manager lock poisoned: {e}"))
        })?;

        if let Some(ref agent_id_str) = args.agent_id {
            // Use handle-based query with permission checks
            let agent_id = agent_id_str.clone();
            let handle = manager
                .get_handle(&arena_id, &agent_id)
                .map_err(crate::error::AlephError::other)?;

            let (_, goal, active_agents, completed_steps, total_steps, _) =
                handle.snapshot_for_context();

            let mut slot_summaries: Vec<SlotSummary> = Vec::new();
            for agent_str in &active_agents {
                if agent_str != &agent_id {
                    continue;
                }
                let agent = agent_str.clone();
                let artifacts = handle.list_artifacts(&agent).unwrap_or_default();
                let slot_status = handle
                    .slot_status(&agent)
                    .map_or_else(|| "Idle".to_string(), |s| format!("{s:?}"));
                slot_summaries.push(SlotSummary {
                    agent_id: agent.as_str().to_string(),
                    status: slot_status,
                    artifact_count: artifacts.len(),
                });
            }

            let status = format!("{:?}", handle.arena_status());

            return Ok(ArenaQueryOutput {
                arena_id: args.arena_id,
                goal,
                status,
                completed_steps,
                total_steps,
                slots: slot_summaries,
            });
        }

        // No agent_id — use manager's global query
        let snapshot = manager.query_arena(&arena_id).ok_or_else(|| {
            crate::error::AlephError::other(format!("Arena not found: {arena_id}"))
        })?;

        let goal = snapshot
            .get("goal")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let status = snapshot
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("Active")
            .to_string();
        let completed_steps = snapshot
            .get("progress")
            .and_then(|p| p.get("completed_steps"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let total_steps = snapshot
            .get("progress")
            .and_then(|p| p.get("total_steps"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;

        let slots = snapshot
            .get("slots")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|s| SlotSummary {
                        agent_id: s
                            .get("agent_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        status: s
                            .get("status")
                            .and_then(|v| v.as_str())
                            .unwrap_or("Idle")
                            .to_string(),
                        artifact_count: s
                            .get("artifact_count")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as usize,
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(ArenaQueryOutput {
            arena_id: args.arena_id,
            goal,
            status,
            completed_steps,
            total_steps,
            slots,
        })
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

/// A shared fact returned from settling.
#[derive(Debug, Clone, Serialize)]
pub struct FactSummary {
    /// The content of the fact
    pub content: String,
    /// Which agent contributed this fact
    pub source_agent: String,
    /// Confidence score
    pub confidence: f32,
    /// Tags
    pub tags: Vec<String>,
}

/// Output from settling an arena.
#[derive(Debug, Clone, Serialize)]
pub struct ArenaSettleOutput {
    /// The arena ID that was settled
    pub arena_id: String,
    /// Number of facts drained from the arena
    pub facts_count: usize,
    /// The actual facts — caller (agent loop) should persist these to memory
    pub facts: Vec<FactSummary>,
    /// Number of artifacts archived
    pub artifacts_archived: usize,
    /// Final status after settling
    pub status: String,
}

/// Tool that settles a `SharedArena`, archiving artifacts and persisting facts.
#[derive(Clone)]
pub struct ArenaSettleTool {
    manager: Arc<RwLock<ArenaManager>>,
}

impl ArenaSettleTool {
    pub const fn new(manager: Arc<RwLock<ArenaManager>>) -> Self {
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
        Some(vec!["arena_settle(arena_id='abc-123')".to_string()])
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
        let mut manager = self.manager.write().map_err(|e| {
            crate::error::AlephError::other(format!("Arena manager lock poisoned: {e}"))
        })?;

        let (report, facts) = manager
            .settle_with_facts(&arena_id)
            .map_err(crate::error::AlephError::other)?;

        let fact_summaries: Vec<FactSummary> = facts
            .into_iter()
            .map(|f| FactSummary {
                confidence: f.confidence(),
                content: f.content,
                source_agent: f.source_agent.to_string(),
                tags: f.tags,
            })
            .collect();

        Ok(ArenaSettleOutput {
            arena_id: args.arena_id,
            facts_count: report.facts_persisted,
            facts: fact_summaries,
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
        assert_eq!(query_output.slots.len(), 1);
        assert_eq!(query_output.slots[0].agent_id, "agent-a");
    }

    #[tokio::test]
    async fn test_arena_query_tool_non_participant_rejected() {
        let manager = make_manager();

        let create_tool = ArenaCreateTool::new(Arc::clone(&manager));
        let create_args = ArenaCreateArgs {
            goal: "Test query acl".to_string(),
            strategy: "peer".to_string(),
            participants: vec!["agent-a".to_string(), "agent-b".to_string()],
            coordinator: Some("agent-a".to_string()),
        };
        let create_output = AlephTool::call(&create_tool, create_args).await.unwrap();

        let query_tool = ArenaQueryTool::new(Arc::clone(&manager));
        let query_args = ArenaQueryArgs {
            arena_id: create_output.arena_id.clone(),
            agent_id: Some("agent-c".to_string()),
        };
        let result = AlephTool::call(&query_tool, query_args).await;
        assert!(result.is_err());
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
        assert_eq!(settle_output.facts_count, 0);
        assert!(settle_output.facts.is_empty());
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
