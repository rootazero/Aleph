//! Subagent spawner — Harness-based sub-agent execution.
//!
//! Replacement for the legacy pre-Harness `run_subagent` entry point.
//! The spawner takes a `SpawnerBase` (shared session/tools/sandbox/provider)
//! plus a `SpawnRequest` (agent_def, task, model, timeout, cancel), builds a
//! child ephemeral `SessionKey`, assembles a `HarnessDeps` bundle with the
//! agent's system prompt and max_iterations + a tool service wrapped in
//! `AllowlistToolService`, seeds the task as a `UserMessage`, runs
//! `AgentHarness::run` under `tokio::time::timeout` + `catch_unwind` for
//! timeout + panic isolation, then walks the child session event log to
//! synthesize a `LoopRunResult`.
//!
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::Arc;

use futures::FutureExt;
use tokio_util::sync::CancellationToken;

use crate::agents::allowlist_tool_service::AllowlistToolService;
use crate::agents::runtime::LoopRunResult;
use crate::agents::AgentDef;
use crate::error::Result as AlephResult;
use crate::harness::agent::AgentHarness;
use crate::harness::callback::NoopHarnessCallback;
use crate::harness::chain_context::ChainContext;
use crate::harness::deps::HarnessDeps;
use crate::harness::trait_def::Harness;
use crate::memory::extensions::MemoryExtensionRegistry;
use crate::memory::store::raw_memory::RawMemoryStore;
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::AiProvider;
use crate::routing::session_key::SessionKey;
use crate::sandbox::Sandbox;
use crate::session::events::{
    now_ms, MessageContent, SessionEvent, SessionEventRecord, TurnTrigger,
};
use crate::session::service::{SessionId, SessionService};
use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};
use crate::tools::service::ToolService;

/// Shared infrastructure shared by all sub-agent spawns in a given
/// orchestration context (session actor, parent tool service, sandbox,
/// provider, and the parent's chain context).
#[derive(Clone)]
pub struct SpawnerBase {
    /// Shared session service (same actor as the parent).
    pub session: Arc<dyn SessionService>,
    /// The parent's tool service. The spawner decorates this with an
    /// `AllowlistToolService` gated on `AgentDef.is_tool_allowed`.
    pub parent_tools: Arc<dyn ToolService>,
    /// Shared sandbox instance.
    pub sandbox: Arc<dyn Sandbox>,
    /// Provider used for LLM calls. The spawner wraps this with a
    /// `ModelOverrideProvider` when `SpawnRequest.model` is set.
    pub provider: Arc<dyn AiProvider>,
    /// The parent's chain context. The spawner derives a child via
    /// `ChainContext::child()`.
    pub chain: ChainContext,
    /// Spec 1 G2 — when set, the spawner emits a `RawMemory(Delegation)`
    /// row after a successful spawn so CompressionService can distil
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
    /// Stage A (P1) — fallback LLM from `[fallback_provider]`. `None` when
    /// not configured or when self-referencing the primary. Inherited
    /// identically from main runner.
    pub fallback_llm: Option<Arc<dyn AiProvider>>,
    /// Stage A (P1) — stall watchdog config from `[stability]`. `None` when
    /// `stall_timeout_secs` is unset.
    pub stall_config: Option<crate::harness::StallConfig>,
    /// Stage A (P1) — bounded consecutive-failure cap from `[stability]`.
    pub consecutive_failure_cap: Option<usize>,
    /// Stage A (P1) — per-turn wall-clock timeout from `[stability]`.
    pub turn_timeout: Option<std::time::Duration>,
    /// Stage A (P1) — trace sink, cloned from parent's HarnessDeps.
    /// Subagent run events flow into the same sink as the main runner.
    pub trace_sink: Option<Arc<dyn crate::harness::TraceSink>>,
    /// P3 Stage I — global plugin registry. Used by `McpScope::provision`
    /// for per-agent MCP scope lookups. `None` means MCP scope is disabled
    /// (legacy callers + tests with no `mcp_servers`); a non-empty
    /// `agent_def.mcp_servers` will fail-loud if this is `None`.
    pub plugin_registry: Option<Arc<crate::extension::registry::PluginRegistry>>,
    /// A2 — global cap on concurrently-running subagent spawns. `None` skips
    /// the cap (direct test callers); `Some(_)` makes `spawn()` acquire a
    /// permit held for the child's full lifetime.
    pub subagent_semaphore: Option<Arc<tokio::sync::Semaphore>>,
}

/// Per-spawn configuration. All lifetimes are scoped to a single `spawn` call.
pub struct SpawnRequest<'a> {
    /// Agent definition (id, allowed_tools, max_iterations, model_hint, …).
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
    /// HarnessDeps (legacy / default). `Some(IsolationMode::Worktree)`
    /// will provision a detached-HEAD git worktree in Task 9.
    pub isolation: Option<crate::agents::IsolationMode>,
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
            let repo_root = std::env::current_dir()
                .map_err(|e| format!("sub-agent failed: cwd: {e}"))?;
            let label = &req.agent_def.id;
            let handle = crate::sandbox::worktree::create(
                &repo_root,
                label,
                base.trace_sink.clone(),
            )
            .await
            .map_err(|e| format!("sub-agent failed: worktree create: {e}"))?;
            Some(handle)
        }
        None => None,
    };

    // P3 Stage I — provision per-agent MCP scope. Held in outer scope so Drop
    // fires as a safety net on cancel/panic/timeout/error. Explicit
    // shutdown() happens on the success path (after harness completes Ok).
    let mcp_scope: Option<crate::extension::registrar::mcp_registrar::McpScope> =
        if !req.agent_def.mcp_servers.is_empty() {
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

    let result: Result<LoopRunResult, String> = async {
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
                },
            )
            .await
            .map_err(|e| format!("sub-agent failed: emit UserMessage: {e}"))?;

        // 4. Build the agent-scoped system prompt. `PromptBuilder::with_agent`
        //    pulls in the AgentRoleLayer; `build_system_prompt(&[])` is fine —
        //    tool schemas are delivered via native tool_use, not the prompt.
        //    `native_tools_enabled = true` skips ToolsLayer and
        //    ResponseFormatLayer so the prompt does not (a) lie to the LLM
        //    that no tools exist, nor (b) mandate the legacy
        //    `{reasoning, action}` JSON envelope which contradicts native
        //    tool_use.
        let system_prompt = PromptBuilder::new(PromptConfig {
            native_tools_enabled: true,
            ..PromptConfig::default()
        })
        .with_agent(req.agent_def.clone())
        .build_system_prompt(&[]);

        // 5. Resolve the model override: explicit > model_hint > native.
        let resolved_model: Option<String> = req
            .model
            .map(str::to_string)
            .or_else(|| req.agent_def.model_hint.clone());
        let llm: Arc<dyn AiProvider> = match resolved_model {
            Some(m) => Arc::new(ModelOverrideProvider {
                inner: base.provider.clone(),
                model: m,
            }),
            None => base.provider.clone(),
        };
        // Stage J-pre: wrap with MeteringProvider so every LLM call from this
        // subagent emits a LoopTraceEvent::ProviderUsage labelled with the
        // subagent's agent_def.id (distinct from "root" label used at the
        // top-level harness wrap site in orchestrator_init.rs).
        let token_counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let llm: Arc<dyn AiProvider> = Arc::new(
            crate::providers::MeteringProvider::new(
                llm,
                base.trace_sink.clone(),
                req.agent_def.id.clone(),
            )
            .with_token_accumulator(token_counter.clone()),
        );

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

        let max_iter = req
            .agent_def
            .max_iterations
            .map(usize::try_from)
            .transpose()
            .map_err(|_| "max_iterations exceeds platform limit".to_string())?;

        // P3 Stage H — isolation override: when a worktree is provisioned for
        // this child, swap in a WorktreeSandbox so all command execution runs
        // at the worktree path with CARGO_TARGET_DIR redirected.
        let sandbox: Arc<dyn crate::sandbox::Sandbox> = match worktree_handle.as_ref() {
            Some(h) => Arc::new(crate::sandbox::WorktreeSandbox::new(
                h.path().to_path_buf(),
            )),
            None => base.sandbox.clone(),
        };

        let deps = HarnessDeps {
            session: base.session.clone(),
            tools: scoped_tools,
            sandbox,
            llm,
            verifier_chain: None,
            context_budget: None,
            context_compactor: None,
            skill_prefetcher: None,
            // Stage A (P1) — was None; now inherited from parent SpawnerBase.
            trace_sink: base.trace_sink.clone(),
            system_prompt: Some(system_prompt),
            prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
            // Stage 4 (#11): stamp the descended child chain on the inner harness
            // so its `chain_context()` accessor reports the correct depth/chain_id
            // instead of falling back to a fresh root.
            chain_context: child_chain.clone(),
            // Stage 5a (#9): inherit parent guardrails so the subagent enforces
            // the same Input/Output/ToolCall checks as the spawning harness.
            guardrails: base.guardrails.clone(),
            // Stage A (P1) — was None; now inherited from parent SpawnerBase.
            fallback_llm: base.fallback_llm.clone(),
            max_iterations: max_iter,
            power: None,
            // Stage A (P1) — was None for all three; now inherited from parent.
            stall_config: base.stall_config.clone(),
            consecutive_failure_cap: base.consecutive_failure_cap,
            turn_timeout: base.turn_timeout,
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
        let outcome =
            tokio::time::timeout(timeout, AssertUnwindSafe(run_fut).catch_unwind()).await;

        match outcome {
            Err(_elapsed) => Err(format!("Sub-agent timed out after {}s", req.timeout_secs)),
            Ok(Err(panic_payload)) => {
                let msg = panic_message(&panic_payload);
                Err(format!("sub-agent panicked: {msg}"))
            }
            Ok(Ok(Err(e))) => Err(format!("sub-agent failed: {e}")),
            Ok(Ok(Ok(()))) => {
                // 8. Query the harness directly for the `hit_limit` signal. The
                //    previous implementation reconstructed this from the event log
                //    because the harness had been moved into the async closure; with
                //    `Arc<AgentHarness>` we just read the flag.
                let hit_limit = harness.hit_limit();

                let result = extract_run_result(
                    base.session.as_ref(),
                    &child_id,
                    &child_chain,
                    hit_limit,
                    token_counter.load(std::sync::atomic::Ordering::Relaxed),
                )
                .await?;

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
    }
    .await;

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
            format!(
                "## Context from parent agent\n\n{}\n\n---\n\n{}",
                summary, task
            )
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
///                 `AgentHarness::hit_limit()` after the run).
async fn extract_run_result(
    session: &dyn SessionService,
    child_id: &SessionId,
    chain: &ChainContext,
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
                    // (no text). Clear any earlier textual answer so the
                    // gateway's `hit_limit && final_text.is_empty()` check in
                    // `helpers::gateway_response_from_outcome` surfaces
                    // `ErrLoopExhausted` instead of echoing a stale earlier
                    // message. The dedicated `final_text_cleared_when_…`
                    // regression test below asserts this behavior.
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
        chain_id: chain.chain_id.clone(),
        depth: chain.depth,
    })
}

/// Generate a unique ephemeral SessionKey for this sub-agent spawn.
fn ephemeral_for(agent_id: &str) -> SessionKey {
    let nonce = uuid::Uuid::new_v4();
    SessionKey::Ephemeral {
        agent_id: agent_id.to_string(),
        ephemeral_id: format!("sub-{nonce}"),
    }
}

/// Whether `target` is the last `AssistantMessage` in `events` (by seq).
fn is_last_assistant(events: &[SessionEventRecord], target: &SessionEventRecord) -> bool {
    events
        .iter()
        .rev()
        .find(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
        .map(|r| r.seq == target.seq)
        .unwrap_or(false)
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

/// Provider wrapper that stamps `RequestPayload.model` with a configured
/// override before delegating to the inner provider. Used when the spawn
/// request (or agent model_hint) supplies a per-spawn model.
struct ModelOverrideProvider {
    inner: Arc<dyn AiProvider>,
    model: String,
}

impl AiProvider for ModelOverrideProvider {
    fn process<'a>(
        &'a self,
        mut payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        payload.model = Some(self.model.clone());
        self.inner.process(payload)
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn color(&self) -> &str {
        self.inner.color()
    }

    fn supports_native_tools(&self) -> bool {
        self.inner.supports_native_tools()
    }

    fn supports_thinking(&self) -> bool {
        self.inner.supports_thinking()
    }

    fn protocol(&self) -> &str {
        self.inner.protocol()
    }
}

#[cfg(test)]
mod tests;
