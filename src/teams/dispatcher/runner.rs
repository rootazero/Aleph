//! Member task runner.
//!
//! Executes a single coordination task by launching its owner agent through
//! the execution adapter, bounded by a timeout with abort-on-expiry.
//!
//! Shared by `team_delegate` (synchronous, leader-driven delegation) and the
//! autonomous [`TeamDispatcher`](super::TeamDispatcher).
//!
//! ## Timeouts and grace windows
//!
//! [`WORKTREE_CLEANUP_GRACE_MS`] is the bounded window between abort-on-timeout
//! and the explicit worktree teardown. It is the same constant read by both
//! the per-task teardown path and the cleanup helper, so a single edit moves
//! both call sites.
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
//! Isolation covers both surfaces: command execution runs at the worktree
//! path via [`WorktreeSandbox`], and the member's `workspace_override` is
//! pointed at the worktree so `run_agent_loop` roots the run's `FsScope` /
//! `ToolContext` there — file-level tools (Edit / Write) resolve inside the
//! isolated checkout instead of the parent repo.

use std::collections::HashMap;

use crate::gateway::context::GatewayContext;
use crate::gateway::event_emitter::team_fanout;
use crate::gateway::event_emitter::NoOpEventEmitter;
use crate::gateway::execution_engine::{ExecutionError, RunRequest};
use crate::gateway::router::SessionKey;
use crate::sandbox::{worktree as worktree_mod, Sandbox, WorktreeHandle, WorktreeSandbox};
use crate::sync_primitives::Arc;
use crate::teams::types::{TeamMember, TeamMemberKind};

/// Grace window between abort-on-timeout and explicit worktree teardown.
///
/// Long enough for a member task flushing stdout / writing tool output to
/// release its open file descriptors before `git worktree remove --force`
/// races them and produces ENOENT / EBUSY in member-side logs. Short enough
/// that a misbehaving member cannot indefinitely block the dispatcher loop.
pub const WORKTREE_CLEANUP_GRACE_MS: u64 = 250;

/// Where a team member task is dispatched. Built by the caller (dispatcher
/// or `team_delegate`) by inspecting the resolved [`TeamMember`].
#[derive(Debug, Clone)]
// rust-doctor-disable-next-line large-enum-variant
// All variants are small String handles; boxing would complicate the public API without meaningful benefit.
pub enum MemberDispatchTarget {
    /// Resolve `agent_id` against the in-process agent registry and run
    /// through the full Orchestrator → Harness path.
    Agent { agent_id: String },
    /// Route via the ACP adapter pool (external coding CLI: Claude Code,
    /// Codex, ...). `agent_id` is kept for logging / DB linkage; routing
    /// uses `harness_id` + `cwd` + optional `session_name`.
    AcpSession {
        agent_id: String,
        harness_id: String,
        cwd: String,
        session_name: Option<String>,
    },
}

impl MemberDispatchTarget {
    /// The canonical `agent_id` string used for run records, locks, and
    /// `coord_tasks.owner` regardless of dispatch backend.
    #[must_use]
    pub fn agent_id(&self) -> &str {
        match self {
            Self::Agent { agent_id } => agent_id,
            Self::AcpSession { agent_id, .. } => agent_id,
        }
    }

    /// Build a target from a resolved team member row. Returns `None` for
    /// an `AcpSession` row missing required routing fields — caller should
    /// treat that as a fail-fast configuration error.
    #[must_use]
    pub fn from_member(member: &TeamMember) -> Option<Self> {
        match member.kind {
            TeamMemberKind::Agent => Some(Self::Agent {
                agent_id: member.agent_id.clone(),
            }),
            TeamMemberKind::AcpSession => Some(Self::AcpSession {
                agent_id: member.agent_id.clone(),
                harness_id: member.acp_harness_id.clone()?,
                cwd: member.acp_cwd.clone()?,
                session_name: member.acp_session_name.clone(),
            }),
        }
    }
}

/// Terminal status of a member task run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberRunStatus {
    /// The agent session finished cleanly.
    Completed,
    /// Execution failed (agent missing, adapter error, or panic).
    Failed,
    /// The run exceeded its timeout and was aborted.
    Timeout,
    /// The target agent was already running something, so this attempt never
    /// started. Not a task failure — the work has not been tried yet, so it is
    /// deliberately excluded from the retry budget (same reasoning as
    /// [`TaskRunStatus::Abandoned`](crate::agents::swarm::tasks::TaskRunStatus)
    /// for crash orphans) and re-dispatched after the usual backoff.
    ///
    /// Newly reachable since team runs stamp `busy_input_mode = "queue"`: the
    /// gate now returns `AgentBusy` for a collision instead of folding the
    /// message inline into the sibling run.
    Busy,
    /// The run was stopped by a cancellation (operator `run.cancel`, the
    /// session cancel sweep, or a parent run tearing its children down).
    ///
    /// Not a verdict on the work: nothing about the task was judged, the
    /// attempt was simply interrupted. Like [`Self::Busy`] it maps to
    /// [`TaskRunStatus::Abandoned`](crate::agents::swarm::tasks::TaskRunStatus)
    /// so `budget_failures_since` does not count it — a cancellation that
    /// leaves the row live must not spend one of the task's retries.
    Cancelled,
}

impl MemberRunStatus {
    /// The run-log row this outcome becomes. Single source for the mapping
    /// because it decides retry-budget accounting: `budget_failures_since`
    /// counts only `Failed` / `Timeout`, so anything that maps to `Abandoned`
    /// is explicitly "this attempt does not count against the task".
    #[must_use]
    pub const fn run_status(self) -> crate::agents::swarm::tasks::TaskRunStatus {
        use crate::agents::swarm::tasks::TaskRunStatus;
        match self {
            Self::Completed => TaskRunStatus::Completed,
            Self::Failed => TaskRunStatus::Failed,
            Self::Timeout => TaskRunStatus::Timeout,
            Self::Busy | Self::Cancelled => TaskRunStatus::Abandoned,
        }
    }
}

/// Which member-run outcome an execution-layer error is.
///
/// Two of the three classes are deliberately NOT failures: a busy target never
/// started, and a cancelled run was interrupted rather than judged. Both map to
/// `TaskRunStatus::Abandoned` and so leave the task's retry budget untouched.
///
/// Split out as a free function (rather than inline `match` arms in
/// `execute_member_task`) so that classification is testable without standing
/// up an execution adapter — the arm ordering is otherwise unreachable from a
/// unit test, which is exactly how the cancellation case stayed miscategorised.
fn classify_execution_error(err: &ExecutionError) -> MemberRunStatus {
    match err {
        ExecutionError::AgentBusy(_) => MemberRunStatus::Busy,
        ExecutionError::Cancelled => MemberRunStatus::Cancelled,
        _ => MemberRunStatus::Failed,
    }
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

/// Build the request metadata for one dispatched member task.
///
/// Named (rather than inline) so the pinned usage mode is assertable: this and
/// `broadcast::member_run_metadata` are the only two team run producers, and
/// both must stamp it — see `teams::run_mode`.
///
/// Deliberately carries no `UNATTENDED_KEY`, unlike cron / heartbeat / A2A /
/// goal continuations: a member run has no channel, so a confirm-gated tool
/// resolves through `OperatorApprovalRequester` to a Panel card that the user
/// who dispatched the team can answer. The marker would auto-deny that working
/// human-in-the-loop path.
///
/// Attribution triple, mirroring `broadcast::member_run_metadata` (MU4-03,
/// adjudicated 2026-08-18): the interactive caller (`team_delegate`) reaches
/// here with the leader run's task-locals still alive — `run_loop` scopes the
/// whole run future and this function executes before the spawn below — so
/// without these stamps a member's delegated run wrote its session row
/// NULL/NULL (adopted by the operator), skipped the exec-tier ceiling
/// (`role_is_operator(None)` is `true` by design), and in a room answered
/// "who is asking" with the room OWNER. The autonomous dispatcher
/// (`schedule/mod.rs`) spawns bare, so every read there is `None`, nothing is
/// stamped, and behaviour is byte-identical to before — that path genuinely
/// has no live caller.
fn task_run_metadata(
    team_id: &str,
    task_id: &str,
    think_level: Option<&str>,
    worktree: Option<&WorktreeHandle>,
) -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("team_id".to_string(), team_id.to_string());
    m.insert("task_id".to_string(), task_id.to_string());
    crate::teams::run_mode::stamp(&mut m);
    // `run_loop` rebuilds the run's scope from `request.metadata` and NOTHING
    // else, and `ensure_session_under_request_scope` stamps the session row
    // from the same map — the task-local alone does not cross the spawn.
    if let Some(attr) = crate::scope::current_scope() {
        crate::scope::stamp_metadata(&mut m, &attr);
    }
    // `TURN_CONTEXT` is the reliable read in tool context (`ScopedToolService`
    // scopes it around every dispatch, per `sessions_send`'s build_sub_metadata);
    // the `caller_identity` task-local covers non-tool callers. An absent role
    // is read as "local/internal, trusted" (`role_is_operator(None)` is true),
    // so a member's delegated run otherwise skips the exec-tier ceiling.
    let caller_role = crate::tools::turn_context::current_turn_context()
        .and_then(|t| t.caller_role.clone())
        .or_else(crate::gateway::caller_identity::current_caller_role);
    if let Some(role) = caller_role {
        m.insert("caller_role".to_string(), role);
    }
    // `with_request_scope` seeds the room author from `AUTHOR_USER_KEY`
    // specifically and derives nothing about it from the scope — in a room
    // those are different people, and losing it downgrades `ambient_actor()`
    // to the room's creator.
    if let Some(author) = crate::scope::current_room_author() {
        m.insert(
            crate::gateway::execution_engine::AUTHOR_USER_KEY.to_string(),
            author,
        );
    }
    // Per-step effort override (workflow `effort`): the execution engine's
    // `resolve_turn_think_level` reads this request-carried key first, so the
    // member run thinks at the step's declared depth.
    if let Some(level) = think_level {
        m.insert(
            crate::agents::thinking::THINK_LEVEL_SESSION_KEY.to_string(),
            level.to_string(),
        );
    }
    if let Some(handle) = worktree {
        m.insert(
            "team_worktree_path".to_string(),
            handle.path().display().to_string(),
        );
        // Lets `run_agent_loop` build a rebasing `FsScope::worktree` so
        // parent-repo absolute paths are redirected into the checkout,
        // matching the subagent spawner's isolation semantics.
        m.insert(
            "team_worktree_repo_root".to_string(),
            handle.repo_root().display().to_string(),
        );
    }
    m
}

/// Execute `task_text` as the resolved [`MemberDispatchTarget`] (within
/// `team_id`), scoped to a task-specific session, bounded by `timeout_secs`.
/// `isolate_workspace` requests a per-task git worktree (best-effort —
/// non-git environments fall back to no isolation with a single warn log).
///
/// - **Agent target**: runs the full Orchestrator → Harness path via the
///   execution adapter. On timeout the spawned execution is aborted.
/// - **`AcpSession` target**: routes through `AcpAdapterManager::prompt_named`
///   so an external coding CLI (Claude Code, Codex, ...) handles the work.
///   The `cwd` from the target is authoritative — workspace isolation does
///   not apply.
///
/// On return — happy path, error, or timeout — the worktree handle (if any)
/// is cleaned up via `WorktreeHandle::cleanup` or its `Drop` safety-net.
///
/// `model_override` pins the member run to a specific model (a workflow step's
/// per-step `model`); `None` keeps the run on the agent's default. It is ignored
/// for `AcpSession` targets — an external coding CLI owns its own model.
///
/// `think_level` pins the member run's reasoning depth (a workflow step's
/// per-step `effort`, already normalised to a canonical think-level id); it is
/// delivered via the run's `think_level` metadata — the same per-run channel a
/// composer pill uses. `None` keeps the session default. Ignored for
/// `AcpSession` targets, exactly like `model_override`.
///
/// `run_id` is the caller-minted engine run id used as `RunRequest::run_id`.
/// Callers that register the run in the `BackgroundAgentTracker` (e.g.
/// `team_delegate`) pass the SAME id as the tracker `request_id`, so the run
/// is addressable for cancellation; callers with no such registration (the
/// autonomous dispatcher) just supply a fresh id.
pub async fn execute_member_task(
    context: &GatewayContext,
    target: &MemberDispatchTarget,
    team_id: &str,
    task_id: &str,
    task_text: String,
    run_id: String,
    timeout_secs: u64,
    isolate_workspace: bool,
    model_override: Option<crate::gateway::model_override::ModelOverride>,
    think_level: Option<String>,
) -> MemberRunOutcome {
    // ACP-backed members short-circuit through the adapter pool — they
    // never visit the in-process registry, never take a worktree handle.
    // Note: teams-r2 WIP's legacy `agent_id="acp:<harness>"` prefix path
    // is intentionally retired here — main's structured
    // `MemberDispatchTarget::AcpSession` is the single source of truth.
    if let MemberDispatchTarget::AcpSession {
        agent_id,
        harness_id,
        cwd,
        session_name,
    } = target
    {
        return execute_acp_member_task(
            context,
            agent_id,
            harness_id,
            cwd,
            session_name.as_deref(),
            team_id,
            task_id,
            task_text,
            timeout_secs,
        )
        .await;
    }

    let agent_id = target.agent_id();
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

    // The agent axis of the permission model, enforced where every teams face
    // funnels through — `team_delegate`, the dispatcher, and the workflow
    // steps that reach here all run a member's task as some agent.
    //
    // `[agents.X.tool_permissions]` is a permission SET selected by naming an
    // agent, and `allowed_users` is what fences who may select it. That fence
    // exists at `handlers::agent::build_run_request`, so a member refused
    // `chat.send{agent_id:"ops"}` is refused. But `team_create` and
    // `team_delegate` are member-open, and this function resolved the agent
    // straight out of the registry — so naming `ops` as a team MEMBER ran it
    // with its permissions anyway. Both steps legal, the pair equivalent: the
    // same two-step bypass §5.17 closed for `sessions_send`, which carries the
    // argument verbatim.
    //
    // Resolver is `ambient_actor()` — the only one that survives the spawn a
    // team run always executes inside (`CALLER_USER` is dead there, and
    // `ambient_owner()` in a room is its creator rather than the speaker).
    // `None` (the background dispatcher, cron, tests) is unrestricted, like
    // every sibling predicate.
    if let Some(allowed) = agent_registry.get_allowed_users(agent_id).await {
        let actor = crate::gateway::visibility::ambient_actor();
        if !crate::config::types::agent_admits_user(allowed.as_deref(), actor.as_deref()) {
            tracing::warn!(
                team_id = %team_id,
                task_id = %task_id,
                agent_id = %agent_id,
                actor = ?actor,
                "team member run: target agent's allowed_users denies this caller"
            );
            return MemberRunOutcome {
                status: MemberRunStatus::Failed,
                reply: None,
                error: Some(format!(
                    "Agent '{agent_id}' does not admit this caller (allowed_users)"
                )),
            };
        }
    }

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

    let session_key = SessionKey::task(
        agent_id,
        crate::teams::run_mode::TEAM_TASK_TASK_TYPE,
        task_id,
    );
    let metadata = task_run_metadata(
        team_id,
        task_id,
        think_level.as_deref(),
        worktree_handle.as_ref(),
    );

    // Workspace for the member run. With a provisioned worktree the member's
    // ENTIRE run is rooted there — `run_agent_loop` derives its `FsScope`,
    // `ToolContext` and project-skill discovery from `workspace_override`, so
    // file tools now land inside the isolated checkout instead of the parent
    // repo (previously only bash was redirected via `WorktreeSandbox`; the
    // file-tool side of the isolation was a documented follow-up). Without a
    // worktree, inherit the dispatcher's project root so a team worker
    // spawned inside a project-scoped chat lands in the same folder.
    // Captured before the spawn boundary because tokio task-locals do not
    // cross `tokio::spawn`.
    let inherited_workspace = worktree_handle
        .as_ref()
        .map(|h| h.path().to_path_buf())
        .or_else(crate::projects::current_project_root);

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
        // Per-step model override (workflow `model`); `None` for plain team
        // tasks → the run stays on the agent's default model.
        model_override,
    };

    let execution_adapter = Arc::clone(context.execution_adapter());
    // Member runs were previously silent to the Panel (NoOp). Team chat needs the
    // Panel to see each member's contribution + live status, so fan run events out
    // to team.<team_id>.* when a gateway event bus was wired at boot. Falls back to
    // NoOp in non-gateway contexts (unit tests / CLI) where no bus is injected.
    let emitter: Arc<dyn crate::gateway::event_emitter::EventEmitter + Send + Sync> =
        match team_fanout::team_event_bus() {
            Some(bus) => Arc::new(team_fanout::TeamFanoutEmitter::new(
                bus,
                team_id.to_string(),
                agent_id.to_string(),
                None,
            )),
            None => Arc::new(NoOpEventEmitter::new()),
        };

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
        Ok(Ok(Err(e))) => {
            let status = classify_execution_error(&e);
            MemberRunOutcome {
                status,
                // Busy and Cancelled are attempts that never started; there is
                // nothing of this attempt's to salvage and pretending otherwise
                // would attribute a previous attempt's text to this one. The
                // genuine failures keep whatever the member managed to say —
                // see the Timeout arm.
                reply: match status {
                    MemberRunStatus::Busy | MemberRunStatus::Cancelled => None,
                    _ => fetch_last_reply(&target_agent, &session_key).await,
                },
                error: Some(match status {
                    MemberRunStatus::Busy => format!("Agent busy, attempt deferred: {e}"),
                    MemberRunStatus::Cancelled => {
                        format!("Run cancelled, attempt not counted: {e}")
                    }
                    _ => format!("Execution failed: {e}"),
                }),
            }
        }
        Ok(Err(join_err)) => MemberRunOutcome {
            status: MemberRunStatus::Failed,
            reply: fetch_last_reply(&target_agent, &session_key).await,
            error: Some(format!("Task panicked: {join_err}")),
        },
        Err(_) => {
            // Timeout — abort the spawned task to free resources.
            abort_handle.abort();
            // `tokio::JoinHandle::abort` only signals cancellation; the
            // task may still be mid-flight, holding the worktree directory
            // open while it finishes flushing stdout / writing tool output.
            // The explicit `cleanup().await` below calls
            // `git worktree remove --force`, which would race the spawned
            // task's open file descriptors and produce ENOENT / EBUSY in
            // member-side logs. A short grace window lets the task unwind
            // before the directory disappears. The window is bounded so a
            // misbehaving member cannot indefinitely block the dispatcher.
            tokio::time::sleep(std::time::Duration::from_millis(
                WORKTREE_CLEANUP_GRACE_MS,
            ))
            .await;
            // Keep what it produced. The per-task session is durable and the
            // messages are already written, so the same one-line read the
            // success arm uses works here too — and the NEXT attempt's
            // `build_recovery_section` tells the member verbatim to "resume
            // from where they left off; reuse work already done and do not
            // restart from scratch" while listing only "timeout: Timed out
            // after N seconds". A ten-minute research step therefore restarted
            // from zero on every retry until it burned `max_retries`.
            //
            // `error` is deliberately left alone rather than merged: "it did
            // not finish" and "here is what it had" are different facts, and
            // `run_task` only writes `task.result` on Completed, so a partial
            // cannot be mistaken for a deliverable.
            MemberRunOutcome {
                status: MemberRunStatus::Timeout,
                reply: fetch_last_reply(&target_agent, &session_key).await,
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

/// Dispatch a task to an ACP-backed external coding CLI session.
///
/// Returns a [`MemberRunOutcome`] mirroring the in-process path so the
/// dispatcher's outcome-recording logic stays uniform. Missing
/// [`AcpAdapterManager`] in the gateway context is a configuration error
/// (`acp.enabled = false` but a team has an `AcpSession` member) and is
/// surfaced as `MemberRunStatus::Failed`, never silently dropped.
#[allow(clippy::too_many_arguments)]
async fn execute_acp_member_task(
    context: &GatewayContext,
    agent_id: &str,
    harness_id: &str,
    cwd: &str,
    session_name: Option<&str>,
    team_id: &str,
    task_id: &str,
    task_text: String,
    timeout_secs: u64,
) -> MemberRunOutcome {
    let manager = match context.acp_manager() {
        Some(m) => Arc::clone(m),
        None => {
            return MemberRunOutcome {
                status: MemberRunStatus::Failed,
                reply: None,
                error: Some(format!(
                    "ACP member '{agent_id}' cannot run: acp manager not configured"
                )),
            };
        }
    };

    tracing::info!(
        team_id = team_id,
        task_id = task_id,
        agent_id = agent_id,
        harness_id = harness_id,
        cwd = cwd,
        session_name = ?session_name,
        "team_dispatcher: routing task to ACP session"
    );

    let prompt_fut = async move {
        manager
            .prompt_named(
                harness_id,
                &task_text,
                cwd,
                session_name,
                None, // use harness default mode
                true, // reuse session for team continuity
                None, // streaming callback wired in B2/follow-up
            )
            .await
    };

    match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), prompt_fut).await {
        Ok(Ok(reply)) => MemberRunOutcome {
            status: MemberRunStatus::Completed,
            reply: Some(reply),
            error: None,
        },
        Ok(Err(e)) => MemberRunOutcome {
            status: MemberRunStatus::Failed,
            reply: None,
            error: Some(format!("ACP session error: {e}")),
        },
        Err(_) => MemberRunOutcome {
            status: MemberRunStatus::Timeout,
            reply: None,
            error: Some(format!("ACP session timed out after {timeout_secs}s")),
        },
    }
}

/// Fetch the last assistant reply from an agent's session.
///
/// Reads a trailing window (default 20 frames — enough to span one full
/// tool round-trip: user → assistant(tool_call) → tool → assistant(final))
/// and returns the LAST message with `MessageRole::Assistant`. Reading
/// only the most-recent frame silently returned `None` whenever the
/// final assistant turn was followed by a tool result (the tool-using
/// pattern the dispatcher overwhelmingly produces), making every
/// tool-using attempt look like an empty reply to the caller.
async fn fetch_last_reply(
    agent: &crate::gateway::agent_instance::AgentInstance,
    session_key: &SessionKey,
) -> Option<String> {
    const TRAILING_WINDOW: usize = 20;
    let history = agent.get_history(session_key, Some(TRAILING_WINDOW)).await;
    history
        .iter()
        .rev()
        .find(|msg| {
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
    use crate::agents::swarm::tasks::retry::budget_failures_since;
    use crate::agents::swarm::tasks::{CoordTaskRun, TaskRunStatus};
    use crate::teams::types::{TeamMember, TeamMemberKind};

    fn run_row(status: TaskRunStatus) -> CoordTaskRun {
        CoordTaskRun {
            id: "r1".into(),
            task_id: "t1".into(),
            agent_id: "reviewer".into(),
            started_at: 1,
            ended_at: Some(2),
            status,
            summary: None,
            error: None,
            review_verdict: None,
            reviewer_kind: None,
            reviewer_id: None,
        }
    }

    /// A busy target never ran, so its run row must not spend a retry. Asserted
    /// against `budget_failures_since` itself rather than against the literal
    /// `Abandoned`, because the property that matters is "the budget does not
    /// count it" — renaming the status must not silently pass this.
    #[test]
    fn a_busy_attempt_does_not_consume_the_retry_budget() {
        let busy = run_row(MemberRunStatus::Busy.run_status());
        assert_eq!(
            budget_failures_since(&[busy], None),
            0,
            "a deferred attempt is not a failed attempt"
        );

        let failed = run_row(MemberRunStatus::Failed.run_status());
        let timed_out = run_row(MemberRunStatus::Timeout.run_status());
        assert_eq!(
            budget_failures_since(&[failed, timed_out], None),
            2,
            "real attempt outcomes must still be counted"
        );
    }

    /// U2 — a cancelled run is an interruption, not a verdict. Asserted the
    /// same way as the busy case: against `budget_failures_since`, so the
    /// property under test is "the budget does not count it", not "it happens
    /// to be spelled Abandoned".
    #[test]
    fn a_cancelled_attempt_does_not_consume_the_retry_budget() {
        let cancelled = run_row(MemberRunStatus::Cancelled.run_status());
        assert_eq!(
            budget_failures_since(&[cancelled], None),
            0,
            "a cancelled attempt is not a failed attempt"
        );
    }

    /// U2 producer side — the arm that *makes* a `Cancelled` outcome. Without
    /// this the enum variant could exist, map correctly, and still never be
    /// reached: `ExecutionError::Cancelled` would fall through to the
    /// `Failed` catch-all and spend a retry, with every mapping test green.
    #[test]
    fn a_cancelled_execution_error_classifies_as_cancelled() {
        assert_eq!(
            classify_execution_error(&ExecutionError::Cancelled),
            MemberRunStatus::Cancelled
        );
        assert_eq!(
            classify_execution_error(&ExecutionError::AgentBusy("x".into())),
            MemberRunStatus::Busy
        );
        assert_eq!(
            classify_execution_error(&ExecutionError::Failed("boom".into())),
            MemberRunStatus::Failed
        );
    }

    fn agent_member() -> TeamMember {
        TeamMember {
            team_id: "t1".into(),
            agent_id: "reviewer".into(),
            role: "reviewer".into(),
            joined_at: 0,
            kind: TeamMemberKind::Agent,
            acp_harness_id: None,
            acp_cwd: None,
            acp_session_name: None,
        }
    }

    fn acp_member(name: Option<&str>) -> TeamMember {
        TeamMember {
            team_id: "t1".into(),
            agent_id: format!(
                "acp:claude-code:/work/proj{}",
                name.map(|n| format!(":{n}")).unwrap_or_default()
            ),
            role: "reviewer".into(),
            joined_at: 0,
            kind: TeamMemberKind::AcpSession,
            acp_harness_id: Some("claude-code".into()),
            acp_cwd: Some("/work/proj".into()),
            acp_session_name: name.map(String::from),
        }
    }

    #[test]
    fn dispatch_target_from_agent_member() {
        let m = agent_member();
        let t = MemberDispatchTarget::from_member(&m).expect("agent must resolve");
        match t {
            MemberDispatchTarget::Agent { agent_id } => assert_eq!(agent_id, "reviewer"),
            MemberDispatchTarget::AcpSession { .. } => panic!("wrong variant"),
        }
    }

    #[test]
    fn dispatch_target_from_acp_member_named() {
        let m = acp_member(Some("review-bot"));
        let t = MemberDispatchTarget::from_member(&m).expect("acp must resolve");
        match t {
            MemberDispatchTarget::AcpSession {
                harness_id,
                cwd,
                session_name,
                ..
            } => {
                assert_eq!(harness_id, "claude-code");
                assert_eq!(cwd, "/work/proj");
                assert_eq!(session_name.as_deref(), Some("review-bot"));
            }
            MemberDispatchTarget::Agent { .. } => panic!("wrong variant"),
        }
    }

    #[test]
    fn dispatch_target_acp_missing_routing_returns_none() {
        let mut m = acp_member(None);
        m.acp_harness_id = None; // simulate corrupt DB row
        assert!(MemberDispatchTarget::from_member(&m).is_none());
    }

    /// Env-var escape hatch must short-circuit before any git plumbing
    /// touches disk. Belt-and-suspenders for ops who want to fall back to
    /// pre-G2 behaviour without a code change.
    #[tokio::test]
    async fn provision_worktree_respects_env_disable() {
        // Serialize against any concurrent test that mutates the same env var.
        // tokio's test framework runs tests in parallel by default; we use
        // a static mutex to guarantee ordering for env-var manipulation.
        static LOCK: crate::sync_primitives::Mutex<()> = crate::sync_primitives::Mutex::new(());
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
        static LOCK: crate::sync_primitives::Mutex<()> = crate::sync_primitives::Mutex::new(());
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

    #[test]
    fn task_run_metadata_pins_the_team_run_mode() {
        let m = task_run_metadata("t1", "task-9", None, None);
        assert_eq!(
            m.get(crate::config::types::policies::MODE_SESSION_KEY)
                .map(String::as_str),
            Some("work")
        );
        assert_eq!(m.get("team_id").map(String::as_str), Some("t1"));
        assert_eq!(m.get("task_id").map(String::as_str), Some("task-9"));
        assert!(
            !m.contains_key(crate::agents::thinking::THINK_LEVEL_SESSION_KEY),
            "an undeclared effort must not write a think-level key"
        );
    }

    /// The per-step `effort` override still rides the same metadata map.
    #[test]
    fn task_run_metadata_carries_a_declared_think_level() {
        let m = task_run_metadata("t1", "task-9", Some("high"), None);
        assert_eq!(
            m.get(crate::agents::thinking::THINK_LEVEL_SESSION_KEY)
                .map(String::as_str),
            Some("high")
        );
    }

    /// MU4-03: the interactive path (`team_delegate`) reaches
    /// `task_run_metadata` with the leader run's task-locals alive, so the
    /// attribution triple must be stamped into the metadata — `run_loop`
    /// rebuilds the run's scope from `request.metadata` and nothing else.
    #[tokio::test]
    async fn task_run_metadata_stamps_the_attribution_triple_under_a_live_scope() {
        let attr = crate::scope::ScopeAttribution::personal("u-bob");
        let m = crate::scope::with_scope(
            Some(attr),
            crate::scope::with_room_author(Some("u-bob".to_string()), async {
                task_run_metadata("t1", "task-9", None, None)
            }),
        )
        .await;
        let expected = crate::scope::scope_from_metadata(&m)
            .expect("scope pair must be stamped into the metadata");
        assert_eq!(expected.owner_user_id, "u-bob");
        assert_eq!(
            m.get(crate::gateway::execution_engine::AUTHOR_USER_KEY)
                .map(String::as_str),
            Some("u-bob"),
            "the room author must ride the metadata — with_request_scope seeds it from this key alone"
        );
    }

    /// The autonomous dispatcher (`schedule/mod.rs`) spawns bare: no scope,
    /// no turn context, no role — nothing may be stamped, byte-identical to
    /// the pre-fix behaviour.
    #[test]
    fn task_run_metadata_without_a_caller_stamps_nothing() {
        let m = task_run_metadata("t1", "task-9", None, None);
        assert!(crate::scope::scope_from_metadata(&m).is_none());
        assert!(!m.contains_key("caller_role"));
        assert!(!m.contains_key(crate::gateway::execution_engine::AUTHOR_USER_KEY));
    }
}
