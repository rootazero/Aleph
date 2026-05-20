//! Orchestrator core + seven-step dispatch. See design §6.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::sync_primitives::Mutex;

use tokio::sync::{broadcast, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::orchestrator::errors::FlowError;
use crate::orchestrator::flow_registry::FlowRegistry;
use crate::orchestrator::flow_spec::{AgentId, FlowId, FlowInput, FlowSpec};
use crate::orchestrator::resolver::{
    depth_guard, resolve_flow_id, resolve_session, RoutingOverrides, SessionResolveInput,
};
use crate::orchestrator::sandbox_factory::SandboxFactory;

/// Spawn handle returned to the Gateway.
pub struct FlowHandle {
    pub session_key: String,
    pub events: broadcast::Receiver<FlowStreamEvent>,
    pub completion: oneshot::Receiver<Result<FlowOutcome, FlowError>>,
    pub cancel: CancellationToken,
}

impl std::fmt::Debug for FlowHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlowHandle")
            .field("session_key", &self.session_key)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum FlowStreamEvent {
    /// Incremental assistant text (preserved from original).
    Delta(String),
    /// Thinking/reasoning fragment. Fired when provider returns thinking content.
    Reasoning(String),
    /// Tool call started. `id` uniquely identifies this invocation for pairing
    /// with `ToolCallDone` and `ToolSummary`.
    ToolCallStart {
        id: String,
        name: String,
        args: serde_json::Value,
    },
    /// Tool call completed. `result` and `error` are mutually exclusive.
    ToolCallDone {
        id: String,
        result: Option<serde_json::Value>,
        error: Option<String>,
    },
    /// One-line summary of a tool call (async LLM-generated; silently skipped on failure).
    ToolSummary { id: String, text: String },
    /// Safety gate blocked the turn. `reason` is for i18n formatting.
    SafetyBlock { reason: String },
    /// Stop-hook blocked the turn; harness has forced another model turn.
    StopHookBlock { reason: String },
    /// Model fallback (primary provider unavailable, switched to backup).
    ModelFallback {
        reason: String,
        fallback_model: String,
    },
    /// Terminal event — carries the complete `FlowOutcome`. Always last.
    Complete(FlowOutcome),
}

#[derive(Debug, Clone, Default)]
pub struct FlowOutcome {
    /// Final assistant-visible text for this flow run.
    pub final_text: String,
    /// Number of assistant turns (AssistantMessage events) produced.
    pub iterations: u32,
    /// Number of tool dispatches (ToolCallRequested events).
    pub tool_calls_made: u32,
    /// Sum of provider-reported token usage across the flow run, populated
    /// from the harness cumulative counter (`AgentHarness::total_tokens`)
    /// after the run completes. Saturates to `u32::MAX` on overflow
    /// (unreachable for a realistic run).
    pub total_tokens: u32,
    /// Deprecated. Kept as a backwards-compatibility alias so existing
    /// `outcome.hit_limit` callers keep compiling. Derived from
    /// [`terminate_reason`](Self::terminate_reason); new consumers should
    /// pattern-match the enum so the specific cap (max-iter, stall, turn
    /// timeout, consecutive-failure, verifier-veto) is distinguishable.
    pub hit_limit: bool,
    /// Precise loop-exit cause. Mirrors the distinct branches inside
    /// `AgentHarness::run` that previously collapsed into the boolean
    /// `hit_limit`. Defaults to `Completed` for a clean exit.
    pub terminate_reason: TerminateReason,
    /// Wall-clock duration of the harness run, measured at the orchestrator
    /// bridge so it includes prompt assembly, the Think→Act loop, and trace
    /// flush. `0` when the runner did not record it (test fixtures).
    pub duration_ms: u64,
    /// Per-component provider-reported token usage accumulated across every
    /// LLM call in this run. Granular complement to [`total_tokens`] —
    /// surfaces cache hit ratio and reasoning-token spend that the single
    /// sum hides.
    pub token_breakdown: TokenBreakdown,
    /// One entry per tool invocation in run order. Sourced from the
    /// `LoopTraceEvent::ToolCallCompleted` stream the harness already emits.
    /// Empty when no tool fired or when the harness ran without a trace
    /// sink path that records timeline.
    pub tool_timeline: Vec<ToolInvocation>,
    /// Best-effort USD cost estimate. `None` when the pricing module
    /// could not resolve the provider/model — pricing is non-load-bearing
    /// (see [`crate::pricing`]).
    pub estimated_cost: Option<crate::pricing::CostEstimate>,
}

/// Loop-exit cause for an agent run. Each variant corresponds to a distinct
/// branch in [`AgentHarness::run`](crate::harness::agent::AgentHarness)
/// that previously collapsed into a single `hit_limit: bool`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum TerminateReason {
    /// Model emitted a terminal `TurnState::Done` and the loop exited cleanly.
    Completed,
    /// `max_iterations` cap reached. `used` is the iteration count that
    /// tripped the guard.
    HitMaxIterations { used: u32 },
    /// `context_budget` directive forced an early `FinalReply` because the
    /// running prompt would otherwise blow the model's context window.
    /// Distinct from `HitMaxIterations` — same outward symptom, different
    /// cause (tokens vs iterations).
    ContextBudgetExhausted,
    /// `stall_config` watchdog tripped before the loop emitted progress.
    StallTimeout { elapsed_ms: u64 },
    /// Per-turn timeout exhausted in `phase` (think / act).
    TurnTimeout { phase: String, elapsed_ms: u64 },
    /// Consecutive failure cap reached after `consecutive` turns produced
    /// only `ToolError` events.
    ConsecutiveFailureCap { consecutive: u32 },
    /// Verifier veto count reached `MAX_VERIFIER_VETOS`.
    VerifierVeto { vetos: u32 },
    /// `CancellationToken` fired before the loop reached `Done`.
    Cancelled,
}

impl Default for TerminateReason {
    fn default() -> Self {
        Self::Completed
    }
}

impl TerminateReason {
    /// `true` for every cap-style exit. Equivalent to the legacy
    /// `hit_limit: bool`; `Completed` and `Cancelled` return `false`. Use
    /// to populate [`FlowOutcome::hit_limit`] so the two fields never drift.
    pub fn is_hit_limit(&self) -> bool {
        !matches!(self, Self::Completed | Self::Cancelled)
    }

    /// Short stable string for logging / metrics — never localized.
    pub fn as_static_str(&self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::HitMaxIterations { .. } => "hit_max_iterations",
            Self::ContextBudgetExhausted => "context_budget_exhausted",
            Self::StallTimeout { .. } => "stall_timeout",
            Self::TurnTimeout { .. } => "turn_timeout",
            Self::ConsecutiveFailureCap { .. } => "consecutive_failure_cap",
            Self::VerifierVeto { .. } => "verifier_veto",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Per-component provider-reported token usage accumulated across every LLM
/// call in a run.
///
/// Component semantics follow [`crate::providers::adapter::TokenUsage`]:
/// `input` + `output` form the raw call cost; `cache_read` +
/// `cache_creation` indicate Anthropic prompt cache behaviour; `reasoning`
/// is the Gemini `thoughtsTokenCount` / OpenAI o-series reasoning count
/// (Anthropic folds it into `output`).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TokenBreakdown {
    pub input: u32,
    pub output: u32,
    pub cache_read: u32,
    pub cache_creation: u32,
    pub reasoning: u32,
}

impl TokenBreakdown {
    /// Saturating sum of every component except `reasoning` — matches the
    /// `turn_token_total` definition in the harness so this stays in lockstep
    /// with [`FlowOutcome::total_tokens`].
    pub fn total(&self) -> u64 {
        u64::from(self.input)
            + u64::from(self.output)
            + u64::from(self.cache_read)
            + u64::from(self.cache_creation)
    }

    /// Fold one `ProviderUsage` trace event into this breakdown. `None`
    /// values default to 0; sums are saturating at `u32::MAX`.
    pub fn accumulate(
        &mut self,
        input: u32,
        output: u32,
        cache_read: Option<u32>,
        cache_creation: Option<u32>,
        reasoning: Option<u32>,
    ) {
        self.input = self.input.saturating_add(input);
        self.output = self.output.saturating_add(output);
        self.cache_read = self.cache_read.saturating_add(cache_read.unwrap_or(0));
        self.cache_creation = self
            .cache_creation
            .saturating_add(cache_creation.unwrap_or(0));
        self.reasoning = self.reasoning.saturating_add(reasoning.unwrap_or(0));
    }
}

/// One tool invocation in run order. Sourced from
/// `LoopTraceEvent::ToolCallCompleted` — the harness already records every
/// field; the timeline is the missing aggregation seam.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ToolInvocation {
    /// Provider-supplied tool_use id (correlates with the matching
    /// `FlowStreamEvent::ToolCallStart`).
    pub id: String,
    /// Resolved tool name (`bash`, `web_search`, …).
    pub name: String,
    /// Wall-clock duration measured by the harness around `tool.execute()`.
    pub duration_ms: u64,
    /// `true` when the underlying `ToolResult` was `Success` /
    /// `SuccessAndStopLoop`; `false` on `Error`.
    pub success: bool,
    /// Provider error message when [`success`] is `false`.
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct FlowRequest {
    pub flow_id: Option<FlowId>,
    pub agent_id: AgentId,
    pub input: FlowInput,
    pub channel: Option<String>,
    /// Client-supplied routing hint for session affinity. When `session_strategy`
    /// resolves to `Reuse`, the hint is passed through; otherwise a fresh key is
    /// generated.
    ///
    /// # Cross-user collision warning
    /// Hints must be unique per user context. If two concurrent requests from
    /// different users share the same hint value, the Gateway's orchestrator will
    /// treat them as conflicting sessions (`FlowError::SessionConflict`), causing
    /// one request to be rejected. In multi-user deployments, namespace hints with
    /// a user-scoped prefix (e.g. `user:{user_id}:session:{hint}`) or rely on
    /// `parent_session` for isolation rather than bare hints.
    pub session_hint: Option<String>,
    pub parent_session: Option<String>,
    pub depth: u8,
    /// Per-request tool service override. `None` causes `AgentHarnessRunner`
    /// to fall back to its default `tool_service`. Gateway production path
    /// must supply `Some` to carry per-request dynamic tools.
    /// Not included in `Debug` output because `Arc<dyn ToolService>` is not `Debug`.
    pub tool_service: Option<std::sync::Arc<dyn crate::tools::service::ToolService>>,
    /// Gateway-side observability sink. `None` is a no-op.
    /// Not included in `Debug` output because `Arc<dyn TraceSink>` is not `Debug`.
    pub trace_sink: Option<std::sync::Arc<dyn crate::harness::TraceSink>>,
}

impl std::fmt::Debug for FlowRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlowRequest")
            .field("flow_id", &self.flow_id)
            .field("agent_id", &self.agent_id)
            .field("input", &self.input)
            .field("channel", &self.channel)
            .field("session_hint", &self.session_hint)
            .field("parent_session", &self.parent_session)
            .field("depth", &self.depth)
            .field(
                "tool_service",
                &self.tool_service.as_ref().map(|_| "<dyn ToolService>"),
            )
            .field(
                "trace_sink",
                &self.trace_sink.as_ref().map(|_| "<dyn TraceSink>"),
            )
            .finish()
    }
}

/// Hardcoded broadcast channel buffer size. Each receiver gets its own
/// circular buffer of this capacity; old messages are overwritten when a
/// receiver falls behind. The sender never blocks.
/// 256 is a deliberate balance: large enough for bursts of rapid tool calls,
/// small enough to bound memory overhead per active flow.
const CHANNEL_BUFFER_SIZE: usize = 256;

/// Orchestrator dependencies. Most are behind `Arc<dyn Trait>` so the struct
/// itself is cheap to share. Per-session lock is an internal `Mutex<HashSet>`.
pub struct Orchestrator {
    pub flow_registry: Arc<FlowRegistry>,
    pub routing_overrides: Arc<RoutingOverrides>,
    pub default_routing: Arc<HashMap<AgentId, FlowId>>,
    pub session_service: Arc<dyn crate::session::service::SessionService>,
    pub sandbox_factory: SandboxFactory,
    /// Harness runner injected at construction — test mocks can swap this out.
    pub harness: Arc<dyn HarnessRunner>,
    /// Phase 3 — provider routing for spawned subagents (the global failover
    /// chain + per-`provider_hint` overrides). `None` for test mocks and the
    /// simple engine; `Some` once `build_failover_chain` has run at boot.
    pub subagent_routing: Option<crate::orchestrator::deps_builder::ProviderChain>,
    active_sessions: Arc<Mutex<HashSet<String>>>,
}

/// Removes `key` from `active` on drop — runs even if the spawned
/// harness task panics.
struct SessionLockGuard {
    active: Arc<Mutex<HashSet<String>>>,
    key: String,
}

impl Drop for SessionLockGuard {
    fn drop(&mut self) {
        let mut guard = self.active.lock().unwrap_or_else(|e| e.into_inner());
        guard.remove(&self.key);
    }
}

#[async_trait::async_trait]
#[allow(clippy::too_many_arguments)] // trait shape driven by Orchestrator::dispatch wiring (Task 2)
pub trait HarnessRunner: Send + Sync {
    async fn run(
        &self,
        session_key: String,
        spec: Arc<FlowSpec>,
        input: FlowInput,
        sandbox: Arc<dyn crate::sandbox::Sandbox>,
        events: broadcast::Sender<FlowStreamEvent>,
        cancel: CancellationToken,
        tool_service_override: Option<std::sync::Arc<dyn crate::tools::service::ToolService>>,
        trace_sink: Option<std::sync::Arc<dyn crate::harness::TraceSink>>,
    ) -> Result<FlowOutcome, FlowError>;
}

impl Orchestrator {
    pub fn new(
        flow_registry: Arc<FlowRegistry>,
        routing_overrides: Arc<RoutingOverrides>,
        default_routing: Arc<HashMap<AgentId, FlowId>>,
        session_service: Arc<dyn crate::session::service::SessionService>,
        sandbox_factory: SandboxFactory,
        harness: Arc<dyn HarnessRunner>,
    ) -> Self {
        Self {
            flow_registry,
            routing_overrides,
            default_routing,
            session_service,
            sandbox_factory,
            harness,
            subagent_routing: None,
            active_sessions: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Phase 3 — attach the subagent provider routing built by
    /// `build_failover_chain`. The gateway execution engine reads it to route
    /// spawned subagents through the failover chain and to honour
    /// `AgentDef.provider_hint`.
    pub fn with_subagent_routing(
        mut self,
        routing: crate::orchestrator::deps_builder::ProviderChain,
    ) -> Self {
        self.subagent_routing = Some(routing);
        self
    }

    /// Seven-step dispatch. See design §6.
    pub async fn dispatch(&self, req: FlowRequest) -> Result<FlowHandle, FlowError> {
        // Step 2: depth guard (cheap, first, to reject runaway callers).
        depth_guard(req.depth)?;

        // Step 1: resolve flow_id → FlowSpec.
        let flow_id = match &req.flow_id {
            Some(id) => id.clone(),
            None => resolve_flow_id(
                &req.agent_id,
                req.channel.as_deref(),
                &self.routing_overrides,
                &self.default_routing,
            )?,
        };
        let spec = self
            .flow_registry
            .resolve(&flow_id)
            .ok_or_else(|| FlowError::UnknownFlow(flow_id.clone()))?;

        // Step 3: agent lookup deferred to harness (it holds the AgentRegistry).

        // Step 4: session resolve + per-session lock.
        let session_input = SessionResolveInput {
            strategy: spec.session_strategy.clone(),
            session_hint: req.session_hint.clone(),
            parent_session: req.parent_session.clone(),
            fresh_key_fn: || uuid::Uuid::new_v4().to_string(),
        };
        let session_res = resolve_session(session_input)?;
        {
            let mut guard = self
                .active_sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if !guard.insert(session_res.session_key.clone()) {
                return Err(FlowError::SessionConflict(session_res.session_key));
            }
        }

        // Step 5: brain pick deferred to HarnessRunner (ProviderRegistry lives there).

        // Step 6: sandbox provision.
        let sandbox = match (self.sandbox_factory)(spec.sandbox_kind, &session_res.session_key) {
            Ok(sb) => sb,
            Err(e) => {
                let mut guard = self
                    .active_sessions
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                guard.remove(&session_res.session_key);
                return Err(e);
            }
        };

        // Step 7: spawn harness, plumbing events + completion + cancel.
        let (event_tx, event_rx) = broadcast::channel::<FlowStreamEvent>(CHANNEL_BUFFER_SIZE);
        let (done_tx, done_rx) = oneshot::channel();
        let cancel = CancellationToken::new();

        let harness = self.harness.clone();
        let spec_clone = spec.clone();
        let input_clone = req.input.clone();
        let sandbox_clone = sandbox.clone();
        let cancel_clone = cancel.clone();
        let session_key = session_res.session_key.clone();
        let active = self.active_sessions.clone();
        let session_for_release = session_res.session_key.clone();
        let tool_service_override = req.tool_service.clone();
        let trace_sink = req.trace_sink.clone();

        tokio::spawn(async move {
            let _lock = SessionLockGuard {
                active,
                key: session_for_release,
            };
            let outcome = harness
                .run(
                    session_key,
                    spec_clone,
                    input_clone,
                    sandbox_clone,
                    event_tx,
                    cancel_clone,
                    tool_service_override,
                    trace_sink,
                )
                .await;
            // `done_tx.send` returns Err only when `done_rx` was dropped by the
            // caller — meaning the handle was abandoned and no one is waiting for
            // the result. Dropping the result is intentional; a `return` or `?`
            // here would propagate an error that signals nothing actionable.
            let _ = done_tx.send(outcome);
            // _lock drops here, releasing the session key regardless of panic.
        });

        Ok(FlowHandle {
            session_key: session_res.session_key,
            events: event_rx,
            completion: done_rx,
            cancel,
        })
    }

    pub async fn reload_flows(
        &self,
        new_set: crate::orchestrator::flow_registry::FlowSet,
    ) -> Result<(), FlowError> {
        self.flow_registry.replace(new_set);
        debug!(count = self.flow_registry.len(), "flow registry reloaded");
        Ok(())
    }
}

#[cfg(test)]
mod outcome_tests {
    use super::*;

    #[test]
    fn flow_outcome_default_is_completed_clean_run() {
        let outcome = FlowOutcome::default();
        assert_eq!(outcome.terminate_reason, TerminateReason::Completed);
        assert!(!outcome.hit_limit);
        assert_eq!(outcome.duration_ms, 0);
        assert_eq!(outcome.total_tokens, 0);
        assert_eq!(outcome.token_breakdown, TokenBreakdown::default());
        assert!(outcome.tool_timeline.is_empty());
        assert!(outcome.estimated_cost.is_none());
    }

    #[test]
    fn terminate_reason_is_hit_limit_matches_legacy_bool_semantics() {
        // Completed / Cancelled are NOT cap-style exits.
        assert!(!TerminateReason::Completed.is_hit_limit());
        assert!(!TerminateReason::Cancelled.is_hit_limit());

        // Every other variant IS a cap.
        assert!(TerminateReason::HitMaxIterations { used: 200 }.is_hit_limit());
        assert!(TerminateReason::ContextBudgetExhausted.is_hit_limit());
        assert!(TerminateReason::StallTimeout { elapsed_ms: 30_000 }.is_hit_limit());
        assert!(TerminateReason::TurnTimeout {
            phase: "think".into(),
            elapsed_ms: 60_000
        }
        .is_hit_limit());
        assert!(TerminateReason::ConsecutiveFailureCap { consecutive: 3 }.is_hit_limit());
        assert!(TerminateReason::VerifierVeto { vetos: 10 }.is_hit_limit());
    }

    #[test]
    fn terminate_reason_static_str_is_stable() {
        assert_eq!(TerminateReason::Completed.as_static_str(), "completed");
        assert_eq!(
            TerminateReason::HitMaxIterations { used: 5 }.as_static_str(),
            "hit_max_iterations"
        );
        assert_eq!(
            TerminateReason::ContextBudgetExhausted.as_static_str(),
            "context_budget_exhausted"
        );
        assert_eq!(
            TerminateReason::StallTimeout { elapsed_ms: 0 }.as_static_str(),
            "stall_timeout"
        );
        assert_eq!(
            TerminateReason::TurnTimeout {
                phase: "act".into(),
                elapsed_ms: 0
            }
            .as_static_str(),
            "turn_timeout"
        );
        assert_eq!(
            TerminateReason::ConsecutiveFailureCap { consecutive: 0 }.as_static_str(),
            "consecutive_failure_cap"
        );
        assert_eq!(
            TerminateReason::VerifierVeto { vetos: 0 }.as_static_str(),
            "verifier_veto"
        );
        assert_eq!(TerminateReason::Cancelled.as_static_str(), "cancelled");
    }

    #[test]
    fn token_breakdown_accumulate_sums_components_with_none_as_zero() {
        let mut b = TokenBreakdown::default();
        b.accumulate(100, 250, Some(40), Some(10), Some(999));
        assert_eq!(b.input, 100);
        assert_eq!(b.output, 250);
        assert_eq!(b.cache_read, 40);
        assert_eq!(b.cache_creation, 10);
        assert_eq!(b.reasoning, 999);
        // total() mirrors harness `turn_token_total`: input+output+cache_read+cache_creation
        assert_eq!(b.total(), 400);

        b.accumulate(5, 10, None, None, None);
        assert_eq!(b.input, 105);
        assert_eq!(b.output, 260);
        assert_eq!(b.cache_read, 40);
        assert_eq!(b.cache_creation, 10);
        assert_eq!(b.reasoning, 999);
        assert_eq!(b.total(), 415);
    }

    #[test]
    fn token_breakdown_accumulate_saturates_at_u32_max() {
        let mut b = TokenBreakdown {
            input: u32::MAX - 1,
            ..Default::default()
        };
        b.accumulate(5, 0, None, None, None);
        assert_eq!(b.input, u32::MAX);
    }

    #[test]
    fn tool_invocation_roundtrips_through_serde() {
        let inv = ToolInvocation {
            id: "toolu_01".into(),
            name: "bash".into(),
            duration_ms: 1234,
            success: false,
            error: Some("exit code 1".into()),
        };
        let json = serde_json::to_string(&inv).expect("serialize");
        let back: ToolInvocation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(inv, back);
    }

    #[test]
    fn terminate_reason_roundtrips_through_serde_with_payload() {
        let r = TerminateReason::TurnTimeout {
            phase: "think".into(),
            elapsed_ms: 60_000,
        };
        let json = serde_json::to_string(&r).expect("serialize");
        assert!(json.contains(r#""kind":"turn_timeout""#));
        assert!(json.contains(r#""phase":"think""#));
        let back: TerminateReason = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(r, back);
    }

    #[test]
    fn flow_outcome_hit_limit_field_can_be_derived_from_terminate_reason() {
        // Demonstrate the construction pattern callers should use to keep
        // `hit_limit` and `terminate_reason` in lockstep.
        let reason = TerminateReason::HitMaxIterations { used: 200 };
        let outcome = FlowOutcome {
            hit_limit: reason.is_hit_limit(),
            terminate_reason: reason,
            ..Default::default()
        };
        assert!(outcome.hit_limit);
        assert_eq!(
            outcome.terminate_reason,
            TerminateReason::HitMaxIterations { used: 200 }
        );
    }
}
