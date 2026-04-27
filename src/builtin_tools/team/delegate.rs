//! TeamDelegateTool — delegate a task to a team member agent.
//!
//! Launches an independent agent session for the target member, sends the task
//! as a message, waits for completion with a timeout, and records the result
//! in the team task store.

use std::collections::HashMap;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::agents::swarm::tasks::{
    CoordTaskStatus, CoordTaskStore, CoordTaskUpdate, NewCoordTask, Priority,
};
use crate::error::{AlephError, Result};
use crate::gateway::context::GatewayContext;
use crate::gateway::event_emitter::NoOpEventEmitter;
use crate::gateway::execution_engine::RunRequest;
use crate::gateway::router::SessionKey;
use crate::sync_primitives::{Arc, Mutex};
use crate::teams::artifacts::{ArtifactStore, ArtifactType, NewArtifact};
use crate::teams::TeamStore;
use crate::tools::AlephTool;

// =============================================================================
// Args / Output
// =============================================================================

/// Arguments for delegating a task to a team member.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TeamDelegateArgs {
    /// ID of the team
    pub team_id: String,

    /// ID of the target member agent to delegate the task to
    pub agent_id: String,

    /// Task description / instruction to send to the agent
    pub task: String,

    /// Timeout in seconds (default: 300)
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_timeout() -> u64 {
    300
}

/// Output from team_delegate.
#[derive(Debug, Clone, Serialize)]
pub struct TeamDelegateOutput {
    /// The task ID created in the team store
    pub task_id: String,
    /// Status of the delegation
    pub status: DelegateStatus,
    /// The agent's reply (if completed successfully)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply: Option<String>,
    /// Error message if failed or timed out
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Outcome of the delegation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegateStatus {
    /// Task completed successfully
    Completed,
    /// Task timed out
    Timeout,
    /// Task execution failed
    Failed,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool that delegates a task to a specific team member agent.
///
/// Flow:
/// 1. Verify the agent is a member of the specified team
/// 2. Create a task record in the team store
/// 3. Launch a session for the target agent via the execution adapter
/// 4. Wait for completion with timeout
/// 5. Update the task record and return the result
#[derive(Clone)]
pub struct TeamDelegateTool {
    store: Arc<dyn TeamStore>,
    coord_store: Arc<dyn CoordTaskStore>,
    artifact_store: Option<Arc<dyn ArtifactStore>>,
    context: Option<GatewayContext>,
}

impl TeamDelegateTool {
    pub fn new(
        store: Arc<dyn TeamStore>,
        coord_store: Arc<dyn CoordTaskStore>,
        artifact_store: Option<Arc<dyn ArtifactStore>>,
    ) -> Self {
        Self {
            store,
            coord_store,
            artifact_store,
            context: None,
        }
    }

    /// Create with a gateway context for execution.
    pub fn with_context(
        store: Arc<dyn TeamStore>,
        coord_store: Arc<dyn CoordTaskStore>,
        artifact_store: Option<Arc<dyn ArtifactStore>>,
        context: GatewayContext,
    ) -> Self {
        Self {
            store,
            coord_store,
            artifact_store,
            context: Some(context),
        }
    }

    /// Set the gateway context (called during tool wiring).
    pub fn set_context(&mut self, context: GatewayContext) {
        self.context = Some(context);
    }

    /// Fetch the last assistant reply from an agent's session.
    async fn fetch_last_reply(
        agent: &crate::gateway::agent_instance::AgentInstance,
        session_key: &SessionKey,
    ) -> Option<String> {
        let history = agent.get_history(session_key, Some(1)).await;
        history
            .last()
            .filter(|msg| {
                matches!(
                    msg.role,
                    crate::gateway::agent_instance::MessageRole::Assistant
                )
            })
            .map(|msg| msg.content.clone())
    }
}

#[async_trait]
impl AlephTool for TeamDelegateTool {
    const NAME: &'static str = "team_delegate";
    const DESCRIPTION: &'static str =
        "Delegate a task to a team member agent. The target agent must be a member of the \
        specified team. Creates a tracked task, launches a session for the member agent, \
        sends the task instruction, and waits for the result with a configurable timeout. \
        Returns the agent's response or an error/timeout status.";

    type Args = TeamDelegateArgs;
    type Output = TeamDelegateOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "team_delegate(team_id='abc123', agent_id='researcher', task='Summarize the latest AI papers')".to_string(),
            "team_delegate(team_id='abc123', agent_id='coder', task='Implement the login page', timeout_secs=600)".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let context = self.context.as_ref().ok_or_else(|| {
            AlephError::other("GatewayContext not configured for team_delegate tool")
        })?;

        // 1. Verify agent is a member of the team
        let members = self.store.get_members(&args.team_id).await?;
        let is_member = members.iter().any(|m| m.agent_id == args.agent_id);
        if !is_member {
            return Err(AlephError::other(format!(
                "Agent '{}' is not a member of team '{}'",
                args.agent_id, args.team_id
            )));
        }

        // 2. Create task record via CoordTaskStore
        let task = self
            .coord_store
            .create_task(NewCoordTask {
                team_id: Some(args.team_id.clone()),
                subject: args.task.clone(),
                description: String::new(),
                owner: Some(args.agent_id.clone()),
                priority: Priority::Normal,
                blocked_by: Vec::new(),
                metadata: serde_json::json!({}),
            })
            .await?;

        info!(
            team_id = %args.team_id,
            agent_id = %args.agent_id,
            task_id = %task.id,
            "team_delegate: task created, launching agent session"
        );

        // Mark task as running
        self.coord_store
            .update_task(
                &task.id,
                CoordTaskUpdate {
                    status: Some(CoordTaskStatus::InProgress),
                    ..Default::default()
                },
            )
            .await?;

        // Acquire task lock (non-fatal — best effort)
        let _ = self
            .coord_store
            .acquire_lock(&task.id, &args.agent_id)
            .await;

        // 3. Look up the target agent in the registry
        let agent_registry = context.agent_registry();
        let target_agent = match agent_registry.get(&args.agent_id).await {
            Some(agent) => agent,
            None => {
                self.coord_store
                    .update_task(
                        &task.id,
                        CoordTaskUpdate {
                            status: Some(CoordTaskStatus::Failed),
                            result: Some(format!(
                                "Agent '{}' not found in registry",
                                args.agent_id
                            )),
                            ..Default::default()
                        },
                    )
                    .await?;
                return Err(AlephError::other(format!(
                    "Agent '{}' not found in registry",
                    args.agent_id
                )));
            }
        };

        // 4. Build a run request using a task-scoped session key
        let session_key = SessionKey::task(&args.agent_id, "team", &task.id);
        let run_id = uuid::Uuid::new_v4().to_string();

        let request = RunRequest {
            run_id: run_id.clone(),
            input: args.task.clone(),
            session_key: session_key.clone(),
            timeout_secs: Some(args.timeout_secs),
            metadata: {
                let mut m = HashMap::new();
                m.insert("team_id".to_string(), args.team_id.clone());
                m.insert("task_id".to_string(), task.id.clone());
                m
            },
            attachments: Vec::new(),
            pending_media: Arc::new(Mutex::new(Vec::new())),
        };

        let execution_adapter = Arc::clone(context.execution_adapter());
        let emitter: Arc<dyn crate::gateway::event_emitter::EventEmitter + Send + Sync> =
            Arc::new(NoOpEventEmitter::new());

        // 5. Spawn execution so we can abort on timeout (prevents resource leak)
        let agent_for_exec = target_agent.clone();
        let handle = tokio::spawn(async move {
            execution_adapter
                .execute(request, agent_for_exec, emitter)
                .await
        });
        let abort_handle = handle.abort_handle();

        let timeout_duration = std::time::Duration::from_secs(args.timeout_secs);
        let execution_result = tokio::time::timeout(timeout_duration, handle).await;

        match execution_result {
            Ok(Ok(Ok(()))) => {
                // Execution completed — fetch the last assistant message
                let reply = Self::fetch_last_reply(&target_agent, &session_key).await;
                let reply_text = reply.unwrap_or_else(|| "(No reply content)".to_string());

                let _ = self
                    .coord_store
                    .release_lock(&task.id, &args.agent_id)
                    .await;
                self.coord_store
                    .update_task(
                        &task.id,
                        CoordTaskUpdate {
                            status: Some(CoordTaskStatus::Completed),
                            result: Some(reply_text.clone()),
                            ..Default::default()
                        },
                    )
                    .await?;

                // Persist delegation result as an artifact (best-effort)
                if let Some(ref artifact_store) = self.artifact_store {
                    let _ = artifact_store
                        .create_artifact(NewArtifact {
                            task_id: task.id.clone(),
                            agent_id: args.agent_id.clone(),
                            artifact_type: ArtifactType::Report,
                            title: format!("Delegation result: {}", args.task),
                            content: reply_text.clone(),
                            metadata: serde_json::Value::Null,
                        })
                        .await;
                }

                info!(
                    task_id = %task.id,
                    reply_len = reply_text.len(),
                    "team_delegate: task completed"
                );

                Ok(TeamDelegateOutput {
                    task_id: task.id,
                    status: DelegateStatus::Completed,
                    reply: Some(reply_text),
                    error: None,
                })
            }
            Ok(Ok(Err(e))) => {
                let error_msg = format!("Execution failed: {}", e);
                warn!(
                    task_id = %task.id,
                    error = %e,
                    "team_delegate: execution failed"
                );

                let _ = self
                    .coord_store
                    .release_lock(&task.id, &args.agent_id)
                    .await;
                self.coord_store
                    .update_task(
                        &task.id,
                        CoordTaskUpdate {
                            status: Some(CoordTaskStatus::Failed),
                            result: Some(error_msg.clone()),
                            ..Default::default()
                        },
                    )
                    .await?;

                Ok(TeamDelegateOutput {
                    task_id: task.id,
                    status: DelegateStatus::Failed,
                    reply: None,
                    error: Some(error_msg),
                })
            }
            Ok(Err(join_err)) => {
                let error_msg = format!("Task panicked: {}", join_err);
                warn!(task_id = %task.id, "team_delegate: task panicked");

                let _ = self
                    .coord_store
                    .release_lock(&task.id, &args.agent_id)
                    .await;
                self.coord_store
                    .update_task(
                        &task.id,
                        CoordTaskUpdate {
                            status: Some(CoordTaskStatus::Failed),
                            result: Some(error_msg.clone()),
                            ..Default::default()
                        },
                    )
                    .await?;

                Ok(TeamDelegateOutput {
                    task_id: task.id,
                    status: DelegateStatus::Failed,
                    reply: None,
                    error: Some(error_msg),
                })
            }
            Err(_) => {
                // Timeout — abort the spawned task to free resources
                abort_handle.abort();

                let error_msg = format!("Timed out after {} seconds", args.timeout_secs);
                warn!(
                    task_id = %task.id,
                    timeout_secs = args.timeout_secs,
                    "team_delegate: timed out, task aborted"
                );

                let _ = self
                    .coord_store
                    .release_lock(&task.id, &args.agent_id)
                    .await;
                self.coord_store
                    .update_task(
                        &task.id,
                        CoordTaskUpdate {
                            status: Some(CoordTaskStatus::Failed),
                            result: Some(error_msg.clone()),
                            ..Default::default()
                        },
                    )
                    .await?;

                Ok(TeamDelegateOutput {
                    task_id: task.id,
                    status: DelegateStatus::Timeout,
                    reply: None,
                    error: Some(error_msg),
                })
            }
        }
    }
}
