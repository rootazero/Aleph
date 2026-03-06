//! Collaborative executor for multi-agent tasks using SharedArena.

use async_trait::async_trait;
use tracing::debug;

use super::{ExecutionContext, TaskExecutor};
use crate::arena::{
    ArenaManifest, ArenaPermissions, CoordinationStrategy, Participant, ParticipantRole, StageSpec,
};
use crate::arena::ArenaManager;
use crate::dispatcher::agent_types::{CollaborativeTask, Task, TaskResult, TaskType};
use crate::error::{AlephError, Result};
use crate::sync_primitives::{Arc, RwLock};

/// Executor that creates SharedArena instances for collaborative multi-agent tasks.
///
/// When executed, it translates a `CollaborativeTask` into an `ArenaManifest`,
/// creates the arena via `ArenaManager`, and returns the arena ID. Actual agent
/// execution happens asynchronously — the handles are distributed to agents
/// through their RunContext.
#[derive(Clone)]
pub struct CollaborativeExecutor {
    arena_manager: Arc<RwLock<ArenaManager>>,
}

impl CollaborativeExecutor {
    /// Create a new CollaborativeExecutor with the given ArenaManager.
    pub fn new(arena_manager: Arc<RwLock<ArenaManager>>) -> Self {
        Self { arena_manager }
    }

    /// Build an ArenaManifest from a CollaborativeTask definition.
    fn build_manifest(collab: &CollaborativeTask) -> Result<ArenaManifest> {
        let strategy = match collab.strategy.as_str() {
            "peer" => {
                let coord = collab
                    .coordinator
                    .clone()
                    .unwrap_or_else(|| collab.agents.first().cloned().unwrap_or_default());
                CoordinationStrategy::Peer { coordinator: coord }
            }
            "pipeline" => {
                let stages = collab
                    .stages
                    .as_ref()
                    .map(|s| {
                        s.iter()
                            .map(|st| StageSpec {
                                agent_id: st.agent_id.clone(),
                                description: st.description.clone(),
                                depends_on: st.depends_on.clone(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                CoordinationStrategy::Pipeline { stages }
            }
            other => {
                return Err(AlephError::other(format!(
                    "Unknown collaboration strategy: {}",
                    other
                )));
            }
        };

        let participants: Vec<Participant> = collab
            .agents
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

        Ok(ArenaManifest {
            goal: collab.description.clone(),
            strategy,
            participants,
            created_by: collab.agents.first().cloned().unwrap_or_default(),
            created_at: chrono::Utc::now(),
        })
    }
}

#[async_trait]
impl TaskExecutor for CollaborativeExecutor {
    fn supported_types(&self) -> Vec<&'static str> {
        vec!["collaborative"]
    }

    fn can_execute(&self, task_type: &TaskType) -> bool {
        matches!(task_type, TaskType::Collaborative(_))
    }

    async fn execute(&self, task: &Task, _ctx: &ExecutionContext) -> Result<TaskResult> {
        let collab = match &task.task_type {
            TaskType::Collaborative(c) => c,
            _ => return Err(AlephError::other("Not a collaborative task")),
        };

        debug!(
            "CollaborativeExecutor creating arena for task '{}' with {} agents",
            task.id,
            collab.agents.len()
        );

        // Build manifest from CollaborativeTask
        let manifest = Self::build_manifest(collab)?;

        // Create arena via ArenaManager
        let mut mgr = self.arena_manager.write().unwrap_or_else(|e| e.into_inner());
        let (arena_id, _handles) = mgr
            .create_arena(manifest)
            .map_err(|e| AlephError::other(e))?;

        debug!(
            "Arena '{}' created for collaborative task '{}'",
            arena_id, task.id
        );

        // Return arena_id — actual agent execution happens asynchronously.
        // The handles are distributed to agents via their RunContext.
        Ok(TaskResult::with_output(serde_json::json!({
            "arena_id": arena_id.as_str(),
            "status": "active",
            "participants": collab.agents,
        }))
        .with_summary(format!(
            "Created collaborative arena {} with {} participants",
            arena_id,
            collab.agents.len()
        )))
    }

    fn name(&self) -> &str {
        "CollaborativeExecutor"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatcher::agent_types::{
        AiTask, CollaborativeStage, CollaborativeTask, FileOp, TaskType,
    };
    use std::path::PathBuf;

    fn make_arena_manager() -> Arc<RwLock<ArenaManager>> {
        Arc::new(RwLock::new(ArenaManager::new()))
    }

    fn make_peer_task() -> Task {
        Task::new(
            "collab_1",
            "Peer Collaboration",
            TaskType::Collaborative(CollaborativeTask {
                description: "Research and summarize a topic".into(),
                agents: vec!["researcher".into(), "writer".into()],
                strategy: "peer".into(),
                coordinator: Some("researcher".into()),
                stages: None,
            }),
        )
    }

    fn make_pipeline_task() -> Task {
        Task::new(
            "collab_2",
            "Pipeline Collaboration",
            TaskType::Collaborative(CollaborativeTask {
                description: "Code review pipeline".into(),
                agents: vec!["analyzer".into(), "reviewer".into(), "approver".into()],
                strategy: "pipeline".into(),
                coordinator: None,
                stages: Some(vec![
                    CollaborativeStage {
                        agent_id: "analyzer".into(),
                        description: "Analyze code quality".into(),
                        depends_on: vec![],
                    },
                    CollaborativeStage {
                        agent_id: "reviewer".into(),
                        description: "Review analysis results".into(),
                        depends_on: vec!["analyzer".into()],
                    },
                    CollaborativeStage {
                        agent_id: "approver".into(),
                        description: "Final approval".into(),
                        depends_on: vec!["reviewer".into()],
                    },
                ]),
            }),
        )
    }

    #[tokio::test]
    async fn test_collaborative_executor_peer() {
        let mgr = make_arena_manager();
        let executor = CollaborativeExecutor::new(mgr.clone());
        let task = make_peer_task();
        let ctx = ExecutionContext::new("graph_1");

        let result = executor.execute(&task, &ctx).await.unwrap();

        // Verify arena_id is in the output
        let arena_id = result.output.get("arena_id").unwrap().as_str().unwrap();
        assert!(!arena_id.is_empty());

        // Verify status
        assert_eq!(result.output.get("status").unwrap().as_str().unwrap(), "active");

        // Verify participants
        let participants = result.output.get("participants").unwrap().as_array().unwrap();
        assert_eq!(participants.len(), 2);

        // Verify summary
        assert!(result.summary.unwrap().contains("2 participants"));

        // Verify arena actually exists in the manager
        let mgr_read = mgr.read().unwrap_or_else(|e| e.into_inner());
        let arena_id_obj = crate::arena::ArenaId::from_string(arena_id);
        assert!(mgr_read.query_arena(&arena_id_obj).is_some());
    }

    #[tokio::test]
    async fn test_collaborative_executor_pipeline() {
        let mgr = make_arena_manager();
        let executor = CollaborativeExecutor::new(mgr.clone());
        let task = make_pipeline_task();
        let ctx = ExecutionContext::new("graph_2");

        let result = executor.execute(&task, &ctx).await.unwrap();

        let arena_id = result.output.get("arena_id").unwrap().as_str().unwrap();
        assert!(!arena_id.is_empty());

        let participants = result.output.get("participants").unwrap().as_array().unwrap();
        assert_eq!(participants.len(), 3);

        assert!(result.summary.unwrap().contains("3 participants"));
    }

    #[tokio::test]
    async fn test_collaborative_executor_invalid_strategy() {
        let mgr = make_arena_manager();
        let executor = CollaborativeExecutor::new(mgr);

        let task = Task::new(
            "collab_bad",
            "Bad Strategy",
            TaskType::Collaborative(CollaborativeTask {
                description: "Will fail".into(),
                agents: vec!["agent-a".into()],
                strategy: "unknown_strategy".into(),
                coordinator: None,
                stages: None,
            }),
        );

        let ctx = ExecutionContext::new("graph_err");
        let result = executor.execute(&task, &ctx).await;

        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("Unknown collaboration strategy"));
    }

    #[test]
    fn test_can_execute() {
        let mgr = make_arena_manager();
        let executor = CollaborativeExecutor::new(mgr);

        // Should match Collaborative
        assert!(executor.can_execute(&TaskType::Collaborative(CollaborativeTask {
            description: "test".into(),
            agents: vec!["a".into()],
            strategy: "peer".into(),
            coordinator: None,
            stages: None,
        })));

        // Should NOT match other types
        assert!(!executor.can_execute(&TaskType::FileOperation(FileOp::List {
            path: PathBuf::from("/tmp"),
        })));

        assert!(!executor.can_execute(&TaskType::AiInference(AiTask {
            prompt: "test".into(),
            requires_privacy: false,
            has_images: false,
            output_format: None,
        })));
    }

    #[test]
    fn test_supported_types() {
        let mgr = make_arena_manager();
        let executor = CollaborativeExecutor::new(mgr);
        assert_eq!(executor.supported_types(), vec!["collaborative"]);
    }

    #[test]
    fn test_name() {
        let mgr = make_arena_manager();
        let executor = CollaborativeExecutor::new(mgr);
        assert_eq!(executor.name(), "CollaborativeExecutor");
    }
}
