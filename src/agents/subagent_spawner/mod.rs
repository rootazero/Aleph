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
    /// The parent runner's cheap-tier summarization provider. Handed to the
    /// child's `ContextCompactor` so its side-channel summarization bills the
    /// operator's flash sibling, not the main reasoning model. `None` keeps the
    /// child summarizing on its own LLM.
    pub cheap_summary_provider: Option<Arc<dyn AiProvider>>,
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
    let _permit = match base.subagent_semaphore.as_ref() {
        Some(sem) => Some(
            sem.clone()
                .acquire_owned()
                .await
                .map_err(|e| format!("sub-agent failed: subagent semaphore closed: {e}"))?,
        ),
        None => None,
    };

    // P3 Stage H — provision worktree if requested. The handle is held in the
    // outer scope so Drop fires as a safety net on cancel/panic/timeout/error.
    // Explicit cleanup happens on the success path (after harness completes Ok).
    let worktree_handle: Option<crate::sandbox::WorktreeHandle> = match req.isolation {
        Some(crate::agents::IsolationMode::Worktree) => {
            // `current_dir()` is a blocking syscall; run it on the blocking pool
            // so the async runtime thread stays available.
            let repo_root = tokio::task::spawn_blocking(std::env::current_dir)
                .await
                .map_err(|e| format!("sub-agent failed: cwd join: {e}"))?
                .map_err(|e| format!("sub-agent failed: cwd: {e}"))?;
            let label = &req.agent_def.id;
            let handle =
                crate::sandbox::worktree::create(&repo_root, label, base.trace_sink.clone())
                    .await
                    .map_err(|e| format!("sub-agent failed: worktree create: {e}"))?;
            Some(handle)
        }
        None => None,
    };

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
        let child_id = ephemeral_for(&req.agent_def.id);

        // 3. Attach the child session and seed the initial Turn + UserMessage.
        //    Any failure here surfaces immediately — the harness never runs.
        base.session
            .attach(child_id.clone())
            .await
            .map_err(|e| format!("sub-agent failed: attach session: {e}"))?;

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

        let effective_task = build_effective_task(
            req.context_summary,
            req.agent_def.context_mode.clone(),
            req.task,
        );
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
                },
            )
            .await
            .map_err(|e| format!("sub-agent failed: emit UserMessage: {e}"))?;

        // Emit SubagentSpawned to the parent session.
        let child_key = child_id.clone();
        let flow_name = req.agent_def.id.clone();
        if let Some(ref parent_str) = base.parent_session_id {
            if let Ok(parent_id) = serde_json::from_str::<SessionId>(parent_str) {
                let _ = base
                    .session
                    .emit_event(
                        &parent_id,
                        SessionEvent::SubagentSpawned {
                            turn_id: turn,
                            child_id: child_key,
                            flow: flow_name,
                            at: now_ms(),
                        },
                    )
                    .await;
            }
        }

        // 4. Build the agent-scoped system prompt. `PromptBuilder::with_agent`
        //    pulls in the AgentRoleLayer; `build_system_prompt(&[])` is fine —
        //    tool schemas are delivered via native tool_use, not the prompt.
        //    The descended `child_chain` is passed in so `ChainContextLayer`
        //    can tell the spawned agent it is nested and how much delegation
        //    budget remains.
        let mut builder = PromptBuilder::new(PromptConfig::default())
            .with_agent(req.agent_def.clone())
            .with_chain_context(child_chain.clone());
        if let Some(strategy) = req.strategy {
            builder = builder.with_strategy(strategy.to_string());
        }
        if let Some(mode) = req.session_mode {
            builder = builder.with_session_mode(mode);
        }
        let system_prompt = builder.build_system_prompt(&[]);

        // 5. Resolve the model override: explicit > model_hint > native.
        let resolved_model: Option<String> = req
            .model
            .map(str::to_string)
            .or_else(|| req.agent_def.model_hint.clone());
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

        let (context_budget, context_compactor, preflight_pipeline) = build_context_triple(
            base.context_budget_config.as_ref(),
            &llm,
            base.cheap_summary_provider.as_ref(),
            &req.agent_def.id,
            &child_id,
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
            verifier_chain: None,
            context_budget,
            context_compactor,
            preflight_pipeline,
            // Stage A (P1) — inherited from parent SpawnerBase. VESR v1.1 (b):
            // when routing capture is on, this is the child's own OutcomeObserver
            // wrapping that sink; otherwise the raw sink (unchanged).
            trace_sink: child_trace_sink,
            system_prompt: Some(system_prompt),
            system_prompt_parts: None,
            recall_context: None,
            // Stage 4 (#11): stamp the descended child chain on the inner harness
            // so its `chain_context()` accessor reports the correct depth/chain_id
            // instead of falling back to a fresh root.
            chain_context: child_chain.clone(),
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
                    req.agent_def.id.clone(),
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
                let msg = panic_message(&panic_payload);
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

                // Emit SubagentReturned to the parent session.
                let summary = result.final_text.clone().unwrap_or_default();
                if let Some(ref parent_str) = base.parent_session_id {
                    if let Ok(parent_id) = serde_json::from_str::<SessionId>(parent_str) {
                        let _ = base
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
                            .await;
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

/// B5 — assemble the child's seed task. A `context_summary` is prepended only
/// when the agent's declared `context_mode` is `Summary`; `Fresh`-mode agents
/// always start from the bare task, making `AgentDef.context_mode`
/// authoritative instead of decorative.
fn build_effective_task(
    context_summary: Option<&str>,
    context_mode: crate::agents::types::ContextMode,
    task: &str,
) -> String {
    match context_summary {
        Some(summary) if context_mode == crate::agents::types::ContextMode::Summary => {
            format!("## Context from parent agent\n\n{summary}\n\n---\n\n{task}")
        }
        _ => task.to_string(),
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
    let events = session
        .get_events(child_id, None, None)
        .await
        .map_err(|e| format!("sub-agent failed: read events: {e}"))?;

    let mut iterations: usize = 0;
    let mut tool_calls_made: usize = 0;
    let mut final_text: Option<String> = None;
    for rec in &events {
        match &rec.event {
            SessionEvent::AssistantMessage { content, .. } => {
                iterations = iterations.saturating_add(1);
                // Keep the most recent assistant text as the "final" answer.
                if !content.text.is_empty() {
                    final_text = Some(content.text.clone());
                } else if is_last_assistant(&events, rec) {
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
fn ephemeral_for(agent_id: &str) -> SessionKey {
    let nonce = uuid::Uuid::new_v4();
    SessionKey::Ephemeral {
        agent_id: agent_id.to_string(),
        ephemeral_id: format!("sub-{nonce}"),
    }
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
                ..CompactorConfig::default()
            },
        )
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

/// Pull a human-readable message out of a panic payload.
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "panic (non-string payload)".to_string()
}

#[cfg(test)]
mod tests;
