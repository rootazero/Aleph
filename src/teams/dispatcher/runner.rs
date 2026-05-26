//! Member task runner.
//!
//! Executes a single coordination task by launching its owner agent through
//! the execution adapter, bounded by a timeout with abort-on-expiry.
//!
//! Shared by `team_delegate` (synchronous, leader-driven delegation) and the
//! autonomous [`TeamDispatcher`](super::TeamDispatcher).
//!
//! ## G2 — per-task worktree isolation
//!
//! Autonomous-dispatch callers may opt into wrapping each member task in a
//! fresh detached-HEAD git worktree so concurrent workers cannot corrupt each
//! other's index. The [`WorktreeHandle`](crate::sandbox::WorktreeHandle) is
//! held in [`execute_member_task`]'s scope and torn down via RAII — explicit
//! `cleanup()` on the happy path, `Drop` safety-net on panic / timeout /
//! abort. Isolation is best-effort: outside a git repository we silently
//! fall back to the pre-G2 behaviour and log a warning.
//!
//! Sandbox-level isolation covers command execution (Bash / git / cargo).
//! File-level tools (Edit / Write) still operate on the parent repo paths;
//! deepening that isolation is a follow-up cycle.

use std::collections::HashMap;

use crate::gateway::context::GatewayContext;
use crate::gateway::event_emitter::NoOpEventEmitter;
use crate::gateway::execution_engine::RunRequest;
use crate::gateway::router::SessionKey;
use crate::sandbox::{worktree as worktree_mod, Sandbox, WorktreeHandle, WorktreeSandbox};
use crate::sync_primitives::Arc;

/// Terminal status of a member task run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberRunStatus {
    /// The agent session finished cleanly.
    Completed,
    /// Execution failed (agent missing, adapter error, or panic).
    Failed,
    /// The run exceeded its timeout and was aborted.
    Timeout,
}

/// Outcome of running a member task. Never an `Err` — every failure mode is
/// mapped here so callers can record task state uniformly.
#[derive(Debug, Clone)]
pub struct MemberRunOutcome {
    pub status: MemberRunStatus,
    /// The agent's last assistant reply (present only on `Completed`).
    pub reply: Option<String>,
    /// Human-readable error (present on `Failed` / `Timeout`).
    pub error: Option<String>,
}

/// Execute `task_text` as agent `agent_id` (within `team_id`), scoped to a
/// task-specific session, bounded by `timeout_secs`. `isolate_workspace`
/// requests a per-task git worktree (best-effort — non-git environments fall
/// back to no isolation with a single warn log).
///
/// The agent runs the full Orchestrator → Harness path via the execution
/// adapter. On timeout the spawned execution is aborted to free resources.
/// On return — happy path, error, or timeout — the worktree handle (if any)
/// is cleaned up via `WorktreeHandle::cleanup` or its `Drop` safety-net.
pub async fn execute_member_task(
    context: &GatewayContext,
    agent_id: &str,
    team_id: &str,
    task_id: &str,
    task_text: String,
    timeout_secs: u64,
    isolate_workspace: bool,
) -> MemberRunOutcome {
    // Resolve the target agent up front — an unknown owner is an explicit
    // failure, never a silent no-op.
    let agent_registry = context.agent_registry();
    let target_agent = match agent_registry.get(agent_id).await {
        Some(a) => a,
        None => {
            return MemberRunOutcome {
                status: MemberRunStatus::Failed,
                reply: None,
                error: Some(format!("Agent '{agent_id}' not found in registry")),
            };
        }
    };

    // G2 — provision a worktree handle when the caller requests isolation.
    // Kept in this function's scope so `Drop` fires on every exit path; the
    // happy path also calls `cleanup()` explicitly below.
    let worktree_handle: Option<WorktreeHandle> = if isolate_workspace {
        provision_worktree(team_id, task_id).await
    } else {
        None
    };
    let sandbox_override: Option<Arc<dyn Sandbox>> = worktree_handle
        .as_ref()
        .map(|h| Arc::new(WorktreeSandbox::new(h.path().to_path_buf())) as Arc<dyn Sandbox>);

    let session_key = SessionKey::task(agent_id, "team", task_id);
    let run_id = uuid::Uuid::new_v4().to_string();
    let metadata = {
        let mut m = HashMap::new();
        m.insert("team_id".to_string(), team_id.to_string());
        m.insert("task_id".to_string(), task_id.to_string());
        if let Some(handle) = worktree_handle.as_ref() {
            m.insert(
                "team_worktree_path".to_string(),
                handle.path().display().to_string(),
            );
        }
        m
    };

    // Inherit the dispatcher's project root so a team worker spawned
    // inside a project-scoped chat lands in the same folder. Captured
    // before the spawn boundary because tokio task-locals do not cross
    // `tokio::spawn`.
    let inherited_workspace = crate::projects::current_project_root();

    let request = RunRequest {
        run_id,
        input: task_text,
        session_key: session_key.clone(),
        timeout_secs: Some(timeout_secs),
        metadata,
        attachments: Vec::new(),
        pending_media: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        sandbox_override,
        workspace_override: inherited_workspace,
            max_iterations_override: None,
    };

    let execution_adapter = Arc::clone(context.execution_adapter());
    let emitter: Arc<dyn crate::gateway::event_emitter::EventEmitter + Send + Sync> =
        Arc::new(NoOpEventEmitter::new());

    // Spawn the execution so it can be aborted on timeout.
    let agent_for_exec = target_agent.clone();
    let handle = tokio::spawn(async move {
        execution_adapter
            .execute(request, agent_for_exec, emitter)
            .await
    });
    let abort_handle = handle.abort_handle();

    let timeout_duration = std::time::Duration::from_secs(timeout_secs);
    let outcome = match tokio::time::timeout(timeout_duration, handle).await {
        Ok(Ok(Ok(()))) => {
            let reply = fetch_last_reply(&target_agent, &session_key).await;
            MemberRunOutcome {
                status: MemberRunStatus::Completed,
                reply: Some(reply.unwrap_or_else(|| "(no reply content)".to_string())),
                error: None,
            }
        }
        Ok(Ok(Err(e))) => MemberRunOutcome {
            status: MemberRunStatus::Failed,
            reply: None,
            error: Some(format!("Execution failed: {e}")),
        },
        Ok(Err(join_err)) => MemberRunOutcome {
            status: MemberRunStatus::Failed,
            reply: None,
            error: Some(format!("Task panicked: {join_err}")),
        },
        Err(_) => {
            // Timeout — abort the spawned task to free resources.
            abort_handle.abort();
            MemberRunOutcome {
                status: MemberRunStatus::Timeout,
                reply: None,
                error: Some(format!("Timed out after {timeout_secs} seconds")),
            }
        }
    };

    // G2 — explicit happy-path cleanup. `Drop` is the safety net for panic /
    // timeout / abort paths; calling `cleanup().await` here turns a noisy
    // Drop-time "leaked" trace into a clean removal.
    if let Some(handle) = worktree_handle {
        if let Err(e) = handle.cleanup().await {
            tracing::warn!(
                team_id = team_id,
                task_id = task_id,
                error = %e,
                "team_worktree: explicit cleanup failed; Drop safety-net will retry"
            );
        }
    }

    outcome
}

/// Best-effort provision a fresh worktree for the given team task. Returns
/// `None` when we are not in a git repository, when the env var
/// `ALEPH_TEAM_MEMBER_WORKTREE` is `"0"`, or when `git worktree add` itself
/// fails. The fallback path matches the pre-G2 behaviour (no isolation).
async fn provision_worktree(team_id: &str, task_id: &str) -> Option<WorktreeHandle> {
    if std::env::var("ALEPH_TEAM_MEMBER_WORKTREE").ok().as_deref() == Some("0") {
        return None;
    }
    let repo_root = match std::env::current_dir() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "team_worktree: cwd lookup failed; skipping isolation");
            return None;
        }
    };
    let safe_team = team_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect::<String>();
    let safe_task = task_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect::<String>();
    let label = if safe_team.is_empty() {
        format!("team-task-{safe_task}")
    } else {
        format!("team-{safe_team}-{safe_task}")
    };
    match worktree_mod::create(&repo_root, &label, None).await {
        Ok(h) => Some(h),
        Err(crate::sandbox::WorktreeError::NotAGitRepo(_)) => {
            tracing::debug!(
                team_id = team_id,
                task_id = task_id,
                "team_worktree: not a git repo; falling back to no isolation"
            );
            None
        }
        Err(e) => {
            tracing::warn!(
                team_id = team_id,
                task_id = task_id,
                error = %e,
                "team_worktree: provision failed; falling back to no isolation"
            );
            None
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Env-var escape hatch must short-circuit before any git plumbing
    /// touches disk. Belt-and-suspenders for ops who want to fall back to
    /// pre-G2 behaviour without a code change.
    #[tokio::test]
    async fn provision_worktree_respects_env_disable() {
        // Serialize against any concurrent test that mutates the same env var.
        // tokio's test framework runs tests in parallel by default; we use
        // a static mutex to guarantee ordering for env-var manipulation.
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let prev = std::env::var("ALEPH_TEAM_MEMBER_WORKTREE").ok();
        std::env::set_var("ALEPH_TEAM_MEMBER_WORKTREE", "0");

        let result = provision_worktree("team-1", "task-1").await;
        assert!(
            result.is_none(),
            "env=0 must short-circuit worktree provisioning"
        );

        match prev {
            Some(v) => std::env::set_var("ALEPH_TEAM_MEMBER_WORKTREE", v),
            None => std::env::remove_var("ALEPH_TEAM_MEMBER_WORKTREE"),
        }
    }

    /// Outside a git repository, provisioning is a logged no-op rather than
    /// an error — callers see `None` and fall back to the shared-tree path.
    #[tokio::test]
    async fn provision_worktree_returns_none_for_non_git_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");

        // Save + change cwd so the env_var path below sees a non-git location.
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_cwd = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(tmp.path()).expect("cd tmp");

        let result = provision_worktree("team-1", "task-1").await;

        std::env::set_current_dir(prev_cwd).expect("restore cwd");

        assert!(
            result.is_none(),
            "non-git cwd must fall back to no isolation, got {result:?}"
        );
    }
}
