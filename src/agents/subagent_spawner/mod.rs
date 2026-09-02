//! Subagent spawner — Harness-based sub-agent execution.
//!
//! Replacement for the legacy pre-Harness `run_subagent` entry point.
//! The spawner takes a `SpawnerBase` (shared session/tools/provider)
//! plus a `SpawnRequest` (`agent_def`, task, model, timeout, cancel), builds a
//! child ephemeral `SessionKey`, assembles a `HarnessDeps` bundle with the
//! agent's system prompt and `max_iterations` + a tool service wrapped in
//! `AllowlistToolService`, seeds the task as a `UserMessage`, runs
//! `AgentHarness::run` under `tokio::time::timeout` + `catch_unwind` for
//! timeout + panic isolation, then walks the child session event log to
//! synthesize a `LoopRunResult`.
//!
pub(crate) mod fork;

use std::panic::AssertUnwindSafe;

use crate::sync_primitives::Arc;

use futures::FutureExt;
use tokio_util::sync::CancellationToken;

use crate::agents::allowlist_tool_service::AllowlistToolService;
use crate::agents::runtime::LoopRunResult;
use crate::agents::AgentDef;
use crate::harness::agent::AgentHarness;
use crate::harness::callback::NoopHarnessCallback;
use crate::harness::chain_context::ChainContext;
use crate::harness::deps::HarnessDeps;
use crate::memory::extensions::MemoryExtensionRegistry;
use crate::memory::store::raw_memory::RawMemoryStore;
use crate::providers::AiProvider;
use crate::routing::session_key::SessionKey;
use crate::session::events::{
    now_ms, MessageContent, SessionEvent, SessionEventRecord, TurnTrigger,
};
use crate::session::service::{SessionId, SessionService};
use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};
use crate::tools::service::ToolService;

/// Shared infrastructure shared by all sub-agent spawns in a given
/// orchestration context (session actor, parent tool service,
/// provider, and the parent's chain context).
#[derive(Clone)]
pub struct SpawnerBase {
    /// Shared session service (same actor as the parent).
    pub session: Arc<dyn SessionService>,
    /// The parent's tool service. The spawner decorates this with an
    /// `AllowlistToolService` gated on `AgentDef.is_tool_allowed`.
    pub parent_tools: Arc<dyn ToolService>,
    /// Provider used for LLM calls. The spawner wraps this with a
    /// `ModelOverrideProvider` when `SpawnRequest.model` is set.
    pub provider: Arc<dyn AiProvider>,
    /// The parent's chain context. The spawner derives a child via
    /// `ChainContext::child()`.
    pub chain: ChainContext,
    /// Spec 1 G2 — when set, the spawner emits a `RawMemory(Delegation)`
    /// row after a successful spawn so `CompressionService` can distil
    /// LESSON-flavoured notes for the parent agent's long-term memory.
    /// The pre-phase7 A2A path emits the same row from `a2a/sub_agent.rs`;
    /// this field plugs the gap on the post-phase7 intra-process path.
    pub raw_memory_writer: Option<Arc<dyn RawMemoryStore>>,
    /// Optional capture-filter registry threaded into the delegation emit.
    pub capture_registry: Option<Arc<MemoryExtensionRegistry>>,
    /// Parent agent identity stamped onto the emitted `RawMemory` row.
    /// `None` falls back to `"default"` to match the A2A path's behaviour.
    pub parent_agent_id: Option<String>,
    /// Parent session id — when set, the row is tagged with it so
    /// `notes` can correlate the lesson with the originating session.
    pub parent_session_id: Option<String>,
    /// Stage 5a (#9) — parent's guardrail registry. Inherited by the
    /// subagent so sub-runs enforce the same Input/Output/ToolCall checks
    /// as the spawning harness. `None` for harness instances without a
    /// configured registry.
    pub guardrails: Option<Arc<crate::guardrails::GuardrailRegistry>>,
    /// Stage A (P1) — stall watchdog config from `[stability]`. `None` when
    /// `stall_timeout_secs` is unset.
    pub stall_config: Option<crate::harness::StallConfig>,
    /// Stage A (P1) — bounded consecutive-failure cap from `[stability]`.
    pub consecutive_failure_cap: Option<usize>,
    /// Stage A (P1) — per-turn wall-clock timeout from `[stability]`.
    pub turn_timeout: Option<std::time::Duration>,
    /// Stage A (P1) — trace sink, cloned from parent's `HarnessDeps`.
    /// Subagent run events flow into the same sink as the main runner.
    pub trace_sink: Option<Arc<dyn crate::harness::TraceSink>>,
    /// P3 Stage I — shared plugin-registry handle. Used by `McpScope::provision`
    /// for per-agent MCP scope lookups (validated + snapshotted under a read
    /// guard at provision time). `None` means MCP scope is disabled (legacy
    /// callers + tests with no `mcp_servers`); a non-empty
    /// `agent_def.mcp_servers` will fail-loud if this is `None`.
    pub plugin_registry:
        Option<Arc<tokio::sync::RwLock<crate::extension::registry::PluginRegistry>>>,
    /// A2 — global cap on concurrently-running subagent spawns. `None` skips
    /// the cap (direct test callers); `Some(_)` makes `spawn()` acquire a
    /// permit held for the child's full lifetime.
    pub subagent_semaphore: Option<Arc<tokio::sync::Semaphore>>,
    /// VESR v1.1 (b) — routing-experience store. `Some` makes `spawn()` wrap the
    /// child trace sink with its own `OutcomeObserver`, recording the subagent's
    /// run under `agent_def.id`. Holds the embedder via `embed_task`. `None`
    /// keeps the child sink raw (today's behavior — no capture).
    pub routing_store: Option<Arc<crate::routing::RoutingExperienceStore>>,
    /// B15 — the runner's boot-time `[execution] max_iterations`, inherited so a
    /// child whose `AgentDef` declares no cap still gets one. `None` falls
    /// through to `FALLBACK_MAX_ITERATIONS` inside `resolve_max_iterations`;
    /// either way the child loop is never left uncapped (the main path's
    /// "the harness loop is never left uncapped" invariant, `harness_bridge`).
    pub default_max_iterations: Option<usize>,
    /// The runner's `[tool_service] parallel_tool_concurrency`, inherited so
    /// the child's Act-phase cap matches the operator's configured value —
    /// including 0/1, which DISABLES the parallel fast path. `None` (tests /
    /// legacy callers) falls back to the config default.
    pub parallel_tool_concurrency: Option<usize>,
    /// The runner's `[context_budget]` config, inherited so the child gets its
    /// OWN budget + compactor + preflight pipeline. `None` (tests / legacy
    /// callers / `[context_budget]` disabled) leaves the child unmanaged,
    /// matching the main harness under the same config.
    ///
    /// The config travels, never a built `ContextBudget`: the instance carries
    /// per-run calibration and circuit-breaker counters that must not be shared
    /// between the parent and a child (nor between two concurrent children).
    pub context_budget_config: Option<crate::context::budget::ContextBudgetConfig>,
    /// The parent runner's per-run budget refiner, used to re-key the child's
    /// **prompt** budget onto the model it will actually run on (`resolved_
    /// model` below) exactly as the main loop re-keys its own every run.
    /// `None` (tests / legacy callers) keeps the chain-minimum derivation.
    pub context_budget_refiner: Option<crate::orchestrator::deps_builder::ContextBudgetRefiner>,
    /// The parent runner's configured per-provider context-window override,
    /// fed to [`Self::context_budget_refiner`] exactly as the main loop feeds
    /// its own refinement.
    pub primary_context_window: Option<u32>,
    /// The parent runner's cheap-tier summarization provider. Handed to the
    /// child's `ContextCompactor` so its side-channel summarization bills the
    /// operator's flash sibling, not the main reasoning model. `None` keeps the
    /// child summarizing on its own LLM.
    pub cheap_summary_provider: Option<Arc<dyn AiProvider>>,
    /// The parent runner's verifier chain, shared with the child so a
    /// subagent that enters a tool-call death loop is caught by the same
    /// structural watchdog as the parent (ToolLoopVerifier, StopHookVerifier,
    /// etc.). `None` keeps the child on the legacy no-verifier path —
    /// matching pre-2026-09 behaviour but explicitly opted-in rather than
    /// the silent default of `verifier_chain: None` that this audit fixed.
    pub verifier_chain: Option<Arc<crate::verification::VerifierChain>>,
}

/// Per-spawn configuration. All lifetimes are scoped to a single `spawn` call.
pub struct SpawnRequest<'a> {
    /// Agent definition (id, `allowed_tools`, `max_iterations`, `model_hint`, …).
    pub agent_def: &'a AgentDef,
    /// Task description — seeded as the child's first `UserMessage`.
    pub task: &'a str,
    /// Optional summary of the parent's context. When set, prefixed to the
    /// task with a "## Context from parent agent" header (matches legacy
    /// `run_subagent` behaviour).
    pub context_summary: Option<&'a str>,
    /// Explicit model override (highest priority). Falls back to
    /// `agent_def.model_hint`, then to whatever the provider uses natively.
    pub model: Option<&'a str>,
    /// Hard wall-clock timeout for the entire run.
    pub timeout_secs: u64,
    /// Cancellation token observed between turns by the harness.
    pub cancel: CancellationToken,
    /// Per-call override of where the child's starting context comes from.
    ///
    /// `None` — every caller that predates the knob — falls back to
    /// `agent_def.context_mode`, so those paths are byte-identical to before.
    pub spawn_context: Option<crate::agents::SpawnContext>,
    /// The parent transcript a `context=fork` spawn copies from, captured once
    /// per tool call by the caller. Required when `spawn_context` is
    /// [`crate::agents::SpawnContext::Fork`]; ignored otherwise. See
    /// [`fork::ForkSource`] for why the caller captures it rather than the
    /// spawner.
    pub fork_source: Option<fork::ForkSource>,
    /// Strict isolation mode (P3 Stage H). `None` = inherit parent's
    /// `HarnessDeps` (legacy / default). `Some(IsolationMode::Worktree)`
    /// will provision a detached-HEAD git worktree in Task 9.
    pub isolation: Option<crate::agents::IsolationMode>,
    /// Welded strategy `<strategy>` body inherited from the parent run.
    /// Injected into the inline `PromptBuilder` so the spawned agent shares
    /// the run-global strategy. `None` keeps the child prompt byte-identical.
    pub strategy: Option<&'a str>,
    /// Parent run's usage mode (chat / work / code). The child inherits the
    /// parent's mode-partitioned tool surface, so its prompt names the
    /// partition (`SessionMode::subagent_prompt_line`). `None` — and Work,
    /// which callers skip as the identity partition — keeps the child prompt
    /// byte-identical.
    pub session_mode: Option<crate::config::types::policies::SessionMode>,
    /// The caller's stable handle for this child, when it has one. Becomes the
    /// child's ephemeral session id (see [`ephemeral_for`]), which is what makes
    /// the durable `SubagentSpawned` / `SubagentReturned` pair addressable by
    /// the id the caller already holds.
    ///
    /// Set by the background `subagent` tool, whose `request_id` is the only
    /// handle the model ever sees. `None` for foreground / synchronous spawns:
    /// they deliver their result inline and have no id to correlate on, so the
    /// child key stays a bare nonce.
    ///
    /// **Do not** try to recover this correlation positionally from the parent
    /// log instead. A single turn can spawn several background children, so
    /// their `SubagentSpawned` events share a `turn_id` and cannot be told
    /// apart — the same parallel-batch ambiguity that broke the session-log
    /// scan `scoped::dispatch` replaced with an ambient call identity.
    pub request_id: Option<&'a str>,
}

/// The environment envelope a spawned sub-agent is given about *itself*.
///
/// # What was missing
///
/// A sub-agent's prompt threaded no [`ResolvedContext`] at all, so every layer
/// that reads one stayed silent: no `<environment_context>` (no cwd, no repo,
/// no branch, no model, no wall-clock), no `## Operating Envelope` (no
/// writable roots, no network posture), no sandbox posture in
/// `## Security & Constraints`. A child that runs `bash`, edits files and
/// browses a repo was told none of it — while the parent, running the same
/// tools against the same tree, was told all of it. The two hand-welds this
/// module still performs (`<strategy>` and the session-mode line) are the
/// scar tissue: both exist only because there was no resolved context for the
/// layers that own those facts to read.
///
/// # Why each field is set the way it is
///
/// * **`cwd`** — the worktree path for an isolated child (definitive: its exec
///   tools run under a [`WorktreeSandbox`](crate::sandbox::WorktreeSandbox)
///   rooted there), otherwise whatever
///   [`current_exec_workspace`](crate::sandbox::context::current_exec_workspace)
///   reports *at this point in the child's own call stack* — which is the same
///   value its `WorkspaceSandbox` will read when it jails a command. Reading
///   it here rather than accepting it as a parameter is the point: the two
///   cannot disagree, whatever the caller believes.
///
///   A detached background child used to reach this with nothing to report
///   (`EXEC_WORKSPACE` is a `tokio::task_local` and does not survive
///   `tokio::spawn`), and it was not only the prompt that suffered — its
///   sandbox jailed to an empty `workspaces/<hash>` directory too, so the
///   silence was accurate. [`crate::scope::CarriedAttribution`] now carries
///   the root across that spawn, so such a child both names and executes in
///   its parent's authorised root. When the value is genuinely absent (a
///   caller outside any run) the envelope still states **no** cwd: naming the
///   daemon's directory would recreate the exact defect the 2026-07-26 round
///   removed from the main path — a prompt that advertises a directory no tool
///   call lands in, followed by the model addressing absolute paths into it
///   and being refused by the jail. Silence is the only honest third answer,
///   which is why [`RuntimeContext::working_dir`] is an `Option`.
/// * **`parent` / `run_id`** — the first production writers either field has
///   ever had. Both were added with a renderer, a doc comment and tests on
///   both ends, and no producer, so `<parent kind="subagent">` and
///   `- Run id:` could not appear for any input. The parent session id lets a
///   child say *whose* explore run it is; the request id is the only handle
///   the model itself ever sees for a background spawn, so it is the one that
///   makes "this task of mine" addressable back to the parent.
/// * **`sandbox_summary`** — only for a worktree-isolated child, where this
///   module owns the sandbox and therefore knows the posture. A non-isolated
///   child inherits the parent's tool service and this function has no handle
///   on that sandbox; guessing one would put a posture in the prompt that the
///   gate does not enforce.
/// * **`session_mode` / `strategy`** — deliberately left unset even though the
///   caller knows both, because this module still welds them in by hand with
///   their own sub-agent-specific wording. Setting them here as well would
///   state each fact twice, which is the rule (§2.3 ③, one question one voice)
///   this whole round is enforcing elsewhere.
/// * **`approval_tier`** — asked of the enforcer, not derived. A child gets
///   no tool service of its own: it runs on the parent's
///   `ScopedToolService` (`parent_view_for_children`), which carries the tier
///   and the very same `PlanGate` `Arc`, so every call the child makes meets
///   the gate that would have met the parent's. `ToolService::enforced_exec_tier`
///   asks that object the identical question its own gate asks
///   (`effective_exec_tier`) — which is why this is not the "second
///   derivation that could disagree with the gate" an earlier round declined
///   to write, and why it is read here rather than snapshotted at spawn time:
///   a human releasing the plan gate mid-turn changes the answer, and the
///   child's prompt is built after that.
///
///   `None` (no tier wired — tests, direct callers) still states nothing.
///   The tier that made this load-bearing is
///   [`ExecTier::Plan`](crate::config::types::policies::ExecTier::Plan): it is
///   the one tier that REFUSES rather than asks, `subagent` is deliberately
///   reachable under it so a plan for a large codebase can be researched by
///   delegation, and a child told nothing spends its whole iteration budget
///   discovering by refusal what one sentence states.
fn child_environment_context(
    model: &str,
    worktree: Option<&std::path::Path>,
    parent_session_id: Option<&str>,
    request_id: Option<&str>,
    approval_tier: Option<crate::config::types::policies::ExecTier>,
) -> crate::thinker::context::ResolvedContext {
    use crate::thinker::context::EnvelopeParent;
    use crate::thinker::runtime_context::RuntimeContext;
    use crate::thinker::{InteractionManifest, InteractionParadigm};

    // Background: a sub-agent has no channel, no human watching its stream,
    // and no interactive rendering surface. It is the paradigm the harness
    // bridge already falls back to when no channel manifest is supplied.
    let paradigm = InteractionParadigm::Background;
    let mut ctx = crate::thinker::context::ContextAggregator::resolve(
        &InteractionManifest::new(paradigm),
        &crate::thinker::security_context::SecurityContext::for_paradigm(paradigm),
    );

    let cwd = worktree
        .map(std::path::Path::to_path_buf)
        .or_else(crate::sandbox::context::current_exec_workspace);
    ctx.runtime_context = Some(match cwd {
        Some(dir) => RuntimeContext::collect_in(model, Some(&dir)),
        None => RuntimeContext::collect_detached(model),
    });

    ctx.sandbox_summary =
        worktree.map(|path| crate::sandbox::SandboxSummary::isolated_worktree(path.to_path_buf()));

    ctx.envelope_parent = parent_session_id
        .filter(|id| !id.is_empty())
        .map(|id| EnvelopeParent {
            kind: "subagent".to_string(),
            id: id.to_string(),
        });
    ctx.run_id = request_id
        .filter(|id| !id.is_empty())
        .map(std::string::ToString::to_string);
    ctx.approval_tier = approval_tier;

    // A delegated child inside a project room is in the SAME room as its
    // parent: `RoomRosterLayer` sits in the pipeline both paths run, and the
    // scope this resolves from is one of the task-locals
    // [`crate::scope::CarriedAttribution`] already carries across every spawn.
    // Until 2026-08-25 nothing on this path read it, so the layer rendered
    // nothing here forever — no error, no red test, just a child that cannot
    // name a teammate.
    //
    // Read live rather than snapshotted at the call site for the reason
    // `approval_tier` is: `project_manage(action='member_add')` can change the
    // answer while the parent turn is still running, and this function runs
    // after that. Personal and org scopes resolve to `None`, which keeps every
    // single-human deployment's prompt byte-identical.
    ctx.room_roster = crate::thinker::layers::ambient_room_roster_line();
    ctx
}

/// Build a child ephemeral session, run the harness, and synthesize the
/// `LoopRunResult` by walking the child session event log.
///
/// Errors:
///   * `"chain depth exceeded"` — the parent's `ChainContext::child()`
///     returned `None` (hit the recursion cap).
///   * `"Sub-agent timed out after Ns"` — the outer `tokio::time::timeout`
///     elapsed before `AgentHarness::run` returned.
///   * `"sub-agent panicked: …"` — the harness task panicked.
///   * `"sub-agent failed: …"` — any other harness / session / tool error.
pub async fn spawn(base: &SpawnerBase, req: SpawnRequest<'_>) -> Result<LoopRunResult, String> {
    // 1. Derive a child chain; fail early if the recursion cap is hit so
    //    callers see the same "depth exceeded" signal the legacy path used.
    let child_chain = base
        .chain
        .child()
        .ok_or_else(|| "chain depth exceeded".to_string())?;

    // A2 — reserve a concurrency permit; held until `spawn` returns.
    //
    // W10 — this acquire is a park, and a park that ignores the cancel token
    // sets this sub-agent's cancel latency to the total runtime of everything
    // ahead of it in the queue: a queued child kept running its parent's
    // cancel out for minutes with nothing observable happening. Criteria §3
    // ("a tool that parks must listen to the cancellation token") applies to
    // the queue wait exactly as it does to the run. `biased` so an
    // already-cancelled token wins the race against a free permit.
    //
    // The error string is byte-exact on purpose:
    // `background_tracker::lifecycle_from_outcome` matches
    // `"sub-agent failed: cancelled"` by EQUALITY (any looser match would
    // misclassify a tool message that merely mentions cancelling), so any
    // other wording here settles the node as `Failed` instead of `Cancelled`.
    let _permit = match base.subagent_semaphore.as_ref() {
        Some(sem) => {
            let sem = sem.clone();
            let permit = tokio::select! {
                biased;
                () = req.cancel.cancelled() => {
                    return Err("sub-agent failed: cancelled".to_string());
                }
                permit = sem.acquire_owned() => permit
                    .map_err(|e| format!("sub-agent failed: subagent semaphore closed: {e}"))?,
            };
            Some(permit)
        }
        None => None,
    };

    // P3 Stage H — provision worktree if requested. The handle is held in the
    // outer scope so Drop fires as a safety net on cancel/panic/timeout/error.
    // Explicit cleanup happens on the success path (after harness completes Ok).
    let worktree_handle: Option<crate::sandbox::WorktreeHandle> = match req.isolation {
        Some(crate::agents::IsolationMode::Worktree) => {
            // B4-02: anchor on the run's project root, not the daemon's cwd.
            // `aleph-server` is a long-lived process whose cwd is wherever it
            // was launched (often `/` under the Tauri shell / launchd, the
            // operator's shell cwd otherwise) and has no relationship to the
            // active project. The main run path uses `current_project_root()`
            // to anchor `FsScope` (and this very function reads the same
            // value later for `session_write_id`, mod.rs:621), so the correct
            // anchor is already in scope.
            //
            // Fall back to cwd only when no project root is published (e.g.
            // operator ran the server outside any project). The fallback is
            // the existing behaviour, preserved to keep `aleph-server` startable
            // from anywhere — but the project's check_root_eq assertion below
            // makes the wrong-tree failure visible.
            let project_root = crate::projects::current_project_root();
            let repo_root = match project_root.as_deref() {
                Some(root) => root.to_path_buf(),
                None => tokio::task::spawn_blocking(std::env::current_dir)
                    .await
                    .map_err(|e| format!("sub-agent failed: cwd join: {e}"))?
                    .map_err(|e| format!("sub-agent failed: cwd: {e}"))?,
            };
            let label = &req.agent_def.id;
            let handle =
                crate::sandbox::worktree::create(&repo_root, label, base.trace_sink.clone())
                    .await
                    .map_err(|e| format!("sub-agent failed: worktree create: {e}"))?;
            Some(handle)
        }
        None => None,
    };

    // B4-03: refuse to provision `Inline` MCP specs at spawn time. They are
    // spawned and snapshotted into the tool catalog (`mcp_registrar::provision`),
    // but `McpScopedToolService::execute` forwards only to the parent — the
    // inline process was never registered in `PluginRegistry`, so the parent's
    // scoped service cannot dispatch to it. Result: the child LLM sees the
    // tool in its catalog, makes the call, the call bounces to
    // tool-not-found, the child burns its iteration budget retrying. Fail
    // loudly so the gap is visible at spawn instead of at the model's first
    // tool call. `Reference` specs remain unaffected — they reuse already-
    // registered servers and dispatch works end-to-end.
    if let Some(inline) = req
        .agent_def
        .mcp_servers
        .iter()
        .find_map(|spec| match spec {
            crate::agents::McpServerSpec::Inline { name, .. } => Some(name),
            _ => None,
        })
    {
        return Err(format!(
            "sub-agent failed: mcp scope: inline MCP server '{inline}' is not dispatchable — \
             McpScopedToolService forwards only to the parent tool registry, and inline \
             servers are not registered there. Use Reference or fix the routing."
        ));
    }

    // P3 Stage I — provision per-agent MCP scope. Held in outer scope so Drop
    // fires as a safety net on cancel/panic/timeout/error. Explicit
    // shutdown() happens on the success path (after harness completes Ok).
    let mcp_scope: Option<crate::extension::registrar::mcp_registrar::McpScope> = if !req
        .agent_def
        .mcp_servers
        .is_empty()
    {
        let registry = base.plugin_registry.as_ref().ok_or_else(|| {
                "sub-agent failed: mcp scope: SpawnerBase.plugin_registry is None but agent_def.mcp_servers is non-empty".to_string()
            })?;
        Some(
            crate::extension::registrar::mcp_registrar::McpScope::provision(
                req.agent_def,
                registry.clone(),
                base.trace_sink.clone(),
            )
            .await
            .map_err(|e| format!("sub-agent failed: mcp scope: {e}"))?,
        )
    } else {
        None
    };

    // P3 Stage H deepening — when a worktree is provisioned, publish a per-run
    // `FsScope` so the FILE tools (file_read / file_write / file_edit /
    // apply_patch / file_ops) resolve inside the worktree too, not just bash:
    // relative paths anchor at the worktree root and parent-repo absolute
    // paths are rebased into the checkout. Both ends are canonicalized so the
    // remap survives symlinked tmpdirs (macOS `/var` → `/private/var`).
    // Without a worktree the body runs un-wrapped, inheriting whatever scope
    // the parent run published — a non-isolated subagent intentionally shares
    // the parent's workspace.
    let fs_scope = match worktree_handle.as_ref() {
        Some(h) => {
            // `canonicalize()` can block on the filesystem; move it off the
            // async runtime thread. Failure to canonicalize is non-fatal — we
            // fall back to the raw worktree/repo paths.
            let wt = h.path().to_path_buf();
            let repo = h.repo_root().to_path_buf();
            let wt_for_blocking = wt.clone();
            let repo_for_blocking = repo.clone();
            let (wt, repo) = tokio::task::spawn_blocking(move || {
                let wt = wt_for_blocking.canonicalize().unwrap_or(wt_for_blocking);
                let repo = repo_for_blocking
                    .canonicalize()
                    .unwrap_or(repo_for_blocking);
                (wt, repo)
            })
            .await
            .unwrap_or((wt, repo));
            Some(crate::tools::fs_scope::FsScope::worktree(wt, repo))
        }
        None => None,
    };

    // P3 Stage H — command sandbox override: a worktree-isolated child runs its
    // exec tools (`bash` / `code_exec` / `code_check`) through a `WorktreeSandbox`
    // so commands execute at the worktree path with `CARGO_TARGET_DIR` redirected.
    // Installed as a task-local around `run_body` below (mirrors `fs_scope`);
    // `None` for a non-isolated child keeps the parent's shared sandbox.
    let sandbox_override: Option<Arc<dyn crate::sandbox::Sandbox>> =
        worktree_handle.as_ref().map(|h| {
            Arc::new(crate::sandbox::WorktreeSandbox::new(h.path().to_path_buf()))
                as Arc<dyn crate::sandbox::Sandbox>
        });

    let run_body = async {
        // 2. Unique ephemeral session key for this sub-agent.
        let child_id = ephemeral_for(&req.agent_def.id, req.request_id);

        // 3. Attach the child session and seed the initial Turn + UserMessage.
        //    Any failure here surfaces immediately — the harness never runs.
        base.session
            .attach(child_id.clone())
            .await
            .map_err(|e| format!("sub-agent failed: attach session: {e}"))?;

        // 3a. Resolve where this child's starting context comes from. The
        //     call's explicit choice wins; otherwise the agent definition's
        //     declared default — which is what every pre-knob caller gets, so
        //     their behaviour is unchanged.
        let spawn_context = req.spawn_context.unwrap_or_else(|| {
            crate::agents::SpawnContext::from_context_mode(&req.agent_def.context_mode)
        });

        // 3b. Build the agent-scoped system prompt.
        //
        //     Moved AHEAD of session seeding (it used to sit at step 4) because
        //     a fork's size ceiling is "the child's compaction warning line
        //     minus whatever the system block already occupies", and that
        //     second term is knowable exactly here. Sizing the fork against the
        //     whole window instead would seed a child that compacts on its
        //     first Think — paying an LLM to summarise history we just paid to
        //     copy. Pure function of `req` / `base`, so the move is safe.
        //
        //     `PromptBuilder::with_agent` pulls in the AgentRoleLayer;
        //     `build_system_prompt_parts(&[])` is fine — tool schemas are delivered
        //     via native tool_use, not the prompt. The descended `child_chain`
        //     is passed in so `ChainContextLayer` can tell the spawned agent it
        //     is nested and how much delegation budget remains.
        let resolved_model: Option<String> = req
            .model
            .map(str::to_string)
            .or_else(|| req.agent_def.model_hint.clone());
        let token_budget = base.context_budget_config.as_ref().map_or_else(
            crate::thinker::prompt_budget::TokenBudget::default,
            |cfg| {
                // Re-key the chain-minimum budget onto the model this child
                // will actually run on — the same refinement the main loop
                // applies (runner_impl), so a child pinned to a narrow model
                // no longer inherits the wider chain budget. Unknown models
                // and missing refiners fall back to `cfg` unchanged (the
                // refiner's own conservative default).
                let refined = match (&base.context_budget_refiner, &resolved_model) {
                    (Some(refiner), Some(model)) => refiner.refine_for_serving_model(
                        cfg,
                        model,
                        base.provider.name(),
                        base.primary_context_window,
                    ),
                    _ => cfg.clone(),
                };
                crate::thinker::prompt_budget::TokenBudget::from_context_window(
                    refined.token_budget,
                )
            },
        );
        // Seed the prompt token gate from the cross-run calibration
        // carry-over under the child's own model — the same factor the main
        // loop seeds its prompt gate with, so a model whose tokenizer the
        // char-ratio heuristic misjudges gets a gate calibrated to it here
        // too. Unknown / never-observed models stay at factor 1.0.
        let token_budget = match resolved_model
            .as_deref()
            .and_then(crate::orchestrator::harness_bridge::seeded_calibration_for_model)
        {
            Some(factor) => token_budget.with_estimate_factor(factor),
            None => token_budget,
        };
        let mut builder = PromptBuilder::new(PromptConfig {
            token_budget,
            ..PromptConfig::default()
        })
        .with_agent(req.agent_def.clone())
        .with_chain_context(child_chain.clone())
        .with_resolved_context(child_environment_context(
            resolved_model.as_deref().unwrap_or(base.provider.name()),
            worktree_handle.as_ref().map(|h| h.path()),
            base.parent_session_id.as_deref(),
            req.request_id,
            // The gate the child's calls will actually meet, asked of the
            // object that enforces it — `parent_tools` IS the service the
            // child's `AllowlistToolService` delegates every execution to.
            base.parent_tools.enforced_exec_tier(),
        ));
        if let Some(strategy) = req.strategy {
            builder = builder.with_strategy(strategy.to_string());
        }
        if let Some(mode) = req.session_mode {
            builder = builder.with_session_mode(mode);
        }
        // Split at the stable/dynamic boundary rather than handed over as one
        // undivided string. Everything the child now learns about *itself* —
        // cwd, model, hour, parent binding, run id, worktree — differs per
        // child, and an undivided block is cached (or not) whole: N members of
        // a fan-out would each write their own copy of the shared scaffold,
        // which is exactly the warmth `context=fork` exists to preserve.
        let system_prompt_parts = builder.build_system_prompt_parts(&[]);
        let system_prompt: String = system_prompt_parts
            .iter()
            .map(|p| p.content.as_str())
            .collect();

        // 3c. Fork: copy the parent's own recent transcript into the child
        //     BEFORE its task turn opens, so `build_prompt` — which walks the
        //     child log from index 0 exactly as it walks the parent's —
        //     reconstructs it through the same code path that produced it. That
        //     shared path is what makes the replay byte-identical, which is
        //     what makes the prefix cacheable.
        let fork_applied = match spawn_context {
            crate::agents::SpawnContext::Fork { turns } => {
                let parent_id = base
                    .parent_session_id
                    .as_deref()
                    .and_then(parent_session_id_of)
                    .ok_or_else(|| {
                        "sub-agent failed: context=fork has no parent session to fork from \
                         (this spawn site runs outside a conversation — use context=isolated \
                         or context=summary)"
                            .to_string()
                    })?;
                let source = req.fork_source.as_ref().ok_or_else(|| {
                    "sub-agent failed: context=fork reached the spawner with no captured \
                     parent transcript — the caller must snapshot it once per tool call \
                     (fork::snapshot) so every child of one fan-out forks the same instant"
                        .to_string()
                })?;
                let budget = fork::ForkBudget::for_child(
                    base.context_budget_config.as_ref(),
                    system_prompt.len(),
                    turns,
                )
                .ok_or_else(|| {
                    "sub-agent failed: context=fork cannot be sized — this run has no \
                     [context_budget], or the child's system prompt already fills its \
                     window. Use context=isolated or context=summary."
                        .to_string()
                })?;
                fork::seed(
                    base.session.as_ref(),
                    &parent_id,
                    &child_id,
                    source,
                    &budget,
                )
                .await?
            }
            _ => None,
        };

        let turn = uuid::Uuid::new_v4();
        base.session
            .emit_event(
                &child_id,
                SessionEvent::TurnStarted {
                    turn_id: turn,
                    trigger: TurnTrigger::SubagentRequest,
                    at: now_ms(),
                },
            )
            .await
            .map_err(|e| format!("sub-agent failed: emit TurnStarted: {e}"))?;

        let mut effective_task = build_effective_task(req.context_summary, spawn_context, req.task);
        // The fork receipt rides on the same message as the task, so the child
        // reads what it is looking at and what it is being asked together —
        // rather than inferring an objective from a window that may start in
        // the middle of one.
        if let Some(note) = fork_applied.as_ref().and_then(fork::ForkPlan::receipt) {
            effective_task = format!("{note}\n\n{effective_task}");
        }
        // VESR v1.1 (b) — capture this subagent's run under its own agent_id by
        // wrapping the child trace sink with a dedicated OutcomeObserver.
        // Subagents bypass the top-level wrap (runner_impl.rs), so we mirror it
        // here at the spawn seam. Borrows `effective_task` before it moves into
        // the UserMessage below. `None` store / `None` sink → child keeps the
        // raw sink (today's behavior, no capture).
        let child_trace_sink: Option<Arc<dyn crate::harness::TraceSink>> = match (
            base.trace_sink.as_ref(),
            base.routing_store.as_ref(),
        ) {
            (Some(sink), Some(store)) => {
                // Attribute from spawn-seam directives only (the parent's
                // resolved model/provider are not reachable here). Explicit
                // model choice = the high-value routing case; full
                // inheritance → "(dynamic)".
                let child_model = req
                    .model
                    .map(str::to_string)
                    .or_else(|| req.agent_def.model_hint.clone())
                    .unwrap_or_else(|| "(dynamic)".to_string());
                let child_provider = req
                    .agent_def
                    .provider_hint
                    .clone()
                    .unwrap_or_else(|| "(dynamic)".to_string());
                match store.embed_task(&effective_task).await {
                    Ok(task_emb) => {
                        let attribution = Arc::new(crate::routing::RoutingAttribution::new(
                            child_id.to_key_string(),
                        ));
                        let _ = attribution.task_emb.set(task_emb);
                        Some(Arc::new(crate::routing::OutcomeObserver::new(
                            sink.clone(),
                            store.clone(),
                            attribution,
                            child_model,
                            child_provider,
                            req.agent_def.id.clone(),
                        ))
                            as Arc<dyn crate::harness::TraceSink>)
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "subagent routing embed failed; capture skipped");
                        base.trace_sink.clone()
                    }
                }
            }
            _ => base.trace_sink.clone(),
        };
        base.session
            .emit_event(
                &child_id,
                SessionEvent::UserMessage {
                    turn_id: turn,
                    content: MessageContent {
                        text: effective_task,
                        blocks: Vec::new(),
                        thinking: None,
                        thinking_signature: None,
                    },
                    at: now_ms(),
                    synthetic: false,
                    author_user_id: None,
                },
            )
            .await
            .map_err(|e| format!("sub-agent failed: emit UserMessage: {e}"))?;

        // Emit SubagentSpawned to the parent session. durably-recovered
        // background sub-agents rely on this event reaching the session log
        // — silently dropping an emit failure would make `check_status` /
        // `wait` return "No background sub-agent found" after a restart, so
        // log the failure to at least make it diagnosable.
        let child_key = child_id.clone();
        let flow_name = req.agent_def.id.clone();
        if let Some(ref parent_str) = base.parent_session_id {
            if let Some(parent_id) = parent_session_id_of(parent_str) {
                if let Err(e) = base
                    .session
                    .emit_event(
                        &parent_id,
                        SessionEvent::SubagentSpawned {
                            turn_id: turn,
                            child_id: child_key.clone(),
                            flow: flow_name.clone(),
                            at: now_ms(),
                        },
                    )
                    .await
                {
                    tracing::error!(
                        error = %e,
                        parent_id = %parent_id,
                        child_id = %child_key,
                        "subagent_spawner: SubagentSpawned emit_event failed; durable recovery may not find this sub-agent"
                    );
                }
            }
        }

        // 5. Resolve the model override: explicit > model_hint > native.
        let llm: Arc<dyn AiProvider> = match resolved_model {
            Some(m) => Arc::new(crate::providers::ModelOverrideProvider::new(
                base.provider.clone(),
                m,
            )),
            None => base.provider.clone(),
        };
        // Stage J-pre: wrap with MeteringProvider so every LLM call from this
        // subagent emits a LoopTraceEvent::ProviderUsage labelled with the
        // subagent's agent_def.id (the top-level wrap site in
        // harness_bridge/runner_impl.rs now labels with its own `spec.agent`
        // for the same reason).
        let llm: Arc<dyn AiProvider> = Arc::new(crate::providers::MeteringProvider::new(
            llm,
            base.trace_sink.clone(),
            req.agent_def.id.clone(),
        ));

        // 6. Wrap the parent's tool service with the allowlist gate.
        // P3 Stage I — if an McpScope was provisioned, layer its tools UNDER
        // AllowlistToolService so the allowlist gate remains the authority on
        // what the child harness can call.
        let agent_def_arc = Arc::new(req.agent_def.clone());
        let parent_tools_with_scope: Arc<dyn ToolService> = match mcp_scope.as_ref() {
            Some(scope) => Arc::new(crate::tools::mcp_scope_view::McpScopedToolService::new(
                base.parent_tools.clone(),
                scope.tools(),
            )),
            None => base.parent_tools.clone(),
        };
        let scoped_tools: Arc<dyn ToolService> = Arc::new(AllowlistToolService::new(
            parent_tools_with_scope,
            agent_def_arc.clone(),
        ));

        // B15 — the spawn path used to hand `AgentDef.max_iterations` straight
        // to `HarnessDeps`, where `None` means *unbounded* (deps.rs). The
        // built-in "default" role — where the model lands when it omits
        // `agent_type` — declares no cap, so such a child looped until the
        // wall-clock spawn timeout killed it and threw the whole run away with
        // an error string. An iteration cap instead fires the harness's
        // boundary grace turn, which returns a usable summary. Resolved through
        // `resolve_max_iterations` rather than a bare `.or(...)` so a
        // configured `0` (frontmatter or `[execution]`) can never degrade into
        // `Some(0)` = "die after one turn".
        let max_iter = Some(crate::orchestrator::harness_bridge::resolve_max_iterations(
            None,
            req.agent_def.max_iterations,
            base.default_max_iterations.unwrap_or(0),
        ));

        // The cheap summarizer is built raw (`deps_builder::summary`), so
        // without this wrap its spend emitted no `ProviderUsage` at all —
        // invisible to the traces DB, Panel Usage and team rollups. Labelled
        // `compactor:<agent>` so rollups can tell compression spend from turn
        // spend. The main `llm` above is already metered (Stage J-pre);
        // wrapping that one again would double-count.
        // rust-doctor-disable-next-line excessive-clone
        let metered_cheap: Option<Arc<dyn AiProvider>> =
            base.cheap_summary_provider.as_ref().map(|cheap| {
                Arc::new(crate::providers::MeteringProvider::new(
                    cheap.clone(),
                    // rust-doctor-disable-next-line excessive-clone
                    base.trace_sink.clone(),
                    format!("compactor:{}", req.agent_def.id),
                )) as Arc<dyn AiProvider>
            });
        let (context_budget, context_compactor, preflight_pipeline) = build_context_triple(
            base.context_budget_config.as_ref(),
            &llm,
            metered_cheap.as_ref(),
            &req.agent_def.id,
            &child_id,
            // rust-doctor-disable-next-line excessive-clone
            req.cancel.clone(),
        );

        // Layer-3 per-turn aggregate budget — derive from `context_budget_config`
        // when the parent has one wired, otherwise fall back to the
        // process-wide singleton (mirrors the root runner's last-resort
        // branch). Without this, subagent tool results only see Layer 2
        // (per-message) caps; large bash/file outputs cannot spill to disk
        // and the subagent reads a truncated result.
        let turn_budget: Option<Arc<crate::tools::turn_budget::TurnResultBudget>> = base
            .context_budget_config
            .as_ref()
            .map(|cfg| {
                let (_, per_turn) = crate::tools::turn_budget::budget_for_window(cfg.token_budget);
                Arc::new(crate::tools::turn_budget::TurnResultBudget::new(per_turn))
            })
            .or_else(crate::tools::turn_budget::global_turn_result_budget);

        let deps = HarnessDeps {
            session: base.session.clone(),
            tools: scoped_tools,
            llm,
            robustness_profile: crate::verification::ModelRobustnessProfile::conservative(),
            // Forward the parent's verifier chain so a subagent that enters
            // a tool-call death loop is caught by the same structural
            // watchdog as the parent (ToolLoopVerifier, StopHookVerifier,
            // etc.). Falling back to None preserves the pre-2026-09 behaviour
            // for callers that don't thread a chain through SpawnerBase —
            // the iteration cap (`resolve_max_iterations`) is the last
            // line of defence in that case.
            verifier_chain: base.verifier_chain.clone(),
            context_budget,
            context_compactor,
            preflight_pipeline,
            // Stage A (P1) — inherited from parent SpawnerBase. VESR v1.1 (b):
            // when routing capture is on, this is the child's own OutcomeObserver
            // wrapping that sink; otherwise the raw sink (unchanged).
            trace_sink: child_trace_sink,
            // Both are set. `HarnessDeps` documents that the two travel
            // independently — the harness does NOT derive one from the other —
            // so an adapter that reads only the legacy field still sees the
            // whole prompt, while the Anthropic adapter places the cache
            // breakpoint at the boundary the parts describe.
            system_prompt: Some(system_prompt),
            system_prompt_parts: Some(system_prompt_parts),
            recall_context: None,
            // Stage 5a (#9): inherit parent guardrails so the subagent enforces
            // the same Input/Output/ToolCall checks as the spawning harness.
            guardrails: base.guardrails.clone(),
            max_iterations: max_iter,
            power: None,
            // Stage A (P1) — was None for all three; now inherited from parent.
            stall_config: base.stall_config.clone(),
            consecutive_failure_cap: base.consecutive_failure_cap,
            turn_timeout: base.turn_timeout,
            turn_budget,
            // §3.2 overflow-tier parity: was `None`, so a subagent's oversized
            // tool results were truncated inline (the subagent then re-ran the
            // tool against truncated context). Reuse the same shared
            // `ToolResultStore` singleton the main harness falls back to
            // (`orchestrator::harness_bridge`), so large subagent results spill
            // to disk and the subagent can re-read the marker.
            //
            // Scope the *handle* like the two other seams do
            // (`tool_service_builder`, `harness_bridge::runner_impl`) — the
            // process-wide handle was unscoped, so a child's Layer-3 spills
            // landed outside every session directory and its own `ctx_search`
            // (which resolves scope from `turn_context::current_session_key()`)
            // could never find them. Failed safe, but the recall was dead.
            //
            // The key is the PARENT session, not `child_id`: a subagent runs its
            // tools through the parent's `ScopedToolService`, so both the
            // TURN_CONTEXT its `ctx_search` reads and the Layer-2 store its
            // individual results spill to are already parent-scoped. Scoping
            // Layer 3 to `child_id` would just move the artifacts to a third
            // directory nothing reads. No parent session (direct/test callers,
            // no ScopedToolService) → keep today's unscoped handle.
            result_store: crate::tools::result_store::global_tool_result_store().map(|store| {
                base.parent_session_id.as_ref().map_or_else(
                    || store.clone(),
                    |sid| {
                        crate::tools::result_store::ToolResultStore::for_session(
                            &store,
                            sid.clone(),
                        )
                    },
                )
            }),
            session_epoch_registrar: None,
            // D6 — per-tool-invocation signal capture, mirroring the main path
            // (`harness_bridge::runner_impl`). This was a hardcoded Noop even
            // though `raw_memory_writer` is `Some` in production (the gateway
            // wires it, and the Delegation emit below already uses it), so every
            // tool call a subagent made was invisible to the `insights.tools`
            // RPC — the sink's only real consumer. Tagged with the sub-role's
            // `agent_def.id` (not the parent's) so per-role tool stats attribute
            // to the role that actually ran the tool, matching the `routing_store`
            // precedent above. No writer (tests / legacy callers) → Noop.
            tool_signal_sink: match base.raw_memory_writer.clone() {
                Some(store) => Arc::new(crate::memory::tool_signal_sink::RawMemoryToolSink::new(
                    store,
                    // The sub-role's id is the BASE; the partition it files
                    // into still has to carry the session's scope, exactly as
                    // `harness_bridge::runner_impl` does for the parent —
                    // otherwise a member's sub-agent tool failures pool into
                    // the org partition every principal can read.
                    //
                    // `project_scoped: false` here is deliberate and not a
                    // second dial: this crate has no `MemoryConfig` handle, and
                    // the flag only reaches `session_write_id`'s NO-session-
                    // scope arm — so passing `false` is byte-identical to
                    // today's uncomposed id for every unscoped run, and correct
                    // for every scoped one. We are inside the parent run's
                    // task-local nest here, so `current_scope()` is live.
                    crate::memory::project_scope::session_write_id(
                        &req.agent_def.id,
                        false,
                        crate::projects::current_project_root().as_deref(),
                    ),
                    child_id.to_key_string(),
                ))
                    as Arc<dyn crate::memory::tool_signal_sink::ToolSignalSink>,
                None => Arc::new(crate::memory::tool_signal_sink::NoopToolSignalSink)
                    as Arc<dyn crate::memory::tool_signal_sink::ToolSignalSink>,
            },
            in_flight_tool_calls: None,
            // Parity with the main gateway harness (was `None` — subagents ran
            // every tool batch serially). Subagents routinely fan out
            // independent reads/searches; the Act phase only parallelizes
            // concurrent-safe calls (writes/exec/send still serialize via the
            // resource-scope partitioner in `tools::concurrency`), so this is a
            // safe throughput win, not a correctness change. Prefer the
            // runner's CONFIGURED `[tool_service] parallel_tool_concurrency`
            // (threaded via `base`; 0/1 disables the fast path) — the previous
            // hardcoded config default silently ignored an operator's setting,
            // so "disable parallel dispatch" only ever applied to the main
            // harness while subagents kept running batches 8-wide.
            parallel_tool_concurrency: Some(
                base.parallel_tool_concurrency
                    .unwrap_or_else(crate::config::types::tools::default_parallel_tool_concurrency),
            ),
        };
        let harness = Arc::new(AgentHarness::new(deps));

        // 7. Run the harness with wall-clock timeout + panic isolation.
        //    AssertUnwindSafe is used because the harness internals (provider
        //    closures, channels) are not `UnwindSafe` but we intentionally
        //    catch panics to synthesize a clean error rather than unwind
        //    into the parent actor.
        //
        //    The harness is held via `Arc` so we retain a handle after the
        //    async closure completes — this lets us query `hit_limit()`
        //    directly instead of reconstructing it from the event log.
        let timeout = std::time::Duration::from_secs(req.timeout_secs);
        let cancel = req.cancel.clone();
        let sid = child_id.clone();
        let harness_for_run = harness.clone();
        let run_fut = async move {
            let mut cb = NoopHarnessCallback;
            harness_for_run.run(&sid, &mut cb, &cancel).await
        };
        let outcome = tokio::time::timeout(timeout, AssertUnwindSafe(run_fut).catch_unwind()).await;

        match outcome {
            Err(_elapsed) => Err(format!("Sub-agent timed out after {}s", req.timeout_secs)),
            Ok(Err(panic_payload)) => {
                let msg = crate::utils::panic_payload::panic_message(&*panic_payload);
                Err(format!("sub-agent panicked: {msg}"))
            }
            Ok(Ok(Err(e))) => Err(format!("sub-agent failed: {e}")),
            Ok(Ok(Ok(()))) => {
                // 8. Query the harness directly for the `hit_limit` and
                //    `total_tokens` signals. The previous implementation
                //    reconstructed these from the event log because the harness
                //    had been moved into the async closure; with
                //    `Arc<AgentHarness>` we just read the flags.
                let hit_limit = harness.hit_limit();

                let total_tokens = harness.total_tokens();
                let result =
                    extract_run_result(base.session.as_ref(), &child_id, hit_limit, total_tokens)
                        .await?;

                // Emit SubagentReturned to the parent session. Same
                // rationale as SubagentSpawned above: durably-recovered
                // background sub-agents need this event in the session
                // log; silently dropping an emit failure would make the
                // sub-agent look "still running" forever after a restart.
                let summary = result.final_text.clone().unwrap_or_default();
                if let Some(ref parent_str) = base.parent_session_id {
                    if let Some(parent_id) = parent_session_id_of(parent_str) {
                        if let Err(e) = base
                            .session
                            .emit_event(
                                &parent_id,
                                SessionEvent::SubagentReturned {
                                    turn_id: turn,
                                    child_id: child_id.clone(),
                                    summary: summary.clone(),
                                    at: now_ms(),
                                },
                            )
                            .await
                        {
                            tracing::error!(
                                error = %e,
                                parent_id = %parent_id,
                                child_id = %child_id,
                                "subagent_spawner: SubagentReturned emit_event failed; durable recovery may not find this sub-agent"
                            );
                        }
                    }
                }

                // 9. Spec 1 G2 — fire-and-forget Delegation emit so CompressionService
                //    can distil parent-side lessons. Skipped silently when no writer is
                //    threaded through (legacy callers, tests, off-by-config).
                if let Some(writer) = base.raw_memory_writer.clone() {
                    let summary = result.final_text.clone().unwrap_or_default();
                    let parent_id = base
                        .parent_agent_id
                        .clone()
                        .unwrap_or_else(|| "default".to_string());
                    crate::a2a::sub_agent::emit_delegation_primitives(
                        writer,
                        req.task.to_string(),
                        summary,
                        parent_id,
                        base.parent_session_id.clone(),
                        req.agent_def.id.clone(),
                        base.capture_registry.clone(),
                    );
                }

                Ok(result)
            }
        }
    };
    let body = crate::sandbox::context::with_sandbox_override(sandbox_override, run_body);
    let result: Result<LoopRunResult, String> = match fs_scope {
        Some(scope) => crate::tools::fs_scope::with_fs_scope(Some(scope), body).await,
        None => body.await,
    };

    // P3 Stage I — explicit MCP scope shutdown on the success path. Errors and
    // cancels leak the scope to the Drop safety net (which logs `leaked: true`).
    if result.is_ok() {
        if let Some(scope) = mcp_scope {
            if let Err(e) = scope.shutdown().await {
                tracing::error!(
                    error = %e,
                    "subagent mcp scope shutdown failed; Drop safety net will retry"
                );
            }
        }
    }

    // P3 Stage H — explicit cleanup on the success path. Errors and cancels
    // leak the handle to the Drop safety net (which logs `leaked: true` via
    // TraceSink).
    if result.is_ok() {
        if let Some(h) = worktree_handle {
            if let Err(e) = h.cleanup().await {
                tracing::error!(
                    error = %e,
                    "subagent worktree cleanup failed; Drop safety net will retry"
                );
            }
        }
    }

    result
}

/// B5 — assemble the child's seed task.
///
/// A `context_summary` is prepended only under [`SpawnContext::Summary`]. Under
/// [`SpawnContext::Isolated`] it is dropped — that is the whole point of the
/// mode — and under [`SpawnContext::Fork`] it is redundant: the child is about
/// to be handed the parent's actual transcript, and a précis of it sitting on
/// top would be a second, lossier account of the same events, contradicting the
/// first wherever they disagree.
///
/// A dropped summary is always *announced*, never erased. The tool schema
/// advertises `context_summary` unconditionally, so a caller can and does
/// supply one for a target that will not use it; the model composed 2 KB of
/// context, the child received the bare task, and the answer came back
/// off-target with nothing to correlate it to. The note names the mode that
/// dropped it and the argument that would have kept it, so the fix is one
/// re-issue away instead of a mystery.
fn build_effective_task(
    context_summary: Option<&str>,
    spawn_context: crate::agents::SpawnContext,
    task: &str,
) -> String {
    use crate::agents::SpawnContext;
    match (context_summary, spawn_context) {
        (Some(summary), SpawnContext::Summary) => {
            format!("## Context from parent agent\n\n{summary}\n\n---\n\n{task}")
        }
        (Some(_), SpawnContext::Isolated) => format!(
            "{task}\n\n[context_summary supplied by caller but ignored: this spawn ran with \
             context=isolated (the target agent's default is context_mode=Fresh unless the \
             call said otherwise). Pass context=\"summary\" to have it delivered.]"
        ),
        (Some(_), SpawnContext::Fork { .. }) => format!(
            "{task}\n\n[context_summary supplied by caller but ignored: this spawn ran with \
             context=fork, so the parent's actual transcript is above and a summary of it \
             would only compete with it.]"
        ),
        (None, _) => task.to_string(),
    }
}

/// Walk the child session event log and synthesize a `LoopRunResult`.
///
/// `iterations` := count of `AssistantMessage` events.
/// `tool_calls_made` := count of `ToolCallRequested` events.
/// `final_text` := text of the last `AssistantMessage`, or `None`.
/// `hit_limit` := passed in by the caller (sourced from
///                 `AgentHarness::hit_limit()` after the run); surfaced to the
///                 parent model as `hit_iteration_limit` by the subagent tool.
/// `total_tokens` := passed in by the caller (sourced from
///                 `AgentHarness::total_tokens()` after the run).
async fn extract_run_result(
    session: &dyn SessionService,
    child_id: &SessionId,
    hit_limit: bool,
    total_tokens: u64,
) -> Result<LoopRunResult, String> {
    let all = session
        .get_events(child_id, None, None)
        .await
        .map_err(|e| format!("sub-agent failed: read events: {e}"))?;

    // Count only what THIS child did.
    //
    // The log is no longer guaranteed to start with the child's own work: a
    // `context=fork` spawn seeds it with a verbatim copy of the parent's
    // transcript first (`fork::seed`). Walking from index 0 would charge the
    // parent's assistant turns and tool calls to the child — a one-turn child
    // forked off twelve parent turns reporting `iterations: 13` — and, on the
    // path that matters, a child that produced **no** assistant message of its
    // own (immediate error, cancelled before its first Think) would hand back
    // *the parent's last answer* as its finding. A sub-agent quoting the
    // question back as its result, with a success shape, is worse than an
    // error.
    //
    // `reduction::own_work_start` is that boundary, shared with the recovery
    // read path (`subagent_tool::recovery::resolve_forgotten`) so the two faces
    // of "what did this child itself do" cannot drift apart. It reads the
    // `SessionForked` marker `fork::seed` writes plus the `TurnStarted` this
    // module emits right after it; with no fork the boundary is index 0 —
    // byte-identical to the previous behaviour.
    let events = &all[crate::session::reduction::own_work_start(&all)..];

    let mut iterations: usize = 0;
    let mut tool_calls_made: usize = 0;
    let mut final_text: Option<String> = None;
    for rec in events {
        match &rec.event {
            SessionEvent::AssistantMessage { content, .. } => {
                iterations = iterations.saturating_add(1);
                // Keep the most recent assistant text as the "final" answer.
                if !content.text.is_empty() {
                    final_text = Some(content.text.clone());
                } else if is_last_assistant(events, rec) {
                    // Edge case: the *last* AssistantMessage is pure tool_use
                    // (no text) — the run ended mid-work (typically a capped
                    // run, `hit_limit=true`). Clear any earlier textual answer
                    // so stale mid-run narration is never presented as the
                    // final result; the subagent tool surfaces the cap via
                    // `hit_iteration_limit` instead. The dedicated
                    // `final_text_cleared_when_…` regression test below
                    // asserts this behavior.
                    final_text = None;
                }
            }
            SessionEvent::ToolCallRequested { .. } => {
                tool_calls_made = tool_calls_made.saturating_add(1);
            }
            _ => {}
        }
    }

    Ok(LoopRunResult {
        final_text,
        iterations,
        tool_calls_made,
        total_tokens: total_tokens as usize,
        hit_limit,
    })
}

/// Generate a unique ephemeral `SessionKey` for this sub-agent spawn.
/// Mint the child's ephemeral session key.
///
/// Two shapes, and the difference is load-bearing:
///
/// * `Some(request_id)` — a **background** child, whose `request_id` is the
///   only handle the model gets back and therefore outlives the call. The key
///   becomes `sub-bg-<request_id>`, which makes the durable
///   `SubagentSpawned { child_id }` / `SubagentReturned { child_id, summary }`
///   pair in the parent's log addressable by that id after a restart. This is a
///   contract, not a formatting choice — see
///   [`crate::agents::subagent_tool::recovery`], and
///   `child_key_roundtrips_through_the_request_id` is the test that goes red
///   first if the shape changes.
/// * `None` — a foreground / batch / MoA-aggregator child, which delivers its
///   result inside the tool call that spawned it. Keeps the historical
///   `sub-<nonce>` shape.
///
/// **The two prefixes must stay distinct.** Every spawn writes the same durable
/// events regardless of shape, so if uncorrelated children also carried the
/// background prefix, `recovery::enumerate` would read each one's random nonce
/// as an unrecoverable `request_id` and `subagent list` would fill up with every
/// foreground sub-agent the session ever ran — a directory lying in the opposite
/// direction from the one this recovery path exists to fix.
fn ephemeral_for(agent_id: &str, request_id: Option<&str>) -> SessionKey {
    let ephemeral_id = match request_id {
        Some(rid) => format!("{SUBAGENT_BG_CHILD_PREFIX}{rid}"),
        None => format!("{ANON_CHILD_PREFIX}{}", uuid::Uuid::new_v4()),
    };
    SessionKey::Ephemeral {
        agent_id: agent_id.to_string(),
        ephemeral_id,
    }
}

/// Public face of the background half of [`ephemeral_for`]: the session key a
/// background child's transcript persists under. The tracker stamps this into
/// `SubagentNode.child_session` so clients receive the address instead of
/// re-deriving its shape — a client-side derivation would be a second source
/// of truth that rots the day this format moves.
#[must_use]
pub fn background_child_session_key(agent_id: &str, request_id: &str) -> SessionKey {
    ephemeral_for(agent_id, Some(request_id))
}

/// Prefix for a **background** child's session key, whose suffix is the
/// caller's `request_id`. Single source shared by the minting side
/// ([`ephemeral_for`]) and the recovery side
/// ([`crate::agents::subagent_tool::recovery`]); two literals would be two
/// answers to one question.
pub const SUBAGENT_BG_CHILD_PREFIX: &str = "sub-bg-";

/// Prefix for a child with no caller-side handle. Its suffix is a bare nonce
/// and addresses nothing.
const ANON_CHILD_PREFIX: &str = "sub-";

/// Interpret a `parent_session_id` string as the session the `SubagentSpawned` /
/// `SubagentReturned` events for its children are written to.
///
/// This is the *emitter's* reading, kept here beside the two `emit_event` calls
/// that use it so the recovery reader cannot drift into a second one. A reader
/// that disagreed about which session holds the events would find an empty log
/// and report "unknown" forever — a silent failure with no error path.
///
/// B4-01: the previous body JSON-parsed the value with
/// `serde_json::from_str::<SessionId>(raw).ok()`, but `SessionId` is an
/// internally tagged enum and the only production caller passes a flat
/// key-string from `SessionKey::to_key_string()` (e.g. `"agent:main:main"`),
/// which is not a JSON document. Every production spawn therefore returned
/// `None`, both `emit_event` guards skipped, and the recovery path's
/// `SubagentSpawned`/`SubagentReturned` events were never written — leaving
/// the event-log recovery reader to find an empty log and report the work
/// as unknown forever. Use the existing flat-string reader.
#[must_use]
pub fn parent_session_id_of(raw: &str) -> Option<SessionId> {
    SessionKey::parse(raw)
}

/// Build a child's context-management triple — budget, LLM compactor, cheap-pass
/// preflight pipeline — from the runner's `[context_budget]` config.
///
/// All three used to be hardcoded `None` here (the only fields in the
/// `HarnessDeps` literal with no comment saying why), so a subagent ran with NO
/// context management at all: `build_prompt` replays the whole child log every
/// turn, nothing ever compacted it, and when the provider finally answered
/// `prompt_too_long` the reactive drain found no compactor, marked the rescue
/// exhausted and killed the run. Read-heavy research children are exactly the
/// ones that hit that wall.
///
/// The child builds its OWN instances, never the parent's: `ContextBudget`
/// carries per-run tokenizer calibration and circuit-breaker counters, and the
/// compactor must summarise through the CHILD's provider — sharing either would
/// cross-contaminate a parent and its concurrently-running children.
///
/// All-or-nothing by construction, which is the gating `HarnessDeps` documents
/// (`preflight_pipeline` is `None` exactly when `context_compactor` is): a
/// compactor without a preflight pipeline would pay for LLM summarisation where
/// free structural pruning was available.
type ContextTriple = (
    Option<Arc<tokio::sync::Mutex<crate::context::budget::ContextBudget>>>,
    Option<Arc<crate::context::compact::compactor::ContextCompactor>>,
    Option<Arc<crate::context::budget::preflight::PreflightPipeline>>,
);

/// `agent_id` + `child_id` scope the compactor's cache-watchdog reset to this
/// sub-agent's own conversation.
///
/// Without it the compactor calls `notify_compaction(None)`, the process-wide
/// reset — so the moment any one sub-agent compacts (routine on a long,
/// tool-heavy child run) every *other* agent's consecutive-miss streak is
/// zeroed too, including the parent's. The watchdog needs 3 consecutive armed
/// misses to warn, which in a busy swarm it would then never reach: the single
/// early-warning signal for prefix breakage is disarmed precisely in the
/// multi-agent runs where prompt-cache spend is highest. The scoped reset it
/// mirrors is `runner_impl.rs`'s on the root path — and it must be scoped the
/// same way, since a fan-out spawns many children of the SAME agent id whose
/// prefixes are entirely independent.
///
/// `cheap_summary` is the root runner's flash-tier summarizer, inherited for the
/// same reason the budget config is: this is the *second* construction site of
/// the same object, and it started life with none of the first one's tiering.
///
/// **Two builders on `ContextCompactor` are deliberately NOT called here**, so
/// nobody "completes the set" later without re-deriving why:
///
/// - `with_cache_carryover` — the carry-over slot holds 16 sessions and evicts
///   least-recently-written. Child sessions are overwhelmingly one-run and each
///   gets a fresh id, so seeding them would push a flood of single-use keys
///   through a 16-slot cache whose entire purpose is keeping the long-lived
///   interactive session hot. That is the exact eviction pathology the slot's
///   LRU-on-write ordering was chosen to prevent; feeding it from a fan-out
///   would defeat the feature for the run that benefits most.
/// - `with_summary_reuse` — reuse reads the hierarchical session summaries
///   `SessionCompactor` accumulates over a conversation's life. A child session
///   is born empty and dies within the spawn, so there is nothing to reuse; the
///   wiring would be a lookup that always misses.
fn build_context_triple(
    cfg: Option<&crate::context::budget::ContextBudgetConfig>,
    llm: &Arc<dyn AiProvider>,
    cheap_summary: Option<&Arc<dyn AiProvider>>,
    agent_id: &str,
    child_id: &SessionId,
    // The child's own stop signal. Required, not optional: this is the SECOND
    // construction site for a compactor, and the first one already learned that
    // a compaction awaited outside any `select!` burns its full 15 s timeout on
    // a cancelled turn and then commits the result. A defaulted parameter here
    // would let this site inherit that silently — and a subagent is exactly
    // where nobody would notice, because the parent is already waiting.
    cancel: CancellationToken,
) -> ContextTriple {
    use crate::context::compact::compactor::{CompactorConfig, ContextCompactor};
    let Some(cfg) = cfg else {
        return (None, None, None);
    };
    let budget = Arc::new(tokio::sync::Mutex::new(
        crate::context::budget::ContextBudget::new(cfg),
    ));
    let compactor = Arc::new(
        ContextCompactor::new(
            llm.clone(),
            CompactorConfig {
                fresh_tail: cfg.fresh_tail_count,
                summarizer_input_budget: cfg.summarizer_input_budget,
                ..CompactorConfig::default()
            },
        )
        .with_cancel(cancel)
        .with_monitor_scope(crate::thinker::prompt_builder::cache_monitor::cache_scope(
            agent_id,
            Some(&child_id.to_key_string()),
        ))
        // Same flash-tier summarizer the root runner uses. Without it this
        // second construction site quietly billed the main reasoning model for
        // every child compaction — worst precisely in a fan-out, where the
        // count is per child.
        .with_cheap_provider(cheap_summary.cloned()),
    );
    let pipeline = Arc::new(crate::context::budget::preflight::default_pipeline(cfg));
    (Some(budget), Some(compactor), Some(pipeline))
}

/// Whether `target` is the last `AssistantMessage` in `events` (by seq).
fn is_last_assistant(events: &[SessionEventRecord], target: &SessionEventRecord) -> bool {
    events
        .iter()
        .rev()
        .find(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
        .is_some_and(|r| r.seq == target.seq)
}

#[cfg(test)]
mod tests;
