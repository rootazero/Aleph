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

/// This run's scope attribution: what the producer stamped, corrected by what
/// the gateway itself knows about the session key.
///
/// [`crate::scope::scope_from_metadata`] reads a map some producer wrote.
/// `projects.current_session_key` is written by exactly one function
/// ([`crate::projects::ProjectStore::claim_session_key`]), so a key it names is
/// a room **by declaration** rather than by inference — and when the two
/// disagree, the declaration is the one that knows.
///
/// The disagreement is not hypothetical and does not heal. A room is opened
/// (`projects.room_session` claims the key) before anyone speaks, and whoever
/// speaks first creates the row. A producer that never heard of rooms stamps
/// that row `personal:<first speaker>`; `stamp_attribution` is create-only and
/// `attribution_backfill`'s predicate is `owner_user_id IS NULL AND scope_id IS
/// NULL`, so the wrong stamp is permanent and the room goes invisible to every
/// other member — including its owner — while `projects.list` keeps listing it.
///
/// `handlers::agent::resolve_attribution` already asks this question for ONE
/// producer, the Panel's `agent.run` / `chat.send`, and keeps asking it there
/// because it can also *refuse*: a non-member gets `ProjectNotFound`, the same
/// refusal a named foreign project gets. This function cannot refuse — it runs
/// after admission, on a request that is already going to execute — so it only
/// corrects the filing. The six producers that never pass through that handler
/// (the channel inbound router, cron, heartbeat, the teams dispatcher,
/// `session_send`, A2A) get the correction here.
///
/// Only the scope is replaced. `owner_user_id` still names whoever spoke: for a
/// project-scoped row visibility is decided by the roster
/// ([`crate::gateway::visibility::owner_and_scope_visible_to`]), so overwriting
/// the owner would buy nothing and lose the attribution.
///
/// A catalogue failure reads as "not a room", matching
/// `handlers::agent::room_claiming`'s ruling for the same lookup: a degraded
/// SQLite must not turn into a mis-scoped turn *or* a refused one. The cost is
/// bounded — the row is then stamped the way it is stamped today.
fn request_scope(request: &RunRequest) -> Option<crate::scope::ScopeAttribution> {
    let stamped = crate::scope::scope_from_metadata(&request.metadata);
    let Some(pid) = room_claiming(&request.session_key) else {
        return stamped;
    };
    let mut attr = stamped?;
    attr.scope = crate::scope::ScopeId::Project(pid);
    Some(attr)
}

/// The project that has claimed `session_key` as its room conversation.
///
/// Twin of `handlers::agent::room_claiming`, deliberately not shared with it:
/// that one lives on the admission path and its `None` feeds a branch that may
/// refuse, this one lives after admission and its `None` means "leave the
/// producer's stamp alone". Both read the same column through the same store
/// method, which is the part that must not be duplicated.
fn room_claiming(session_key: &crate::routing::session_key::SessionKey) -> Option<String> {
    match crate::projects::ProjectStore::shared()
        .project_for_session_key(&session_key.to_key_string())
    {
        Ok(pid) => pid,
        Err(e) => {
            tracing::warn!(error = %e, "projects: room claim lookup failed; leaving the producer's scope stamp alone");
            None
        }
    }
}

/// Establishes this run's scope attribution (owner/scope) and this turn's
/// speaker as task-locals for `fut`'s duration, both derived from
/// `request.metadata` — see [`crate::scope::stamp_metadata`] and
/// [`super::AUTHOR_USER_KEY`] at the two origin sites (`build_run_request`, the
/// channel inbound router's `execute_for_context_inner`).
///
/// The two travel together on purpose. The scope names the ROOM, the author
/// names whoever is typing, and in a project room those genuinely differ; the
/// main path's user-message writer (`harness_bridge::session_seed`) reaches
/// neither the request nor `CALLER_USER`, so seeding both here is what keeps
/// the transcript label and the memory partition talking about the same turn.
///
/// Extracted from [`ExecutionEngine::run_agent_loop`]'s wrapping nest for
/// testability: unlike the other layers in that nest (agent id, project
/// root, fs scope), this one depends on nothing but the metadata map, so it
/// can be driven directly with a minimal `RunRequest` and a probe future.
pub(super) async fn with_request_scope<F, T>(request: &RunRequest, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    let author = request.metadata.get(super::AUTHOR_USER_KEY).cloned();
    crate::scope::with_scope(
        request_scope(request),
        crate::scope::with_room_author(author, fut),
    )
    .await
}

/// The run-admission spend arm: deny before this run claims any resource if
/// its principal ([`crate::spend::principal_from_metadata`], resolved off
/// `request.metadata` the same way [`with_request_scope`] resolves scope —
/// see that resolver's doc for why it is unconditionally equivalent to the
/// floor arm's `ambient_principal`) is over its ceiling for the period.
///
/// Both engines call this — `ExecutionEngine::execute` (`execute.rs`, ahead
/// of `admit_run`) and `SimpleExecutionEngine::execute` (`simple.rs`, which
/// has no `admit_run` to gate alongside and so calls this as its own first
/// act) — as the very first thing they do, before either claims a session
/// run slot, a concurrency permit, or (`SimpleExecutionEngine`) transitions
/// the agent to `Running`. A principal already over the line should never be
/// handed a resource it is about to be denied anyway, and a shared call site
/// is what keeps `SimpleExecutionEngine`, which has no `admit_run` to
/// piggyback on, from silently skipping a floor the full engine enforces —
/// see this module's doc and the plan this task belongs to for why "a floor
/// only one engine honours is not a floor".
///
/// One helper rather than each engine open-coding the
/// `spend::principal_from_metadata` / `spend::check` pairing itself: a
/// second, hand-written copy of that pairing is exactly the kind of drift
/// [`crate::spend::check`]'s own doc warns against.
pub(super) fn deny_if_over_spend(request: &RunRequest) -> Result<(), ExecutionError> {
    let principal = crate::spend::principal_from_metadata(&request.metadata);
    let now_ms = chrono::Utc::now().timestamp_millis();
    admission_result_for(crate::spend::check(&principal, now_ms))
}

/// [`deny_if_over_spend`], plus the one thing a bare `?` on it cannot do: put
/// a `RunError` on the wire when it denies.
///
/// This fires *before* `RunAccepted` — the run has not yet claimed a slot,
/// so nothing downstream will ever emit a terminal frame for it. Every other
/// `Err` an engine's `execute()` can produce is caught by the think/act
/// loop's own error arm, which renders `ExecutionError::user_receipt` onto
/// the wire before returning — see `execute.rs`'s `Err(e) => { .. }` tail.
/// The admission arm runs ahead of that whole apparatus, so it is on its own
/// for upholding the same contract, the one
/// `busy_queue::spawn_queued_run`/`deliver_with_ticket` already assume every
/// `execute()` error keeps: "the engine already emits `RunError` for
/// anything that fails inside `execute`". Skipping this and returning the
/// bare `Err` — which is what both engines did before this existed — breaks
/// that contract silently: `chat.send`/`agent.run` still returns a `run_id`,
/// but the run never reaches `RunAccepted` OR `RunError`, so every observer
/// (Panel spinner, CLI, channel reply) waits on a run that will never answer
/// and only `spend_ledger` and a `tracing::error!` line know why (see
/// task-12's real-machine fixture, assertion 4).
///
/// `session_key` is stamped explicitly on the frame — the same reason
/// `spawn_queued_run`'s own never-ran producer does, on the same never-ran
/// case: with no `RunAccepted` to have seeded it, `EventVisibilityIndex`'s
/// run→session index has nothing to resolve `ByRunId` against, so the frame
/// must carry its own addressing or the delivery filter drops it before any
/// client sees it.
pub(super) async fn deny_if_over_spend_and_report<E: EventEmitter + Send + Sync>(
    request: &RunRequest,
    emitter: &E,
) -> Result<(), ExecutionError> {
    report_admission_denial(deny_if_over_spend(request), request, emitter).await
}

/// The reporting half of [`deny_if_over_spend_and_report`], with the
/// admission result taken as a plain parameter instead of computed here —
/// the same hazard-free split [`admission_result_for`] exists for: a test
/// can drive this with a hand-built `Err(ExecutionError::SpendExhausted {
/// .. })` without installing a low ceiling into the process-wide
/// policy/ledger `OnceLock`s the rest of this crate's tests already share
/// and race.
async fn report_admission_denial<E: EventEmitter + Send + Sync>(
    result: Result<(), ExecutionError>,
    request: &RunRequest,
    emitter: &E,
) -> Result<(), ExecutionError> {
    if let Err(e) = result {
        let (error_code, error_message) = e.user_receipt(
            crate::gateway::i18n::Locale::from_run_metadata(&request.metadata),
        );
        let seq = emitter.next_seq();
        if let Err(emit_err) = emitter
            .emit(crate::gateway::event_emitter::StreamEvent::RunError {
                run_id: request.run_id.clone(),
                seq,
                error: error_message,
                error_code: Some(error_code.to_string()),
                session_key: Some(request.session_key.to_key_string()),
            })
            .await
        {
            tracing::warn!(
                run_id = %request.run_id,
                error = %emit_err,
                "failed to emit RunError stream event for a spend-denied admission",
            );
        }
        return Err(e);
    }
    Ok(())
}

/// The translation [`deny_if_over_spend`] applies to whatever
/// [`crate::spend::check`] returns — split out so it is testable without
/// touching the process-global ledger/policy `check` reads. `cargo test
/// --lib` runs every test in this crate in one binary, and
/// `providers::metering`'s tests already install a real (if generously
/// high) process-wide policy for their own wiring tests; a second test here
/// racing `spend::check`'s global read would either see that policy or the
/// pre-install default depending on execution order. Taking the `Verdict`
/// as a plain parameter sidesteps the hazard entirely — same reasoning as
/// `spend::check_with`'s own doc.
fn admission_result_for(verdict: crate::spend::Verdict) -> Result<(), ExecutionError> {
    match verdict {
        crate::spend::Verdict::Allowed(_) => Ok(()),
        crate::spend::Verdict::Denied { limit, spent } => {
            // `spent.period_end_ms` is `None` only out of a raw
            // `SpendLedger` read (see `Spent::period_end_ms`'s doc) — this
            // `spent` came from `spend::check`/`check_with`, which always
            // fills it in before returning `Denied` (the only early-return
            // that skips filling it is `Verdict::Allowed`, taken while the
            // policy is disabled). A `None` here means an earlier layer
            // broke that guarantee; recomputing a plausible-looking instant
            // would hide exactly the drift this field exists to prevent, so
            // this is `expect`, not a fallback.
            let reset_ms = spent
                .period_end_ms
                .expect("spend::check always fills period_end_ms before returning Verdict::Denied");
            Err(ExecutionError::SpendExhausted { limit, reset_ms })
        }
    }
}

/// Create this run's session row **under the run's own attribution**.
///
/// `SessionMetadata::stamp_attribution` reads `scope::current_scope()` on the
/// CREATE branch of `SessionStore::get_or_create`, and that task-local does not
/// survive `tokio::spawn`. Every producer of a run — the Panel handler
/// (`handlers::agent`), the channel inbound router, cron, heartbeat, the teams
/// dispatcher, `sessions_send`, A2A — hands the request to a *spawned* task, so
/// by the time the engine creates the row the ambient scope is `None`.
///
/// ⚠️ **"the attribution is sitting right there in `request.metadata`" is a
/// claim about each producer, not a property of this helper.** This helper can
/// only read what a producer wrote, and until 2026-08-09 the teams fan-out
/// wrote neither key — `member_run_metadata` inserted `team_id` / `chain_depth`
/// / `platform` / run-mode and stopped — so for every member run this helper
/// was a no-op while a reader of this sentence would believe it was covered.
/// The census that matters is `scope_stamping_producers_are_all_accounted_for`
/// in this module's tests, not this paragraph. The row
/// then persists with `owner_user_id`/`scope_id` NULL and is adopted as
/// owner-owned, which for a member means their own session is invisible to them
/// (`sessions.list` empty, `sessions.set_topic` "not found",
/// `chat.context_estimate` null) and their transcript is attributed to the
/// operator — including to `handlers::trace`'s cross-user read audit, which
/// compares against `effective_owner` and therefore never fires for it.
///
/// Reading the metadata rather than capturing the caller's task-local is
/// deliberate and load-bearing: `current_scope()` is **also** `None` in the
/// gateway dispatch loop (which scopes `CALLER_USER`/`CALLER_ROLE`, not the
/// attribution), so the resolved attribution exists ONLY in the metadata map.
/// Using the same accessor [`with_request_scope`] uses is what keeps the row
/// and the loop from disagreeing about whose turn this is.
///
/// One helper rather than the same three lines at both engines' call sites:
/// `ExecutionEngine::execute` and `SimpleExecutionEngine::execute` each create
/// the row, and a second copy is a second answer waiting to drift.
pub(super) async fn ensure_session_under_request_scope(
    agent: &AgentInstance,
    request: &RunRequest,
) {
    crate::scope::with_scope(
        request_scope(request),
        agent.ensure_session(&request.session_key),
    )
    .await;
}

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
                // The exec-side twin of `fs_scope`, published from the SAME
                // `override > agent workspace` value so the two layers cannot
                // drift on "where does this run work". This is the ONLY channel
                // by which the sandbox learns the authorised root: routing it
                // through the tool's `working_dir` argument (as the tool
                // adapters used to) launders a gateway-owned path through a
                // model-writable field, and the jail — which exists to judge
                // model-supplied paths — then refused it.
                crate::sandbox::context::with_exec_workspace(
                    Some(scope_workspace.clone()),
                    crate::tools::fs_scope::with_fs_scope(
                        Some(fs_scope),
                        // P1 data isolation: publish this run's owner/scope
                        // attribution as a task-local, sibling of `originator`
                        // below — both are derived from `request.metadata` and
                        // must be re-seeded at every spawn boundary that carries
                        // this request's metadata forward (see
                        // `scope::with_scope`'s doc and `carry_policy_metadata`).
                        with_request_scope(
                            request,
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
