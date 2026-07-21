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
    /// Chain ID shared across all depths in a subagent call chain.
    pub chain_id: String,
    /// Nesting depth (0 = root agent).
    pub depth: u32,
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
    /// Welded strategy `<strategy>` body inherited from the parent run.
    /// Threaded into `SpawnRequest.strategy` → the child's inline prompt.
    /// `None` leaves the child prompt byte-identical to the pre-strategy build.
    pub strategy: Option<String>,
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
        }
    }

    /// B15 — wire the parent runner's boot-time iteration cap, inherited by
    /// every spawned child that declares none of its own.
    #[must_use]
    pub const fn with_default_max_iterations(mut self, max_iterations: usize) -> Self {
        self.default_max_iterations = Some(max_iterations);
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
        let _handle = tokio::task::spawn_blocking(move || {
            persist_transcript(&transcript_for_persist, &chain_id_for_persist);
        });

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
        // Phase 3 — resolve the per-agent provider: when `provider_hint` names
        // a registered override, the subagent runs on that provider (pinned,
        // then falling through the global chain); otherwise the shared default.
        let provider = config
            .agent_def
            .provider_hint
            .as_deref()
            .and_then(|hint| self.provider_overrides.get(hint))
            .cloned()
            .unwrap_or_else(|| self.provider.clone());
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
        };
        let req = SpawnRequest {
            agent_def: &config.agent_def,
            task: &config.task,
            context_summary: config.context_summary.as_deref(),
            model: config.model.as_deref(),
            timeout_secs: config.timeout_secs,
            cancel: self.cancel_token.clone(),
            isolation: config.agent_def.isolation.clone(),
            strategy: self.strategy.as_deref().or(config.strategy.as_deref()),
            session_mode: self.session_mode,
        };
        spawn(&base, req).await
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Truncate a string for log output, appending "..." if truncated.
#[must_use]
pub fn truncate_for_log(s: &str, max_len: usize) -> String {
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
pub const fn format_outcome(outcome: &TranscriptOutcome) -> &str {
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
pub fn persist_transcript(transcript: &SubagentTranscript, session_id: &str) {
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

    #[test]
    fn agent_runtime_config_construction() {
        let config = AgentRuntimeConfig {
            agent_def: make_agent_def(),
            task: "Do something".to_string(),
            context_summary: Some("Parent context".to_string()),
            model: Some("claude-sonnet".to_string()),
            timeout_secs: 60,
            strategy: None,
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
