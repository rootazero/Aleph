//! Sub-agent runtime — canonical home for `AgentRuntime` and the types it
//! exposes to `SubagentTool` and callers.
//!
//! Wraps the Harness-based spawner with lifecycle tracing and transcript
//! persistence.

use std::collections::HashMap;
use std::time::Instant;

use tokio_util::sync::CancellationToken;

use crate::agents::AgentDef;
use crate::harness::chain_context::ChainContext;
use crate::memory::extensions::MemoryExtensionRegistry;
use crate::memory::store::raw_memory::RawMemoryStore;
use crate::providers::AiProvider;
use crate::session::service::SessionService;
use crate::sync_primitives::Arc;
use crate::tools::service::ToolService;

// =============================================================================
// LoopRunResult
// =============================================================================

/// Outcome of a completed sub-agent run.
///
/// Cancellation is surfaced as an `Err("sub-agent failed: …")` from
/// `AgentRuntime::run`, not via a boolean on this struct — so there is no
/// dedicated `cancelled` field.
#[derive(Debug, Clone)]
pub struct LoopRunResult {
    pub final_text: Option<String>,
    pub iterations: usize,
    pub tool_calls_made: usize,
    pub total_tokens: usize,
    pub hit_limit: bool,
}

// =============================================================================
// AgentRuntimeConfig
// =============================================================================

/// Configuration for launching a sub-agent.
pub struct AgentRuntimeConfig {
    /// The agent definition describing role, tools, and limits.
    pub agent_def: AgentDef,
    /// The task to execute.
    pub task: String,
    /// Optional context summary from the parent agent.
    pub context_summary: Option<String>,
    /// Explicit model override (highest priority).
    pub model: Option<String>,
    /// Timeout in seconds for the entire run.
    pub timeout_secs: u64,
}

// =============================================================================
// Transcript types
// =============================================================================

/// Outcome classification for a sub-agent execution.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum TranscriptOutcome {
    /// The sub-agent completed successfully.
    Success,
    /// The sub-agent encountered an error.
    Error(String),
    /// The sub-agent was terminated due to timeout.
    Timeout,
}

/// Structured transcript of a sub-agent execution for observability.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SubagentTranscript {
    /// Unique identifier for the agent instance.
    pub agent_id: String,
    /// Agent type name (from `agent_def`).
    pub agent_type: String,
    /// Summary of the task that was executed.
    pub task_summary: String,
    /// How the execution ended.
    pub outcome: TranscriptOutcome,
    /// Number of think-act iterations completed.
    pub iterations: usize,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
    /// Total tokens consumed.
    pub tokens_used: usize,
    /// Key findings extracted from the agent's final response (first 200 chars).
    pub key_findings: String,
}

// =============================================================================
// AgentRuntime
// =============================================================================

/// Middle layer that manages sub-agent lifecycle: setup, execution, and transcript.
pub struct AgentRuntime {
    provider: Arc<dyn AiProvider>,
    child_chain: ChainContext,
    cancel_token: CancellationToken,
    /// Shared session actor used by the Harness spawner for the child's
    /// ephemeral session.
    session: Arc<dyn SessionService>,
    /// Parent tool service — decorated with `AllowlistToolService` inside
    /// the spawner.
    parent_tools: Arc<dyn ToolService>,
    /// Spec 1 G2 — when set, the spawner emits a `RawMemory(Delegation)`
    /// row after each successful subagent run.
    raw_memory_writer: Option<Arc<dyn RawMemoryStore>>,
    /// Optional capture-filter registry threaded into the delegation emit.
    capture_registry: Option<Arc<MemoryExtensionRegistry>>,
    /// Parent agent identity stamped onto the emitted Delegation row.
    parent_agent_id: Option<String>,
    /// Parent session id stamped onto the emitted Delegation row.
    parent_session_id: Option<String>,
    /// Stage 5a (#9) — guardrail registry inherited by spawned subagents.
    /// `None` keeps the legacy "no guardrails" path; `Some(_)` propagates
    /// to every `SpawnerBase` built by `spawn_subagent`.
    guardrails: Option<Arc<crate::guardrails::GuardrailRegistry>>,
    /// Stage A (P1) — stall watchdog config threaded into `SpawnerBase`.
    stall_config: Option<crate::harness::StallConfig>,
    /// Stage A (P1) — consecutive-failure cap threaded into `SpawnerBase`.
    consecutive_failure_cap: Option<usize>,
    /// Stage A (P1) — per-turn timeout threaded into `SpawnerBase`.
    turn_timeout: Option<std::time::Duration>,
    /// Stage A (P1) — trace sink threaded into `SpawnerBase`.
    trace_sink: Option<Arc<dyn crate::harness::TraceSink>>,
    /// A2 — subagent concurrency cap, threaded into every `SpawnerBase`.
    subagent_semaphore: Option<Arc<tokio::sync::Semaphore>>,
    /// B2 — shared plugin-registry handle, threaded into `SpawnerBase` for
    /// per-agent MCP scope provisioning.
    plugin_registry: Option<Arc<tokio::sync::RwLock<crate::extension::registry::PluginRegistry>>>,
    /// Phase 3 — `provider_hint` → pinned-then-fall-through provider. An empty
    /// map (the `new()` default) means every spawn uses `provider`.
    provider_overrides: HashMap<String, Arc<dyn AiProvider>>,
    /// Parent run's usage mode, applied to every spawn (skipped for Work by
    /// the wiring site — identity partition, byte-identical child prompt).
    session_mode: Option<crate::config::types::policies::SessionMode>,
    /// Welded strategy `<strategy>` body applied to every spawn's
    /// `AgentRuntimeConfig`. `None` (the `new()` default) keeps the legacy
    /// no-strategy path.
    strategy: Option<String>,
    /// VESR v1.1 (b) — routing-experience store threaded into every
    /// `SpawnerBase` so spawned subagents capture their run under
    /// `agent_def.id`. `None` (the `new()` default) keeps subagents
    /// capture-free.
    routing_store: Option<Arc<crate::routing::RoutingExperienceStore>>,
    /// B15 — the parent runner's boot-time `[execution] max_iterations`,
    /// threaded into `SpawnerBase` so a child role with no declared cap
    /// inherits one instead of running its Think→Act loop unbounded.
    default_max_iterations: Option<usize>,
    /// The parent runner's `[tool_service] parallel_tool_concurrency`,
    /// threaded into `SpawnerBase` so a child's Act-phase cap matches the
    /// operator's configured value (including 0/1 = disabled).
    parallel_tool_concurrency: Option<usize>,
    /// The parent runner's `[context_budget]` config, threaded into
    /// `SpawnerBase` so each spawned child builds its own budget + compactor +
    /// preflight pipeline instead of running context-unmanaged.
    context_budget_config: Option<crate::context::budget::ContextBudgetConfig>,
}

impl AgentRuntime {
    /// Create a new runtime with the shared infrastructure needed to spawn agents.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: Arc<dyn AiProvider>,
        child_chain: ChainContext,
        cancel_token: CancellationToken,
        session: Arc<dyn SessionService>,
        parent_tools: Arc<dyn ToolService>,
    ) -> Self {
        Self {
            provider,
            child_chain,
            cancel_token,
            session,
            parent_tools,
            raw_memory_writer: None,
            capture_registry: None,
            parent_agent_id: None,
            parent_session_id: None,
            guardrails: None,
            stall_config: None,
            consecutive_failure_cap: None,
            turn_timeout: None,
            trace_sink: None,
            subagent_semaphore: None,
            plugin_registry: None,
            provider_overrides: HashMap::new(),
            strategy: None,
            session_mode: None,
            routing_store: None,
            default_max_iterations: None,
            parallel_tool_concurrency: None,
            context_budget_config: None,
        }
    }

    /// B15 — wire the parent runner's boot-time iteration cap, inherited by
    /// every spawned child that declares none of its own.
    #[must_use]
    pub const fn with_default_max_iterations(mut self, max_iterations: usize) -> Self {
        self.default_max_iterations = Some(max_iterations);
        self
    }

    /// Wire the parent runner's `[context_budget]` config so every spawned
    /// child gets its own budget / compactor / preflight pipeline.
    #[must_use]
    pub fn with_context_budget_config(
        mut self,
        cfg: crate::context::budget::ContextBudgetConfig,
    ) -> Self {
        self.context_budget_config = Some(cfg);
        self
    }

    /// Wire the parent runner's `[tool_service] parallel_tool_concurrency`,
    /// inherited by every spawned child's Act phase.
    #[must_use]
    pub const fn with_parallel_tool_concurrency(mut self, cap: usize) -> Self {
        self.parallel_tool_concurrency = Some(cap);
        self
    }

    /// Wire the welded strategy `<strategy>` body inherited by every spawn.
    #[must_use]
    pub fn with_strategy(mut self, strategy: String) -> Self {
        self.strategy = Some(strategy);
        self
    }

    /// Wire the parent run's usage mode inherited by every spawn.
    #[must_use]
    pub const fn with_session_mode(
        mut self,
        mode: crate::config::types::policies::SessionMode,
    ) -> Self {
        self.session_mode = Some(mode);
        self
    }

    /// Phase 3 — wire the per-`provider_hint` override registry. Each spawn
    /// whose `agent_def.provider_hint` matches a key runs on that provider.
    #[must_use]
    pub fn with_provider_overrides(
        mut self,
        overrides: HashMap<String, Arc<dyn AiProvider>>,
    ) -> Self {
        self.provider_overrides = overrides;
        self
    }

    /// A2 — wire the shared subagent concurrency semaphore.
    pub fn with_subagent_semaphore(mut self, sem: Arc<tokio::sync::Semaphore>) -> Self {
        self.subagent_semaphore = Some(sem);
        self
    }

    /// B2 — wire the shared plugin-registry handle for per-agent MCP scope.
    #[must_use]
    pub fn with_plugin_registry(
        mut self,
        registry: Arc<tokio::sync::RwLock<crate::extension::registry::PluginRegistry>>,
    ) -> Self {
        self.plugin_registry = Some(registry);
        self
    }

    /// Stage 5a (#9) — wire a guardrail registry that subagents inherit.
    pub fn with_guardrails(mut self, registry: Arc<crate::guardrails::GuardrailRegistry>) -> Self {
        self.guardrails = Some(registry);
        self
    }

    // Stage A (P1) — resilience builders threaded into SpawnerBase →
    // HarnessDeps. `SubagentTool` applies them via `build_runtime`; `trace_sink`
    // is wired in production at the run_loop.rs construction site.

    /// Stage A (P1) — wire the stall watchdog config.
    #[must_use]
    pub const fn with_stall_config(mut self, config: crate::harness::StallConfig) -> Self {
        self.stall_config = Some(config);
        self
    }

    /// Stage A (P1) — wire the consecutive-failure cap.
    #[must_use]
    pub const fn with_consecutive_failure_cap(mut self, cap: usize) -> Self {
        self.consecutive_failure_cap = Some(cap);
        self
    }

    /// Stage A (P1) — wire the per-turn wall-clock timeout.
    #[must_use]
    pub const fn with_turn_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.turn_timeout = Some(timeout);
        self
    }

    /// Stage A (P1) — wire the trace sink. Subagents emit into the same sink.
    pub fn with_trace_sink(mut self, sink: Arc<dyn crate::harness::TraceSink>) -> Self {
        self.trace_sink = Some(sink);
        self
    }

    /// VESR v1.1 (b) — wire the routing-experience store inherited by subagents.
    #[must_use]
    pub fn with_routing_store(
        mut self,
        store: Arc<crate::routing::RoutingExperienceStore>,
    ) -> Self {
        self.routing_store = Some(store);
        self
    }

    /// Wire a `RawMemoryStore` so the spawner emits the Delegation hook.
    pub fn with_raw_memory_writer(mut self, writer: Arc<dyn RawMemoryStore>) -> Self {
        self.raw_memory_writer = Some(writer);
        self
    }

    /// Wire an optional capture-filter registry threaded into the emit.
    pub fn with_capture_registry(mut self, registry: Arc<MemoryExtensionRegistry>) -> Self {
        self.capture_registry = Some(registry);
        self
    }

    /// Set the parent agent id stamped onto the emitted Delegation row.
    pub fn with_parent_agent_id(mut self, id: impl Into<String>) -> Self {
        self.parent_agent_id = Some(id.into());
        self
    }

    /// Set the parent session id stamped onto the emitted Delegation row.
    pub fn with_parent_session_id(mut self, sid: impl Into<String>) -> Self {
        self.parent_session_id = Some(sid.into());
        self
    }

    /// Execute a sub-agent to completion with lifecycle tracing.
    pub async fn run(&self, config: AgentRuntimeConfig) -> Result<LoopRunResult, String> {
        let start = Instant::now();
        let agent_id = format!("{}-{}", config.agent_def.id, uuid::Uuid::new_v4());
        let agent_type = config.agent_def.id.clone();
        let task_summary = truncate_for_log(&config.task, 120);

        tracing::info!(
            agent_id = %agent_id,
            agent_type = %agent_type,
            task = %task_summary,
            "SubagentStart: launching sub-agent"
        );

        // SubagentStart lifecycle hook (observer-only — the child has already
        // launched). Reuses the process-global fire-and-forget helper; a silent
        // no-op when no hooks are registered, so it is safe on every spawn.
        crate::extension::hooks::fire_global_observer(
            crate::extension::HookEvent::SubagentStart,
            self.parent_session_id.as_deref().unwrap_or_default(),
            vec![
                ("SUBAGENT_ID", agent_id.clone()),
                ("SUBAGENT_TYPE", agent_type.clone()),
                ("TASK", task_summary.clone()),
                (
                    "PARENT_AGENT_ID",
                    self.parent_agent_id.clone().unwrap_or_default(),
                ),
                ("CHAIN_DEPTH", self.child_chain.depth.to_string()),
            ],
        )
        .await;

        let result = self.execute_via_harness(&config).await;

        let duration_ms = start.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
        let key_findings = match &result {
            Ok(run_result) => run_result
                .final_text
                .as_deref()
                .unwrap_or("")
                .chars()
                .take(200)
                .collect::<String>(),
            Err(_) => String::new(),
        };
        let transcript = match &result {
            Ok(run_result) => SubagentTranscript {
                agent_id: agent_id.clone(),
                agent_type: agent_type.clone(),
                task_summary: task_summary.clone(),
                outcome: TranscriptOutcome::Success,
                iterations: run_result.iterations,
                duration_ms,
                tokens_used: run_result.total_tokens,
                key_findings: key_findings.clone(),
            },
            Err(e) => {
                // Match the spawner's exact wall-clock-timeout prefix
                // ("Sub-agent timed out after Ns") rather than a loose substring —
                // an inner error merely *containing* "timed out" (e.g. a wrapped
                // "connection timed out") must not be misclassified as a hard timeout.
                let outcome = if e.starts_with("Sub-agent timed out") {
                    TranscriptOutcome::Timeout
                } else {
                    TranscriptOutcome::Error(e.clone())
                };
                SubagentTranscript {
                    agent_id: agent_id.clone(),
                    agent_type: agent_type.clone(),
                    task_summary: task_summary.clone(),
                    outcome,
                    iterations: 0,
                    duration_ms,
                    tokens_used: 0,
                    key_findings: key_findings.clone(),
                }
            }
        };

        tracing::info!(
            agent_id = %transcript.agent_id,
            agent_type = %transcript.agent_type,
            outcome = ?format_outcome(&transcript.outcome),
            iterations = transcript.iterations,
            duration_ms = transcript.duration_ms,
            tokens_used = transcript.tokens_used,
            "SubagentEnd: sub-agent completed"
        );

        // Persist on the blocking pool so the async runtime thread is not held
        // by filesystem I/O. Transcript persistence is best-effort; errors are
        // logged inside `persist_transcript`.
        let transcript_for_persist = transcript.clone();
        let chain_id_for_persist = self.child_chain.chain_id.clone();
        // Detach rather than await: dropping the `JoinHandle` leaves the spawned
        // task running (drop is not cancellation), which is what best-effort wants.
        drop(tokio::task::spawn_blocking(move || {
            persist_transcript(&transcript_for_persist, &chain_id_for_persist);
        }));

        // SubagentStop lifecycle hook (observer-only). Carries the completion
        // outcome so hooks can react to delegation results without re-reading
        // the transcript store.
        crate::extension::hooks::fire_global_observer(
            crate::extension::HookEvent::SubagentStop,
            self.parent_session_id.as_deref().unwrap_or_default(),
            vec![
                ("SUBAGENT_ID", transcript.agent_id.clone()),
                ("SUBAGENT_TYPE", transcript.agent_type.clone()),
                ("OUTCOME", format_outcome(&transcript.outcome).to_string()),
                ("ITERATIONS", transcript.iterations.to_string()),
                ("DURATION_MS", transcript.duration_ms.to_string()),
                ("TOKENS_USED", transcript.tokens_used.to_string()),
                ("KEY_FINDINGS", transcript.key_findings.clone()),
            ],
        )
        .await;

        result
    }

    async fn execute_via_harness(
        &self,
        config: &AgentRuntimeConfig,
    ) -> Result<LoopRunResult, String> {
        use crate::agents::subagent_spawner::{spawn, SpawnRequest, SpawnerBase};
        // `self.child_chain` is the already-descended chain produced by the
        // caller (SubagentTool::execute calls `self.chain.child()`). The
        // spawner descends again via `base.chain.child()`, so synthesize the
        // logical parent here to keep depth accounting equivalent to the
        // retiring `run_subagent` path (which consumed the child_chain as-is).
        //
        // Invariant: `child_chain` came from `ChainContext::child()`, which
        // returns `Some` only after incrementing depth by 1. depth == 0 here
        // would indicate a caller constructed the runtime with an un-descended
        // chain — the assert catches that in debug builds; release uses
        // `saturating_sub` to avoid underflow.
        debug_assert!(
            self.child_chain.depth > 0,
            "AgentRuntime received an un-descended ChainContext; callers must pass `parent_chain.child()`"
        );
        let parent_chain = ChainContext {
            chain_id: self.child_chain.chain_id.clone(),
            depth: self.child_chain.depth.saturating_sub(1),
            max_depth: self.child_chain.max_depth,
        };
        // Phase 3 — resolve the per-agent provider: `provider_hint` pins a
        // registered override (pinned, then falling through the global chain);
        // a `provider/model` model id routes across vendors; otherwise the
        // shared default. See `resolve_spawn_route`.
        //
        // The effective model is `model` then `model_hint` — the same order the
        // spawner resolves — so a role whose frontmatter carries the qualified
        // form routes too.
        let effective_model = config
            .model
            .as_deref()
            .or(config.agent_def.model_hint.as_deref());
        let (provider, routed_model) = resolve_spawn_route(
            &self.provider,
            &self.provider_overrides,
            &config.agent_def,
            effective_model,
        );
        let base = SpawnerBase {
            session: self.session.clone(),
            parent_tools: self.parent_tools.clone(),
            provider,
            chain: parent_chain,
            raw_memory_writer: self.raw_memory_writer.clone(),
            capture_registry: self.capture_registry.clone(),
            parent_agent_id: self.parent_agent_id.clone(),
            parent_session_id: self.parent_session_id.clone(),
            guardrails: self.guardrails.clone(),
            // Stage A (P1):
            stall_config: self.stall_config.clone(),
            consecutive_failure_cap: self.consecutive_failure_cap,
            turn_timeout: self.turn_timeout,
            trace_sink: self.trace_sink.clone(),
            // P3 Stage I — per-agent MCP scope; provisioned when an agent_def
            // declares `mcp_servers` and a registry is wired (B2).
            plugin_registry: self.plugin_registry.clone(),
            // A2 — subagent concurrency cap.
            subagent_semaphore: self.subagent_semaphore.clone(),
            // VESR v1.1 (b) — threaded from the gateway so subagents capture.
            routing_store: self.routing_store.clone(),
            // B15 — the parent's iteration cap, so a capless child role does
            // not run its loop unbounded until the spawn timeout kills it.
            default_max_iterations: self.default_max_iterations,
            // The parent's Act-phase parallel cap, so a child honours the
            // operator's `[tool_service] parallel_tool_concurrency` (0/1 =
            // disabled) instead of the hardcoded config default.
            parallel_tool_concurrency: self.parallel_tool_concurrency,
            // The parent's `[context_budget]` config, so the child is context-
            // managed on the same terms (the spawner builds its own instances).
            // rust-doctor-disable-next-line excessive-clone
            context_budget_config: self.context_budget_config.clone(),
        };
        let req = SpawnRequest {
            agent_def: &config.agent_def,
            task: &config.task,
            context_summary: config.context_summary.as_deref(),
            // A rewritten (de-qualified) id wins; otherwise pass the caller's
            // model through untouched and let the spawner apply its own
            // `model_hint` fallback.
            model: routed_model.as_deref().or(config.model.as_deref()),
            timeout_secs: config.timeout_secs,
            cancel: self.cancel_token.clone(),
            isolation: config.agent_def.isolation.clone(),
            strategy: self.strategy.as_deref(),
            session_mode: self.session_mode,
        };
        spawn(&base, req).await
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Resolve which provider a spawn runs on, and the model id that goes on the
/// wire.
///
/// Precedence:
///   1. `agent_def.provider_hint` — an explicit author/operator choice, and the
///      only form that existed before. Wins outright.
///   2. A `provider/model` id whose prefix names the PARENT's own provider: keep
///      the parent provider, stamp the bare model id. Without this a direct
///      single-vendor deployment (the common case) sent `anthropic/claude-…` to
///      Anthropic verbatim and got a 404.
///   3. A `provider/model` id whose prefix names another CONFIGURED provider
///      (`overrides`, keyed by `[providers]` toml name and matched through
///      `canonical_provider_id`, so `kimi` ≡ `moonshot/…` and `vertex-anthropic`
///      ≡ `anthropic/…`): run the child there with the bare model id. This is
///      what makes a cross-vendor MoA fan-out
///      (`proposer_models: ["openai/gpt-5.2", "anthropic/claude-opus-5"]`)
///      actually reach two vendors instead of handing both strings to whichever
///      provider the parent happened to hold.
///   4. Otherwise: the parent's provider, model string untouched. Untouched
///      matters — an OpenAI-compatible aggregator primary (OpenRouter and
///      friends) *wants* `anthropic/claude-…` on the wire, and such a deployment
///      has no separate `[providers] anthropic` entry to match in step 3.
///
/// Returns the provider plus `Some(bare_model)` when the id was rewritten, and
/// `None` when the caller's model string must be passed through as-is.
fn resolve_spawn_route(
    parent: &Arc<dyn AiProvider>,
    overrides: &HashMap<String, Arc<dyn AiProvider>>,
    agent_def: &AgentDef,
    model: Option<&str>,
) -> (Arc<dyn AiProvider>, Option<String>) {
    if let Some(pinned) = agent_def
        .provider_hint
        .as_deref()
        .and_then(|hint| overrides.get(hint))
    {
        return (pinned.clone(), None);
    }
    let Some((prefix, bare)) = model.and_then(split_provider_prefix) else {
        return (parent.clone(), None);
    };
    let want = crate::providers::model_catalog::canonical_provider_id(prefix);
    let parent_vendor = parent
        .serving_provider_hint()
        .and_then(|name| crate::providers::model_catalog::canonical_provider_id(&name));
    if want.is_some() && want == parent_vendor {
        return (parent.clone(), Some(bare.to_string()));
    }
    match override_for_provider(overrides, prefix, want) {
        Some(provider) => (provider, Some(bare.to_string())),
        None => (parent.clone(), None),
    }
}

/// Look up a configured non-primary provider by `[providers]` toml name, falling
/// back to a canonical-vendor match. The fallback resolves ties by the
/// lexicographically smallest key so two entries for one vendor pick
/// deterministically (same rule as `MultiProviderRegistry::default_provider`).
fn override_for_provider(
    overrides: &HashMap<String, Arc<dyn AiProvider>>,
    name: &str,
    canonical: Option<&'static str>,
) -> Option<Arc<dyn AiProvider>> {
    if let Some(provider) = overrides.get(name) {
        return Some(provider.clone());
    }
    let want = canonical?;
    overrides
        .iter()
        .filter(|(key, _)| {
            crate::providers::model_catalog::canonical_provider_id(key) == Some(want)
        })
        .min_by(|(a, _), (b, _)| a.cmp(b))
        .map(|(_, provider)| provider.clone())
}

/// Split a `provider/model` id into `(provider, model)`.
///
/// `None` when there is no prefix, or when either half is blank — those are not
/// qualified ids and must be passed through untouched. Only the FIRST `/` is
/// split on, so a nested aggregator id (`x-ai/openai/…`) keeps its remainder.
fn split_provider_prefix(model: &str) -> Option<(&str, &str)> {
    let (prefix, rest) = model.split_once('/')?;
    let prefix = prefix.trim();
    let rest = rest.trim();
    (!prefix.is_empty() && !rest.is_empty()).then_some((prefix, rest))
}

/// Truncate a string for log output, appending "..." if truncated.
#[must_use]
fn truncate_for_log(s: &str, max_len: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_len {
        s.to_string()
    } else {
        match s.char_indices().nth(max_len) {
            Some((idx, _)) => format!("{}...", &s[..idx]),
            None => s.to_string(),
        }
    }
}

/// Format a `TranscriptOutcome` for log output.
#[must_use]
const fn format_outcome(outcome: &TranscriptOutcome) -> &str {
    match outcome {
        TranscriptOutcome::Success => "success",
        TranscriptOutcome::Error(_) => "error",
        TranscriptOutcome::Timeout => "timeout",
    }
}

/// Maximum transcript directories to retain per session.
const MAX_TRANSCRIPT_DIRS: usize = 50;

fn cleanup_old_transcripts(base_dir: &std::path::Path) {
    let parent = match base_dir.parent() {
        Some(p) => p,
        None => return,
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };

    let mut dirs: Vec<(std::path::PathBuf, std::time::SystemTime)> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let modified = e.metadata().ok()?.modified().ok()?;
            Some((e.path(), modified))
        })
        .collect();

    if dirs.len() <= MAX_TRANSCRIPT_DIRS {
        return;
    }

    dirs.sort_by_key(|(_, t)| *t);
    let to_remove = dirs.len() - MAX_TRANSCRIPT_DIRS;
    for (path, _) in dirs.into_iter().take(to_remove) {
        let _ = std::fs::remove_dir_all(path);
    }
}

/// Persist a subagent transcript to disk for future retrieval.
/// Best-effort: errors are logged but not propagated.
fn persist_transcript(transcript: &SubagentTranscript, session_id: &str) {
    // Sanitize path components so a user/project-controlled agent id or
    // session id cannot traverse out of the transcript directory.
    let safe_session = session_id.replace(['/', '\\'], "_").replace("..", "_");
    let safe_agent_id = transcript
        .agent_id
        .replace(['/', '\\'], "_")
        .replace("..", "_");
    let base = match dirs::home_dir() {
        Some(h) => h.join(".aleph/data/transcripts").join(safe_session),
        None => {
            tracing::warn!("Cannot resolve home dir for transcript persistence");
            return;
        }
    };
    if let Err(e) = std::fs::create_dir_all(&base) {
        tracing::warn!(error = %e, "Failed to create transcript directory");
        return;
    }
    let path = base.join(format!("{safe_agent_id}.json"));
    match serde_json::to_string_pretty(transcript) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                tracing::warn!(path = %path.display(), error = %e, "Failed to write transcript");
            } else {
                tracing::debug!(path = %path.display(), "Transcript persisted");
                cleanup_old_transcripts(&base);
            }
        }
        Err(e) => tracing::warn!(error = %e, "Failed to serialize transcript"),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_agent_def() -> AgentDef {
        AgentDef::new("test-agent", crate::agents::AgentMode::SubAgent)
    }

    /// Provider stub that reports a configured key through
    /// `serving_provider_hint`, the way `HttpProvider` does at the leaf of the
    /// decorator stack.
    struct NamedProvider(&'static str);

    impl crate::providers::AiProvider for NamedProvider {
        fn process<'a>(
            &'a self,
            _payload: crate::providers::adapter::RequestPayload<'a>,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = crate::error::Result<crate::providers::adapter::ProviderResponse>,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(async {
                Ok(crate::providers::adapter::ProviderResponse::text_only(
                    "stub".to_string(),
                ))
            })
        }
        fn name(&self) -> &str {
            self.0
        }
        fn color(&self) -> &str {
            "#000"
        }
        fn serving_provider_hint(&self) -> Option<std::borrow::Cow<'_, str>> {
            Some(std::borrow::Cow::Borrowed(self.0))
        }
    }

    fn named(name: &'static str) -> Arc<dyn AiProvider> {
        Arc::new(NamedProvider(name))
    }

    fn overrides(entries: &[(&str, &'static str)]) -> HashMap<String, Arc<dyn AiProvider>> {
        entries
            .iter()
            .map(|(key, provider)| ((*key).to_string(), named(provider)))
            .collect()
    }

    /// An explicit `provider_hint` is an author decision and still wins outright
    /// — the model id never redirects it.
    #[test]
    fn spawn_route_provider_hint_wins() {
        let parent = named("anthropic");
        let ovr = overrides(&[("openai", "openai")]);
        let def = make_agent_def().with_provider_hint("openai");
        let (provider, model) = resolve_spawn_route(&parent, &ovr, &def, Some("openai/gpt-5"));
        assert_eq!(provider.name(), "openai");
        assert_eq!(
            model, None,
            "a pinned provider passes the caller's model string through untouched"
        );
    }

    /// A qualified id naming the parent's OWN vendor keeps the parent provider
    /// and drops the prefix — the direct single-vendor deployment used to send
    /// `anthropic/claude-…` to Anthropic verbatim.
    #[test]
    fn spawn_route_strips_prefix_for_parent_vendor() {
        let parent = named("anthropic");
        let ovr = overrides(&[("openai", "openai")]);
        let (provider, model) = resolve_spawn_route(
            &parent,
            &ovr,
            &make_agent_def(),
            Some("anthropic/claude-opus-5"),
        );
        assert_eq!(provider.name(), "anthropic");
        assert_eq!(model.as_deref(), Some("claude-opus-5"));
    }

    /// A qualified id naming a different configured provider routes there — the
    /// case that makes a cross-vendor MoA fan-out actually reach two vendors.
    #[test]
    fn spawn_route_crosses_to_configured_provider() {
        let parent = named("anthropic");
        let ovr = overrides(&[("openai", "openai-chain")]);
        let (provider, model) =
            resolve_spawn_route(&parent, &ovr, &make_agent_def(), Some("openai/gpt-5.2"));
        assert_eq!(provider.name(), "openai-chain");
        assert_eq!(model.as_deref(), Some("gpt-5.2"));
    }

    /// Provider keys are operator-chosen, so the match falls back to canonical
    /// vendor slugs: a `[providers] kimi` entry serves `moonshot/…`.
    #[test]
    fn spawn_route_matches_provider_alias() {
        let parent = named("anthropic");
        let ovr = overrides(&[("kimi", "kimi-chain")]);
        let (provider, model) =
            resolve_spawn_route(&parent, &ovr, &make_agent_def(), Some("moonshot/kimi-k2.7"));
        assert_eq!(provider.name(), "kimi-chain");
        assert_eq!(model.as_deref(), Some("kimi-k2.7"));
    }

    /// An unmatched prefix must pass through untouched: an aggregator primary
    /// (OpenRouter and friends) wants the qualified id on the wire, and a
    /// deployment with no matching `[providers]` entry has nothing to route to.
    #[test]
    fn spawn_route_leaves_unmatched_prefix_untouched() {
        let parent = named("openrouter");
        let (provider, model) = resolve_spawn_route(
            &parent,
            &HashMap::new(),
            &make_agent_def(),
            Some("anthropic/claude-opus-5"),
        );
        assert_eq!(provider.name(), "openrouter");
        assert_eq!(model, None);
    }

    /// A bare model id keeps the pre-existing behaviour exactly: parent
    /// provider, model stamped by the spawner as given.
    #[test]
    fn spawn_route_bare_model_is_unchanged() {
        let parent = named("anthropic");
        let ovr = overrides(&[("openai", "openai")]);
        for model in [None, Some("gpt-5"), Some("claude-opus-5")] {
            let (provider, routed) = resolve_spawn_route(&parent, &ovr, &make_agent_def(), model);
            assert_eq!(provider.name(), "anthropic");
            assert_eq!(routed, None, "bare id {model:?} must not be rewritten");
        }
    }

    /// Two entries for one vendor resolve deterministically (smallest key), so
    /// the route never depends on HashMap iteration order.
    #[test]
    fn spawn_route_alias_tie_break_is_deterministic() {
        let parent = named("anthropic");
        let ovr = overrides(&[("zeta-gpt", "zeta"), ("alpha-gpt", "alpha")]);
        for _ in 0..8 {
            let (provider, _) =
                resolve_spawn_route(&parent, &ovr, &make_agent_def(), Some("openai/gpt-5"));
            assert_eq!(provider.name(), "alpha");
        }
    }

    /// A blank half is not a qualified id.
    #[test]
    fn split_provider_prefix_rejects_blank_halves() {
        assert_eq!(
            split_provider_prefix("openai/gpt-5"),
            Some(("openai", "gpt-5"))
        );
        assert_eq!(split_provider_prefix("/gpt-5"), None);
        assert_eq!(split_provider_prefix("openai/"), None);
        assert_eq!(split_provider_prefix("gpt-5"), None);
        // Only the first slash is split, so nested aggregator ids keep the rest.
        assert_eq!(
            split_provider_prefix("x-ai/openai/gpt-5"),
            Some(("x-ai", "openai/gpt-5"))
        );
    }

    #[test]
    fn agent_runtime_config_construction() {
        let config = AgentRuntimeConfig {
            agent_def: make_agent_def(),
            task: "Do something".to_string(),
            context_summary: Some("Parent context".to_string()),
            model: Some("claude-sonnet".to_string()),
            timeout_secs: 60,
        };

        assert_eq!(config.task, "Do something");
        assert_eq!(config.timeout_secs, 60);
        assert!(config.context_summary.is_some());
        assert!(config.model.is_some());
    }

    #[test]
    fn transcript_outcome_variants() {
        let success = TranscriptOutcome::Success;
        assert_eq!(format_outcome(&success), "success");

        let error = TranscriptOutcome::Error("something broke".to_string());
        assert_eq!(format_outcome(&error), "error");

        let timeout = TranscriptOutcome::Timeout;
        assert_eq!(format_outcome(&timeout), "timeout");
    }

    #[test]
    fn subagent_transcript_field_access() {
        let transcript = SubagentTranscript {
            agent_id: "test-agent-123".to_string(),
            agent_type: "test-agent".to_string(),
            task_summary: "test task".to_string(),
            outcome: TranscriptOutcome::Success,
            iterations: 5,
            duration_ms: 1200,
            tokens_used: 500,
            key_findings: "some findings".to_string(),
        };

        assert_eq!(transcript.agent_id, "test-agent-123");
        assert_eq!(transcript.agent_type, "test-agent");
        assert_eq!(transcript.task_summary, "test task");
        assert_eq!(transcript.iterations, 5);
        assert_eq!(transcript.duration_ms, 1200);
        assert_eq!(transcript.tokens_used, 500);
    }

    #[test]
    fn truncate_for_log_short_string() {
        assert_eq!(truncate_for_log("hello", 10), "hello");
    }

    #[test]
    fn truncate_for_log_long_string() {
        let s = "a".repeat(200);
        let result = truncate_for_log(&s, 50);
        assert!(result.ends_with("..."));
        assert_eq!(result.len(), 53);
    }

    #[test]
    fn truncate_for_log_unicode_safe() {
        let s = "你好世界这是一个很长的字符串用于测试截断功能";
        let result = truncate_for_log(s, 5);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn transcript_serialization_roundtrip() {
        let transcript = SubagentTranscript {
            agent_id: "test-123".to_string(),
            agent_type: "explorer".to_string(),
            task_summary: "Find all Rust files".to_string(),
            outcome: TranscriptOutcome::Success,
            iterations: 5,
            duration_ms: 1200,
            tokens_used: 3000,
            key_findings: "Found 42 Rust files in src/".to_string(),
        };

        let json = serde_json::to_string(&transcript).unwrap();
        let deserialized: SubagentTranscript = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.agent_id, "test-123");
        assert_eq!(deserialized.iterations, 5);
        assert_eq!(deserialized.key_findings, "Found 42 Rust files in src/");
        assert!(matches!(deserialized.outcome, TranscriptOutcome::Success));
    }

    #[test]
    fn transcript_error_outcome_roundtrip() {
        let transcript = SubagentTranscript {
            agent_id: "err-1".to_string(),
            agent_type: "planner".to_string(),
            task_summary: "Plan feature".to_string(),
            outcome: TranscriptOutcome::Error("timeout".to_string()),
            iterations: 0,
            duration_ms: 5000,
            tokens_used: 0,
            key_findings: String::new(),
        };

        let json = serde_json::to_string(&transcript).unwrap();
        assert!(json.contains("\"timeout\""));
        let deserialized: SubagentTranscript = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized.outcome, TranscriptOutcome::Error(ref e) if e == "timeout"));
    }
}
