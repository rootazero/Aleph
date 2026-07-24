//! `TeamDelegateTool` — delegate a task to a team member agent.
//!
//! Synchronous, leader-driven delegation: creates a tracked task, launches an
//! independent agent session for the target member, waits for completion with
//! a timeout, and records the result.
//!
//! This is distinct from the autonomous [`TeamDispatcher`](crate::teams::dispatcher):
//! `team_delegate` runs exactly one member and blocks the caller for the
//! result. Both share [`execute_member_task`] for the actual execution.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::agents::swarm::tasks::{
    CoordTaskStatus, CoordTaskStore, CoordTaskUpdate, NewCoordTask, Priority,
};
use crate::error::{AlephError, Result};
use crate::gateway::context::GatewayContext;
use crate::sync_primitives::Arc;
use crate::teams::artifacts::{ArtifactStore, ArtifactType, NewArtifact, TaskStatus};
use crate::teams::dispatcher::{execute_member_task, MemberDispatchTarget, MemberRunStatus};
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

const fn default_timeout() -> u64 {
    300
}

/// Output from `team_delegate`.
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
/// 3. Run the member agent via the shared execution path, with timeout
/// 4. Update the task record and return the result
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

    /// Persist a delegation result as a report artifact (best-effort).
    async fn persist_result_artifact(
        &self,
        task_id: &str,
        agent_id: &str,
        task: &str,
        reply: &str,
    ) {
        let Some(ref artifact_store) = self.artifact_store else {
            return;
        };
        let _ = artifact_store
            .create_artifact(NewArtifact {
                task_id: task_id.to_string(),
                agent_id: agent_id.to_string(),
                artifact_type: ArtifactType::Report,
                title: format!("Delegation result: {task}"),
                content: reply.to_string(),
                metadata: serde_json::Value::Null,
                status: TaskStatus::Completed,
                blocked_by: vec![],
                assignee: None,
                priority: 0,
            })
            .await;
    }

    /// Record the terminal status of the delegated task.
    async fn finish_task(&self, task_id: &str, status: CoordTaskStatus, result: String) {
        if let Err(e) = self
            .coord_store
            .update_task(
                task_id,
                CoordTaskUpdate {
                    status: Some(status),
                    result: Some(result),
                    ..Default::default()
                },
            )
            .await
        {
            // A failed write leaves the task InProgress until the zombie
            // reclaimer force-fails it — log so the stall is diagnosable.
            tracing::warn!(
                task_id = %task_id,
                error = %e,
                "team_delegate: failed to record terminal task status"
            );
        }
    }
}

/// Force-settles a delegation's coord task if the awaiting future is dropped
/// mid-flight (the leader run's timeout or cancel fired while it awaited the
/// member). `Drop` cannot await — the best-effort cleanup is spawned onto the
/// runtime when one is still available; during process teardown the zombie
/// reclaimer's TTL sweep remains the backstop.
struct SettleOnDrop {
    coord_store: Arc<dyn CoordTaskStore>,
    task_id: String,
    agent_id: String,
    armed: bool,
}

impl SettleOnDrop {
    fn new(coord_store: Arc<dyn CoordTaskStore>, task_id: String, agent_id: String) -> Self {
        Self {
            coord_store,
            task_id,
            agent_id,
            armed: true,
        }
    }

    /// The normal settle path owns the terminal write from here on.
    fn defuse(&mut self) {
        self.armed = false;
    }
}

impl Drop for SettleOnDrop {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let store = Arc::clone(&self.coord_store);
        let task_id = std::mem::take(&mut self.task_id);
        let agent_id = std::mem::take(&mut self.agent_id);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                tracing::warn!(
                    task_id = %task_id,
                    "team_delegate: awaiting leader dropped mid-delegation; force-settling task as Failed"
                );
                if let Err(e) = store
                    .update_task(
                        &task_id,
                        CoordTaskUpdate {
                            status: Some(CoordTaskStatus::Failed),
                            result: Some(
                                "delegation dropped mid-flight: the awaiting leader run \
                                 timed out or was cancelled"
                                    .to_string(),
                            ),
                            ..Default::default()
                        },
                    )
                    .await
                {
                    tracing::warn!(task_id = %task_id, error = %e,
                        "team_delegate: drop-fence terminal write failed (zombie reclaimer is the backstop)");
                }
                let _ = store.release_lock(&task_id, &agent_id).await;
            });
        }
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

        // 1. Verify the agent is a member of the team.
        let members = self.store.get_members(&args.team_id).await?;
        let member = members
            .iter()
            .find(|m| m.agent_id == args.agent_id)
            .ok_or_else(|| {
                AlephError::other(format!(
                    "Agent '{}' is not a member of team '{}'",
                    args.agent_id, args.team_id
                ))
            })?;
        // Build the dispatch target up front so we surface ACP routing
        // misconfiguration before creating a task row.
        let target = MemberDispatchTarget::from_member(member).ok_or_else(|| {
            AlephError::other(format!(
                "Member '{}' has kind=AcpSession but is missing routing fields",
                args.agent_id
            ))
        })?;

        // Tree budget: a team_delegate is a leader-driven delegation. ACP
        // targets stay out of the token budget entirely (external CLI, no
        // in-process session accruing SessionStore tokens). For an in-process
        // member, refuse BEFORE creating the task row when the caller's shared
        // budget is spent (an F9 compact refusal the model can act on); the
        // child is enrolled after the row exists (below). The caller session is
        // the leader's live turn context.
        let caller_session = crate::tools::turn_context::current_session_key();
        let is_acp = matches!(target, MemberDispatchTarget::AcpSession { .. });
        if !is_acp {
            if let Some(caller) = caller_session.as_deref() {
                if let Some(reason) = crate::gateway::goal_budget::tree_budget_refusal(
                    context.session_store(),
                    caller,
                )
                .await
                {
                    return Ok(TeamDelegateOutput {
                        task_id: String::new(),
                        status: DelegateStatus::Failed,
                        reply: None,
                        error: Some(reason),
                    });
                }
            }
        }

        // 2. Create the task record. No `managed_by` flag is set — the
        //    autonomous dispatcher must not pick up a task that `team_delegate`
        //    runs itself.
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

        self.coord_store
            .update_task(
                &task.id,
                CoordTaskUpdate {
                    status: Some(CoordTaskStatus::InProgress),
                    ..Default::default()
                },
            )
            .await?;
        // Task lock — best effort (advisory; a single synchronous owner here).
        let _ = self
            .coord_store
            .acquire_lock(&task.id, &args.agent_id)
            .await;

        // Settle-on-drop fence: the leader awaiting this delegation can
        // itself be killed mid-await (its own run timeout — e.g. the
        // group-chat `member_run_timeout_secs` cap — or a cancel), dropping
        // this future between the InProgress write above and the normal
        // settle below. Without a fence the task row stays InProgress with
        // its lock held until the zombie reclaimer's TTL sweep. Defused once
        // the member run returns.
        let mut settle_fence = SettleOnDrop::new(
            Arc::clone(&self.coord_store),
            task.id.clone(),
            args.agent_id.clone(),
        );

        // Enroll the child (agent:<owner>:team:<task>) — the exact key the run
        // executes under (runner.rs) — into the caller's goal tree budget so its
        // spend counts. Account-only (`false`): the budget was already checked
        // above, and best-effort enrollment never blocks the delegation.
        if !is_acp {
            if let Some(caller) = caller_session.as_deref() {
                let child_key =
                    crate::gateway::router::SessionKey::task(target.agent_id(), "team", &task.id);
                let _ = crate::gateway::goal_budget::check_and_enroll_delegation(
                    context.session_store(),
                    caller,
                    &child_key,
                    false,
                )
                .await;
            }
        }

        // W12 — running-only registration so the leader session's Interrupt
        // demote guard (`steering::session_is_interruptible` →
        // `session_has_running`) sees this in-flight delegation and queues a
        // mid-task correction instead of tearing the leader (and this member
        // run) down. RAII: delists when the run settles — no completed entry,
        // so nothing feeds the proactive announce (the reply is returned
        // inline below). The token is a placeholder: `execute_member_task`
        // has no in-flight abort channel (documented stack-B behaviour), so
        // cancelling this entry only signals a token nothing observes.
        // Skipped when no caller session is wired (nothing to guard).
        let running_reg = caller_session.as_deref().map(|caller| {
            crate::agents::background_tracker::RunningRegistration::register(
                crate::agents::background_tracker::BackgroundAgentTracker::global(),
                uuid::Uuid::new_v4().to_string(),
                tokio_util::sync::CancellationToken::new(),
                format!("team_delegate → {}: {}", args.agent_id, args.task),
                crate::agents::background_tracker::SpawnMeta {
                    parent_id: None,
                    depth: 1,
                    root_session: caller.to_string(),
                    model: None,
                },
            )
        });

        // 3. Run the member agent via the shared execution path. G2 —
        // `team_delegate` is the synchronous leader-driven path (one task
        // at a time, leader awaits), so worktree isolation is unnecessary:
        // pass `false` and keep behaviour identical to pre-G2.
        let outcome = execute_member_task(
            context,
            &target,
            &args.team_id,
            &task.id,
            args.task.clone(),
            args.timeout_secs,
            false,
            // team_delegate is the synchronous leader path with no per-step
            // model or effort override — keep the member on its default
            // model and thinking depth.
            None,
            None,
        )
        .await;
        // The member run has settled — the fence's job is done (the normal
        // bookkeeping below owns the terminal write), and the running-only
        // entry delists so the demote guard stops reading the leader as busy.
        settle_fence.defuse();
        drop(running_reg);
        let _ = self
            .coord_store
            .release_lock(&task.id, &args.agent_id)
            .await;

        // 4. Record the result and return.
        match outcome.status {
            MemberRunStatus::Completed => {
                let reply = outcome
                    .reply
                    .unwrap_or_else(|| "(No reply content)".to_string());
                self.finish_task(&task.id, CoordTaskStatus::Completed, reply.clone())
                    .await;
                self.persist_result_artifact(&task.id, &args.agent_id, &args.task, &reply)
                    .await;
                info!(task_id = %task.id, reply_len = reply.len(), "team_delegate: task completed");
                Ok(TeamDelegateOutput {
                    task_id: task.id,
                    status: DelegateStatus::Completed,
                    reply: Some(reply),
                    error: None,
                })
            }
            MemberRunStatus::Failed => {
                let error = outcome
                    .error
                    .unwrap_or_else(|| "Execution failed".to_string());
                self.finish_task(&task.id, CoordTaskStatus::Failed, error.clone())
                    .await;
                Ok(TeamDelegateOutput {
                    task_id: task.id,
                    status: DelegateStatus::Failed,
                    reply: None,
                    error: Some(error),
                })
            }
            MemberRunStatus::Timeout => {
                let error = outcome
                    .error
                    .unwrap_or_else(|| format!("Timed out after {} seconds", args.timeout_secs));
                self.finish_task(&task.id, CoordTaskStatus::Failed, error.clone())
                    .await;
                Ok(TeamDelegateOutput {
                    task_id: task.id,
                    status: DelegateStatus::Timeout,
                    reply: None,
                    error: Some(error),
                })
            }
        }
    }
}
