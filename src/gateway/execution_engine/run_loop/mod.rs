//! Agent loop execution and streaming callback.
//!
//! Contains `run_agent_loop` (the think-act two-step loop).

mod inner;
mod project_context;
#[cfg(test)]
mod tests;

// Re-export the project-context helpers at the historical `run_loop::` path so
// any internal consumer keeps resolving the same items.
pub(crate) use project_context::lifecycle_hook_context;

use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::sync_primitives::Arc;

use super::{ExecutionError, RunRequest};
use crate::extension::HookEvent;
use crate::gateway::agent_instance::AgentInstance;
use crate::gateway::event_emitter::EventEmitter;

use crate::executor::ToolRegistry;
use crate::thinker::ProviderRegistry as ThinkerProviderRegistry;

use super::engine::ExecutionEngine;

// ============================================================================
// Agent loop execution
// ============================================================================

impl<P: ThinkerProviderRegistry + 'static, R: ToolRegistry + 'static> ExecutionEngine<P, R> {
    /// Run the agent loop (think->act two-step, Claude Code-inspired).
    ///
    /// Uses the flat `LoopToolRegistry`; tool permissions are enforced by
    /// `ScopedToolService` (merged global → agent → channel policy).
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_agent_loop<E: EventEmitter + Send + Sync + 'static>(
        &self,
        run_id: &str,
        request: &RunRequest,
        agent: Arc<AgentInstance>,
        emitter: Arc<E>,
        deadline: Arc<tokio::sync::Mutex<tokio::time::Instant>>,
        trace_task_id: Option<String>,
        cancel_token: CancellationToken,
        occupancy_out: Arc<std::sync::Mutex<Option<super::helpers::RunContextOccupancy>>>,
    ) -> Result<String, ExecutionError> {
        // Resolve the extension manager + snapshot its HookExecutor once for
        // the whole run. Both flow into `run_agent_loop_inner` so tool
        // dispatch and history compaction can fire hooks without
        // re-snapshotting per turn.
        let extension_manager: Option<Arc<crate::extension::ExtensionManager>> =
            crate::gateway::handlers::plugins::get_extension_manager()
                .ok()
                .map(Arc::clone);
        if let Some(ext_manager) = extension_manager.as_ref() {
            if let Err(e) = ext_manager.ensure_loaded().await {
                warn!("Failed to ensure extension manager is loaded: {}", e);
            }
        }
        let hook_executor = if let Some(ext_manager) = extension_manager.as_ref() {
            let snapshot = ext_manager.hook_executor_snapshot().await;
            (snapshot.hook_count() > 0).then(|| Arc::new(snapshot))
        } else {
            None
        };
        let hook_session_id = request.session_key.to_key_string();

        // BeforeAgentStart — interceptor-kind hooks may abort the run before
        // any provider call; observer-kind hooks just witness the start.
        if let Some(executor) = hook_executor.as_ref() {
            let ctx = lifecycle_hook_context(&hook_session_id, run_id, &agent);
            match executor
                .execute_interceptors(HookEvent::BeforeAgentStart, ctx)
                .await
            {
                Ok((_ctx, hr)) if hr.denied || hr.blocked => {
                    let reason = hr
                        .deny_reason
                        .or(hr.block_reason)
                        .unwrap_or_else(|| "agent start blocked by hook".to_string());
                    warn!(
                        run_id = run_id,
                        reason = %reason,
                        "BeforeAgentStart hook aborted the run"
                    );
                    return Err(ExecutionError::Failed(format!(
                        "BeforeAgentStart hook aborted the run: {reason}"
                    )));
                }
                // Graceful stop (Claude-Code `continue: false`): the hook
                // decided the agent should not start, but this is NOT an error
                // — the run did exactly what the hook asked. Surface the hook's
                // message as the run output instead of failing.
                Ok((_ctx, hr)) if hr.prevent_continuation => {
                    // `stop_message` handles the fallback chain (plain stdout
                    // `messages` → Claude-Code JSON `stopReason` in
                    // `additional_contexts` → default) shared with the
                    // UserPromptSubmit seam and the extension stop gate.
                    let stop_msg = hr.stop_message(
                        "Run halted by BeforeAgentStart hook (prevent_continuation).",
                    );
                    warn!(
                        run_id = run_id,
                        "BeforeAgentStart hook requested prevent_continuation; stopping run"
                    );
                    return Ok(stop_msg);
                }
                Ok(_) => {}
                Err(e) => warn!(run_id = run_id, error = %e, "BeforeAgentStart hook failed"),
            }
        }

        // Publish the project root as a task-local for the duration of the
        // think→act loop so child runs spawned mid-loop (session.send, team
        // dispatcher worker tasks, etc.) inherit the project context.
        // `None` is also published explicitly so a nested run cannot leak
        // an outer scope's project into a non-project agent.
        //
        // Alongside it, publish the per-run `FsScope` carrying this run's
        // workspace artifact dir (`<workspace>/output/documents`, the same
        // value `ToolContext::from_workspace` derives). File tools prefer the
        // task-local over the shared `ToolContextHandle`, so a concurrent run
        // rewriting the handle mid-run no longer redirects THIS run's
        // relative-path writes into the other run's workspace. Mirrors the
        // `effective_workspace` fallback inside `run_agent_loop_inner`
        // (override > agent workspace); validation of the override stays in
        // the inner fn — a vanished dir still fails the run there.
        let scope_workspace = request
            .workspace_override
            .clone()
            .unwrap_or_else(|| agent.workspace().to_path_buf());
        // Team-worktree runs (dispatcher members) carry the parent repo root
        // in metadata: build a rebasing worktree scope so the member's file
        // tools anchor at the worktree root AND parent-repo absolute paths
        // are redirected into the checkout — the same semantics the subagent
        // spawner publishes for `IsolationMode::Worktree`. Everything else
        // gets the plain workspace artifact scope.
        let fs_scope = match request.metadata.get("team_worktree_repo_root") {
            Some(repo_root) if request.workspace_override.is_some() => {
                let wt = scope_workspace
                    .canonicalize()
                    .unwrap_or_else(|_| scope_workspace.clone());
                let repo = std::path::PathBuf::from(repo_root);
                let repo = repo.canonicalize().unwrap_or(repo);
                crate::tools::fs_scope::FsScope::worktree(wt, repo)
            }
            _ => {
                crate::tools::fs_scope::FsScope::workspace(scope_workspace.join("output/documents"))
            }
        };
        // Publish the active agent id as a task-local for the whole run so
        // agent-scoped tools (skill_list / skill_read) can resolve this agent's
        // `~/.aleph/agents/<id>/skills` directory. Mirrors the project-root
        // scope below; `None` outside this scope keeps non-agent paths intact.
        // Originating channel user id (raw sender) for the approval-originator
        // gate. Read before `request` is moved into the loop below; published as
        // a run-tree-wide task-local next to `FsScope`/agent-id so the channel
        // approval bridge can stamp it onto a pending record. `None` for
        // non-channel runs — the gate then degrades to the prior behaviour.
        let originator = request.metadata.get("originator_user_id").cloned();
        // This run's channel-delivery buffer, published run-tree-wide for the
        // same reason as `originator`: the tool chokepoint that harvests a
        // tool's `_media` sits many frames below here and must not have the
        // buffer threaded through `build_request_tool_service` to reach it.
        // Without this, only the slash fast path (which holds the buffer
        // directly) could ever deliver media to a channel — a model-initiated
        // `media_send` / `image_generate` reached the artifact pane and stopped
        // there. Clone rather than move: `request` goes into the loop below.
        let delivery_media = request.pending_media.clone();
        let mut result = crate::agents::with_agent_id(
            Some(agent.id().to_string()),
            crate::projects::with_project_root(
                request.workspace_override.clone(),
                crate::tools::fs_scope::with_fs_scope(
                    Some(fs_scope),
                    crate::tools::turn_context::with_originator(
                        originator,
                        crate::gateway::media::with_pending_media(
                            Some(delivery_media),
                            self.run_agent_loop_inner(
                                run_id,
                                request,
                                agent.clone(),
                                emitter,
                                deadline,
                                trace_task_id,
                                cancel_token,
                                extension_manager,
                                hook_executor.clone(),
                                hook_session_id.clone(),
                                occupancy_out,
                            ),
                        ),
                    ),
                ),
            ),
        )
        .await;

        // AgentEnd — observers witness the end; Interceptor-kind hooks may
        // rewrite the final assistant text via `update_output:` (hermes
        // `transform_llm_output` parity). This reuses the exact `updated_output`
        // seam already honored on the AfterToolCall path — no new protocol.
        // block / deny are meaningless post-hoc (the run is over) and ignored.
        if let Some(executor) = hook_executor.as_ref() {
            let mut ctx = lifecycle_hook_context(&hook_session_id, run_id, &agent);
            ctx = ctx.with_env("AGENT_OUTCOME", if result.is_ok() { "ok" } else { "error" });
            match &result {
                Ok(text) => ctx = ctx.with_tool_output(text.clone()),
                Err(e) => ctx = ctx.with_env("AGENT_ERROR", e.to_string()),
            }
            executor.execute_observers(HookEvent::AgentEnd, &ctx).await;
            // Only the success path carries a final text to transform.
            if let Ok(ref mut text) = result {
                if let Ok((_ctx, hr)) = executor
                    .execute_interceptors(HookEvent::AgentEnd, ctx)
                    .await
                {
                    if let Some(new_text) = hr.updated_output {
                        *text = new_text;
                    }
                }
            }
        }
        result
    }
}
