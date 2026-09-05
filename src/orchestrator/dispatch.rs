//! Orchestrator core + seven-step dispatch. See design §6.

use crate::sync_primitives::Arc;
use futures::FutureExt;
use std::collections::{HashMap, HashSet};

use crate::sync_primitives::Mutex;

use tokio::sync::{broadcast, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::orchestrator::errors::FlowError;
use crate::orchestrator::flow_registry::FlowRegistry;
use crate::orchestrator::flow_spec::{AgentId, FlowId, FlowInput, FlowSpec};
use crate::orchestrator::resolver::{
    depth_guard, resolve_flow_id, resolve_session, SessionResolveInput, DEFAULT_AGENT_FLOW_ID,
};
use crate::orchestrator::sandbox_factory::SandboxFactory;

/// Spawn handle returned to the Gateway.
pub struct FlowHandle {
    pub session_key: String,
    /// Parent session key resolved by `SessionStrategy::Child`, if any.
    /// The gateway MUST thread this through to the session store row so
    /// child sessions preserve their lineage. `None` for root sessions
    /// (i.e. the original `req.parent_session` was `None` or `Child`
    /// resolution produced a fresh key).
    pub parent_session_key: Option<String>,
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
// rust-doctor-disable-next-line large-enum-variant
pub enum FlowStreamEvent {
    /// Incremental assistant text (preserved from original).
    Delta(String),
    /// Thinking/reasoning fragment. Fired when provider returns thinking content.
    Reasoning(String),
    /// Tool call started. `id` uniquely identifies this invocation for pairing
    /// with `ToolCallDone`.
    ToolCallStart {
        id: String,
        name: String,
        args: serde_json::Value,
    },
    /// Tool call completed. `result` and `error` are mutually exclusive.
    /// `duration_ms` is the tool's measured wall-clock execution time, so the
    /// live `StreamEvent::ToolEnd` carries it instead of waiting for the
    /// end-of-run `RunSummary.tool_summaries`.
    ToolCallDone {
        id: String,
        result: Option<serde_json::Value>,
        error: Option<String>,
        duration_ms: u64,
    },
    /// Live context-window occupancy after one LLM call. Emitted once per
    /// call (`HarnessCallback::on_context_usage`) so the panel gauge tracks a
    /// long run in real time — including the drop right after a mid-run
    /// compaction. `context_window` is the run's pre-resolved denominator.
    ContextGauge {
        context_tokens: u32,
        context_window: u32,
        total_tokens: u64,
    },
    /// Safety gate blocked the turn. `reason` is for i18n formatting.
    SafetyBlock { reason: String },
    /// Terminal event — carries the complete `FlowOutcome`. Always last.
    Complete(FlowOutcome),
}

#[derive(Debug, Clone, Default)]
pub struct FlowOutcome {
    /// Final assistant-visible text for this flow run.
    pub final_text: String,
    /// Number of assistant turns (`AssistantMessage` events) produced.
    pub iterations: u32,
    /// Number of tool dispatches (`ToolCallRequested` events).
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
    /// Tokens occupying the model's context window as of the run's most recent
    /// LLM call (last prompt sent + that call's output). Gauge numerator —
    /// distinct from cumulative [`total_tokens`]. `0` when no LLM call ran.
    pub context_tokens: u32,
    /// Authoritative context-window size (tokens) for this run's model, with a
    /// conservative fallback for unknown models. Gauge denominator. `0` only on
    /// default/test fixtures.
    pub context_window: u32,
    /// The model that actually served this run, and the provider that served
    /// it. Same pair the cost estimate is keyed on — never the `FailoverProvider`
    /// wrapper name. Carried out so the session row can record which model it
    /// last spoke to; `sessions.model` had a column, a Panel column, and no
    /// writer that ever ran.
    pub serving_model: Option<String>,
    /// Provider id behind [`serving_model`](Self::serving_model).
    pub serving_provider: Option<String>,
}

/// Loop-exit cause for an agent run. Each variant corresponds to a distinct
/// branch in [`AgentHarness::run`](crate::harness::agent::AgentHarness)
/// that previously collapsed into a single `hit_limit: bool`.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum TerminateReason {
    /// Model emitted a terminal `TurnState::Done` and the loop exited cleanly.
    #[default]
    Completed,
    /// `max_iterations` cap reached. `used` is the iteration count that
    /// tripped the guard.
    HitMaxIterations { used: u32 },
    /// `context_budget` directive forced an early `FinalReply` because the
    /// running prompt would otherwise blow the model's context window.
    /// Distinct from `HitMaxIterations` — same outward symptom, different
    /// cause (tokens vs iterations).
    ///
    /// **Retired signal, like [`Self::DiminishingReturns`]: no producer.** The
    /// budget's `FinalReply` hard-stop is gone — pressure now escalates
    /// `SplitSession` → `CompactToFit`'s deterministic truncation floor and the
    /// loop never breaks on it (pinned by the harness's never-break tests).
    /// Kept for i18n / `escalate_partial_result` back-compat and for
    /// blobs persisted before the escalation landed.
    ContextBudgetExhausted,
    /// `stall_config` watchdog tripped before the loop emitted progress.
    StallTimeout { elapsed_ms: u64 },
    /// Per-turn timeout exhausted in `phase` (think / act).
    TurnTimeout { phase: String, elapsed_ms: u64 },
    /// Consecutive failure cap reached after `consecutive` turns produced
    /// only `ToolError` events.
    ConsecutiveFailureCap { consecutive: u32 },
    /// Verifier veto count reached the per-model `steer_max` cap.
    VerifierVeto { vetos: u32 },
    /// The provider returned a response with no text, no `tool_calls` and no
    /// thinking on every attempt, including bounded retries. The user
    /// received no answer — distinct from a clean `Completed`.
    EmptyResponseExhausted,
    /// A Stop hook returned `Halt` (exit code 3 from `ShellStopHook`).
    /// Mirrors claude-code's `preventContinuation: true` exit-protocol:
    /// the loop ends immediately with the hook's reason surfaced to the
    /// user. Distinct from `VerifierVeto` (which forces one Continue +
    /// retry) — Halt is a permanent stop signal from policy.
    StopHookHalt { reason: String },
    /// The provider returned `stop_reason=MaxTokens` on every attempt
    /// including the bounded `max_output_tokens` recovery retries. The
    /// model could not finish its response within the allowed budget;
    /// distinct from `Completed` (clean exit) and `EmptyResponseExhausted`
    /// (no content at all).
    MaxOutputTokensExhausted,
    /// Retired signal, kept for i18n / summary back-compat only. The
    /// context-budget diminishing-returns hard-stop was removed (R10: the
    /// harness must not make its own completion judgement — the model, plus
    /// the harder caps `max_iterations` / `ToolLoopVerifier` / the
    /// consecutive-failure cap, govern instead). No longer produced by the loop.
    DiminishingReturns,
    /// The provider returned a context-window overflow (`prompt_too_long`
    /// / 413) and the harness attempted reactive compaction, but either
    /// the rescue retry still overflowed or no compactor was wired. The
    /// `RetryVerdict::CompactAndRetry` path used to leak into
    /// `HarnessError::Llm` before this variant existed — see the wiring
    /// in `context::compact::rescue::try_reactive_compact_and_retry`.
    ReactiveCompactExhausted,
    /// `CancellationToken` fired before the loop reached `Done`.
    Cancelled,
    /// The run ended in a `HarnessError` and no site inside the loop had
    /// recorded a cause of its own — a provider auth failure, a transport
    /// error, a guardrail abort. Distinct from [`Completed`](Self::Completed),
    /// which is this enum's `Default` and therefore what an unrecorded failure
    /// reported on every terminal frame before this variant existed.
    ///
    /// Written by the orchestrator bridge (`AgentHarnessRunner::run`), not by
    /// the loop. The loop's own `Err` arm deliberately writes nothing but
    /// `Cancelled`, so that the causes it *did* record (`ReactiveCompactExhausted`,
    /// `StopHookHalt`, the caps) survive the exit; the bridge fills this in
    /// only when the reason is still the default, which is unambiguous because
    /// nothing in the crate ever assigns `Completed` (pinned by
    /// `harness_bridge::tests::no_site_records_a_completed_terminate_reason`).
    ///
    /// **Not a cap**: [`is_hit_limit`](Self::is_hit_limit) is `false`, next to
    /// `Completed` and `Cancelled`. The failure text itself reaches the user on
    /// the separate `RunError` frame — this variant exists only to stop the
    /// terminal summary from claiming the run finished cleanly.
    Failed,
    /// A budget cap (`HitMaxIterations` / `ContextBudgetExhausted` /
    /// `MaxOutputTokensExhausted`) fired AFTER the model already emitted
    /// useful partial text in this run. The harness preserves the
    /// partial via `partial_summary` so downstream consumers (cron
    /// carry-over, CLI resume hints) can pick the run back up next
    /// time. `reason` is the stable token of the underlying cap
    /// (`"hit_max_iterations"`, `"context_budget_exhausted"`,
    /// `"max_output_tokens_exhausted"`), so existing dashboards that
    /// branch on the bare label keep working.
    ///
    /// Emission is policy-gated in
    /// [`escalate_partial_result`] — when there is no partial text the
    /// harness keeps emitting the bare variant, preserving prior
    /// behaviour byte-for-byte.
    BudgetExhaustedPartialResult {
        reason: String,
        partial_summary: String,
    },
}

impl TerminateReason {
    /// `true` for every cap-style exit. Equivalent to the legacy
    /// `hit_limit: bool`; `Completed`, `Cancelled` and `Failed` return
    /// `false`. Use to populate [`FlowOutcome::hit_limit`] so the two fields
    /// never drift.
    ///
    /// `Failed` sits with the other two on purpose: a run that died on a
    /// provider error did not *reach* a limit, and the surfaces that gate on
    /// this flag say "you ran out of budget, raise it" — advice that is simply
    /// wrong for a crash.
    #[must_use]
    pub const fn is_hit_limit(&self) -> bool {
        !matches!(self, Self::Completed | Self::Cancelled | Self::Failed)
    }

    /// Short stable string for logging / metrics — never localized.
    #[must_use]
    pub const fn as_static_str(&self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::HitMaxIterations { .. } => "hit_max_iterations",
            Self::ContextBudgetExhausted => "context_budget_exhausted",
            Self::StallTimeout { .. } => "stall_timeout",
            Self::TurnTimeout { .. } => "turn_timeout",
            Self::ConsecutiveFailureCap { .. } => "consecutive_failure_cap",
            Self::VerifierVeto { .. } => "verifier_veto",
            Self::EmptyResponseExhausted => "empty_response_exhausted",
            Self::StopHookHalt { .. } => "stop_hook_halt",
            Self::MaxOutputTokensExhausted => "max_output_tokens_exhausted",
            Self::DiminishingReturns => "diminishing_returns",
            Self::ReactiveCompactExhausted => "reactive_compact_exhausted",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::BudgetExhaustedPartialResult { .. } => "budget_exhausted_partial_result",
        }
    }
}

/// Stable label that downstream consumers (cron carry-over, log
/// filters, metric dashboards) recognise as "this run produced partial
/// work that should be carried forward". Lives outside the enum so
/// non-Rust consumers (TOML / panel JS) can grep the string without
/// depending on the Rust binding.
pub const BUDGET_PARTIAL_RESULT_LABEL: &str = "budget_exhausted_partial_result";

/// Escalate a bare budget-cap reason to
/// [`TerminateReason::BudgetExhaustedPartialResult`] when the run
/// produced any useful final text. Non-budget reasons and runs with no
/// partial output pass through unchanged.
///
/// Called once in `AgentHarnessRunner::run` just after
/// `harness.terminate_reason()` so the policy lives in one place
/// rather than at each of the 4 emit sites inside the harness loop.
///
/// Why this can't live inside the harness loop:
/// the partial-text inspection happens AFTER `final_text` extraction
/// (which itself reads back the session log post-loop), so by the time
/// we know "is there partial text?" the harness has already returned
/// and only the orchestrator sees both signals.
#[must_use]
pub fn escalate_partial_result(reason: TerminateReason, partial: Option<&str>) -> TerminateReason {
    let Some(text) = partial else {
        return reason;
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return reason;
    }
    let base_label = reason.as_static_str();
    match reason {
        TerminateReason::HitMaxIterations { .. }
        | TerminateReason::ContextBudgetExhausted
        | TerminateReason::MaxOutputTokensExhausted => {
            TerminateReason::BudgetExhaustedPartialResult {
                reason: base_label.to_string(),
                partial_summary: trimmed.to_string(),
            }
        }
        // Every other terminate reason is either a clean exit
        // (`Completed`), a structural failure (`StallTimeout`,
        // `TurnTimeout`, `ConsecutiveFailureCap`, `VerifierVeto`,
        // `EmptyResponseExhausted`), a permanent halt (`StopHookHalt`),
        // an externally-driven stop (`Cancelled`), a productivity stop
        // (`DiminishingReturns` — resuming would just resume the spin),
        // or already a PartialResult — none should be re-classified as
        // "partial, resume me later".
        other => other,
    }
}

/// Per-component provider-reported token usage accumulated across every LLM
/// call in a run.
///
/// Component semantics follow [`crate::providers::adapter::TokenUsage`]:
/// `input` + `output` form the raw call cost; `cache_read` +
/// `cache_creation` indicate Anthropic prompt cache behaviour; `reasoning`
/// is the Gemini `thoughtsTokenCount` / `OpenAI` o-series reasoning count
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
    #[must_use]
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

impl From<&crate::providers::adapter::TokenUsage> for TokenBreakdown {
    /// One call's usage as a breakdown of its own. The run-cumulative counter
    /// folds these with [`TokenBreakdown::accumulate`]; this is the same fold
    /// starting from zero, so a single call's breakdown and its contribution to
    /// the run total are the same arithmetic by construction.
    fn from(u: &crate::providers::adapter::TokenUsage) -> Self {
        let mut b = Self::default();
        b.accumulate(
            u.input_tokens,
            u.output_tokens,
            u.cache_read_tokens,
            u.cache_creation_tokens,
            u.thinking_tokens,
        );
        b
    }
}

/// One tool invocation in run order. Sourced from
/// `LoopTraceEvent::ToolCallCompleted` — the harness already records every
/// field; the timeline is the missing aggregation seam.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ToolInvocation {
    /// Provider-supplied `tool_use` id (correlates with the matching
    /// `FlowStreamEvent::ToolCallStart`).
    pub id: String,
    /// Resolved tool name (`bash`, `web_search`, …).
    pub name: String,
    /// Wall-clock duration measured by the harness around `tool.execute()`.
    pub duration_ms: u64,
    /// `true` when the underlying `ToolResult` was `Success`;
    /// `false` on `Error`.
    pub success: bool,
    /// Provider error message when [`success`] is `false`.
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct FlowRequest {
    pub flow_id: Option<FlowId>,
    pub agent_id: AgentId,
    pub input: FlowInput,
    /// Originating platform (`request.metadata["platform"]`), for diagnostics
    /// only — it appears in this struct's `Debug` output and nowhere else.
    /// It is **not** a routing input: the `(agent, channel)` override rung that
    /// once read it was cut with `RoutingOverrides` (see
    /// `resolver::resolve_flow_id`). The channel fact still reaches the prompt,
    /// but through `interaction_manifest`, which is built from the same
    /// metadata key.
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
    /// P1 data isolation: the run's owner/scope attribution. `FlowRequest`
    /// has no metadata map (unlike `RunRequest`), so this rides as the pair of
    /// strings [`crate::scope::stamp_metadata`] would have written, wrapped in
    /// [`crate::scope::FlowScope`].
    ///
    /// The wrapper is the point. Its fields are private and its only non-empty
    /// constructor takes an already-resolved `ScopeAttribution`, so the one
    /// production filler of this field
    /// (`gateway::execution_engine::run_loop::request_scope_strings`) cannot be
    /// replaced by a read of `RunRequest.metadata` — that pair is
    /// `(Option<String>, Option<String>)` and no longer fits. `src/gateway/
    /// CLAUDE.md` 地雷 Q is the incident this closes; the type's own doc says
    /// what it does not close.
    ///
    /// Callers inside `process_request`'s task tree may inherit the ambient
    /// `crate::scope::current_scope()`; callers with no live task-local
    /// (cron, hook-less wakes) must pass it explicitly.
    /// [`FlowScope::unscoped`](crate::scope::FlowScope::unscoped) = unscoped
    /// (legacy owner semantics) — see `Orchestrator::dispatch`, which
    /// re-derives a `ScopeAttribution` from the pair via
    /// `crate::scope::scope_from_metadata` and re-seeds it inside the spawn.
    pub scope: crate::scope::FlowScope,
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
    /// Phase 4 (F4): channel-aware interaction envelope. When `Some`, the
    /// harness bridge threads this `InteractionManifest` into the
    /// `ResolvedContext` used for prompt assembly, so per-channel
    /// constraints (paradigm, capabilities, output limits) reach the
    /// LLM. `None` keeps the legacy `Background` default — appropriate
    /// for subagent dispatch and internal tooling that runs without a
    /// channel.
    pub interaction_manifest: Option<crate::thinker::interaction::InteractionManifest>,
    /// G2 — per-request sandbox override. `None` falls back to
    /// `Orchestrator::sandbox_factory(&session_key)` — the pre-G2 default.
    /// `Some(sandbox)` short-circuits the factory; used by the
    /// team dispatcher to inject a `WorktreeSandbox` so concurrent team
    /// members write to isolated git worktrees and cannot corrupt each
    /// other's index.
    ///
    /// Not included in `Debug` output because `Arc<dyn Sandbox>` is not `Debug`.
    pub sandbox_override: Option<std::sync::Arc<dyn crate::sandbox::Sandbox>>,
    /// Per-request workspace override (project mode). When `Some`, this path
    /// is persisted on the `RunStarted` marker so a crash-resume can land
    /// back in the same project folder, and it surfaces to `HarnessRunner`
    /// implementations that need to scope their work to a user-picked
    /// directory rather than `~/.aleph/workspaces/{agent_id}/`. `None`
    /// keeps the legacy agent-workspace behaviour.
    pub workspace_override: Option<std::path::PathBuf>,
    /// D2: per-run override that wins over both `FlowOverrides.max_iterations`
    /// and the boot-time `[execution] max_iterations` default. Forwarded
    /// by `Orchestrator::dispatch` into `HarnessRunner::run` so cron-driven
    /// flows can cap themselves tightly without lowering the global default.
    pub max_iterations_override: Option<u32>,
    /// Ephemeral per-turn prompt context assembled by the gateway run_loop:
    /// the effective working-directory reminder, the project's own skill list,
    /// and any `UserPromptSubmit` hook additions — each fenced in
    /// `<system-reminder>`.
    ///
    /// Explicitly NOT the project's `CLAUDE.md` / `AGENTS.md` / rules. Those
    /// travel one path only — `prompt_build.rs` → `load_project_instructions`
    /// → `ExtraFilesLayer`, where they are sanitized and charged to the prompt
    /// budget. Pushing them here too is what the 2026-07-26 fix removed
    /// (`run_loop/inner.rs` says so at the producer); this doc kept advertising
    /// the removed copy. Forwarded by `Orchestrator::dispatch` into
    /// `HarnessRunner::run`, which merges it into the transient trailing recall
    /// message (`HarnessDeps::recall_context`). It is delivered to the model
    /// each Think but NEVER persisted, so the stored user message — and the
    /// session title derived from it — stays equal to the raw user input.
    /// `None` keeps the pre-feature prompt byte-identical.
    pub transient_context: Option<String>,
    /// Reasoning depth for this run, DECLARED by the user (composer pill /
    /// `chat.send` `thinking` param) or by the model (`self_config`) — never
    /// inferred from the message (R7). Forwarded into `HarnessRunner::run`,
    /// which puts it on `HarnessDeps` so every `RequestPayload` carries it.
    /// `None` = send no thinking directive, leaving each provider on its own
    /// default (the behaviour of every release before this field existed).
    pub think_level: Option<crate::agents::thinking::ThinkLevel>,
    /// This turn's resolved operating envelope — approval tier, usage mode, and
    /// the effective working directory — all already resolved by the gateway with
    /// request/session/global precedence (and, for the tier, the channel clamp).
    /// Forwarded into `HarnessRunner::run`, which threads it into the
    /// `ResolvedContext` that `SecurityLayer` and `RuntimeContextLayer` render.
    /// [`TurnEnvelope::none()`] for internal / subagent dispatch that resolves
    /// none of the three — their prompt stays byte-identical. See
    /// [`TurnEnvelope`] for the per-field contract.
    pub envelope: crate::thinker::TurnEnvelope,
    /// This turn's explicit (provider, model) pick — the chat-window model
    /// picker's `chat.send.model_override`, or the `[voice]` low-TTFT pin on a
    /// voice turn. Highest-priority model directive: it outranks the session's
    /// sticky `select_model` pick and the agent's own `model_hint`, because it
    /// is the most recent deliberate choice.
    ///
    /// It has to travel as a request field. The binder resolves its directive
    /// from `session_model_handle` (written only by the `select_model` tool)
    /// and the agent registry — neither of which the picker touches — so before
    /// this field existed the pick reached exactly two places: the vision
    /// capability check, and the `ModelResolved` event that *told the user the
    /// switch had happened*. The turn itself was served by the agent's
    /// configured model. `None` = no per-turn pick (every non-gateway
    /// dispatcher), leaving the directive chain byte-identical.
    pub model_directive: Option<crate::providers::session_model_handle::SessionModelPref>,
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
    pub default_routing: Arc<HashMap<AgentId, FlowId>>,
    pub session_service: Arc<dyn crate::session::service::SessionService>,
    pub sandbox_factory: SandboxFactory,
    /// Harness runner injected at construction — test mocks can swap this out.
    pub harness: Arc<dyn HarnessRunner>,
    /// Phase 3 — provider routing for spawned subagents (the global failover
    /// chain + per-`provider_hint` overrides). `None` for test mocks and the
    /// simple engine; `Some` once `build_failover_chain` has run at boot.
    pub subagent_routing: Option<crate::orchestrator::deps_builder::ProviderChain>,
    /// Shared `AgentRegistry`. Same `Arc` cloned into `AgentHarnessRunner` at
    /// boot so all subagent dispatchers see user-defined agents. `None` only
    /// in test fixtures and the simple engine — the gateway path falls back
    /// to `AgentRegistry::with_builtins()` in that case.
    pub agent_registry: Option<Arc<crate::agents::AgentRegistry>>,
    active_sessions: Arc<Mutex<HashSet<String>>>,
}

/// Extract a human-readable message from the opaque `Box<dyn Any + Send>`
/// panic payload returned by [`std::panic::catch_unwind`] / `FutureExt::catch_unwind`.
///
/// `catch_unwind` deliberately erases the panic type so panics from
/// different call sites stay distinct; the trade-off is that the caller
/// has to downcast speculatively. The two common shapes are `&'static str`
/// (the most common — `panic!("foo")`) and `String` (`panic!("{}", x)`),
/// plus the synthetic `"Box<dyn Any>"` payload from `std::panic::panic_any`.
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

/// Removes `key` from `active` on drop — runs even if the spawned
/// harness task panics.
struct SessionLockGuard {
    active: Arc<Mutex<HashSet<String>>>,
    key: String,
}

impl Drop for SessionLockGuard {
    fn drop(&mut self) {
        // Use `try_lock` so the `Drop` impl never blocks during stack
        // unwinding. The previous form called `self.active.lock()` directly;
        // if a prior panic held the mutex and another thread was parked on
        // it, the drop impl would block forever and deadlock the runtime.
        //
        // `try_lock` here is best-effort: losing the race means we leave
        // the entry in `active_sessions` until the next dispatch sees the
        // conflict and the operator clears it. Surfacing the loss as a
        // warning gives operators a signal; the alternative — recovering
        // from a poisoned mutex — was the source of the original review
        // finding because it silently reused a connection state that
        // might have been mid-statement.
        match self.active.try_lock() {
            Ok(mut guard) => {
                guard.remove(&self.key);
            }
            Err(_) => {
                tracing::warn!(
                    key = %self.key,
                    "SessionLockGuard: active_sessions lock contended during drop; \
                     entry will be retained until the next successful dispatch"
                );
            }
        }
    }
}

#[async_trait::async_trait]
#[allow(clippy::too_many_arguments)] // trait shape driven by Orchestrator::dispatch wiring (Task 2)
pub trait HarnessRunner: Send + Sync {
    /// Drive one flow execution. The trailing `workspace_override` is the
    /// per-run project workspace from the desktop "Enter Project" picker;
    /// `None` keeps the legacy agent-workspace path. Implementations MUST
    /// persist it on the `RunStarted` marker so [`crate::gateway::resume_coordinator`]
    /// can land a re-trigger back in the same project folder.
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
        interaction_manifest: Option<crate::thinker::interaction::InteractionManifest>,
        workspace_override: Option<std::path::PathBuf>,
        // D2: highest-priority cap. `Some(n>0)` overrides both the per-flow
        // preset and the boot-time default; `None` or `Some(0)` is "unset"
        // and falls through to the legacy resolution chain.
        max_iterations_override: Option<u32>,
        // Ephemeral per-turn prompt context (working-directory reminder,
        // project skill list, hook additions) merged into the transient
        // trailing recall message and never persisted. Project instruction
        // files are NOT in here — see `FlowRequest::transient_context`.
        // `None` = no reminder this turn.
        transient_context: Option<String>,
        // Declared reasoning depth for this run. `None` = omit the thinking
        // directive entirely and leave the provider on its own default.
        think_level: Option<crate::agents::thinking::ThinkLevel>,
        // This turn's resolved operating envelope: approval tier (`Approval
        // mode:` line), usage mode (`Usage mode:` line), and the effective
        // working directory that anchors the runtime `cwd=` / `repo=` / `git=`
        // segments. `TurnEnvelope::none()` leaves all three lines absent.
        envelope: crate::thinker::TurnEnvelope,
        // This turn's explicit model pick (chat-window picker / voice pin).
        // Outranks the session's sticky `select_model` pick and the agent's
        // `model_hint`; `None` leaves the directive chain untouched.
        model_directive: Option<crate::providers::session_model_handle::SessionModelPref>,
    ) -> Result<FlowOutcome, FlowError>;

    /// The guardrail registry this runner installs on its own harness, if
    /// any. The gateway threads it into `SubagentTool` so spawned subagents
    /// enforce the same Input/Output/ToolCall checks as the main harness
    /// (Stage 5a inheritance — previously the spawner's `guardrails` field
    /// existed but no production caller ever populated it). Default `None`
    /// keeps test mocks and the simple engine unguarded, matching their
    /// main-harness behavior.
    fn guardrails(&self) -> Option<Arc<crate::guardrails::GuardrailRegistry>> {
        None
    }

    /// VESR v1.1 (b) — the routing-experience store this runner records into,
    /// if any. The gateway threads it into `SubagentTool` so spawned subagents
    /// capture their own routing experience under their agent_id. Default `None`
    /// keeps test mocks and the simple engine capture-free.
    fn routing_store(&self) -> Option<Arc<crate::routing::RoutingExperienceStore>> {
        None
    }

    /// Stage A (P1) — the `[stability]` stall-watchdog config this runner
    /// installs on its own harness. The gateway threads it into `SubagentTool`
    /// so spawned subagents get the same runaway protection as the main run
    /// (previously the spawner's `stall_config` field existed but no
    /// production caller ever populated it — the documented "inherited by
    /// subagents" contract was silently broken). Default `None` keeps test
    /// mocks and the simple engine watchdog-free.
    fn stall_config(&self) -> Option<crate::harness::deps::StallConfig> {
        None
    }

    /// Stage A (P1) — the `[stability]` consecutive-failure cap this runner
    /// installs. Threaded into `SubagentTool` so subagents bound the same
    /// failure streak the main run does. Default `None` = uncapped.
    fn consecutive_failure_cap(&self) -> Option<usize> {
        None
    }

    /// Stage A (P1) — the `[stability]` per-turn wall-clock timeout this runner
    /// installs. Threaded into `SubagentTool` so a hung subagent turn aborts
    /// on the same deadline the main run enforces. Default `None` = no timeout.
    fn turn_timeout(&self) -> Option<std::time::Duration> {
        None
    }

    /// B15 — the boot-time `[execution] max_iterations` this runner caps its own
    /// Think→Act loop with. Threaded into `SubagentTool` so a child role that
    /// declares no `max_iterations` inherits a cap instead of looping until the
    /// spawn timeout kills it (an iteration cap yields a usable summary via the
    /// boundary grace turn; a timeout throws the whole run away). Default `None`
    /// leaves the spawner on `FALLBACK_MAX_ITERATIONS` — still capped, just not
    /// the operator's configured value.
    fn default_max_iterations(&self) -> Option<usize> {
        None
    }

    /// The `[tool_service] parallel_tool_concurrency` cap this runner's own
    /// Act phase dispatches with. Threaded into `SubagentTool` so spawned
    /// subagents honour the operator's configured value (0/1 = disabled)
    /// instead of the hardcoded default the spawner previously pinned —
    /// an operator who disabled parallel dispatch still had subagents
    /// running batches 8-wide. Default `None` leaves the spawner on the
    /// config default, matching test mocks / the simple engine.
    fn parallel_tool_concurrency(&self) -> Option<usize> {
        None
    }

    /// The `[context_budget]` config this runner builds its own per-run
    /// budget / compactor / preflight pipeline from. Threaded into
    /// `SubagentTool` so a spawned subagent gets the same context management
    /// instead of running with none: the spawner pinned all three to `None`,
    /// so a child's prompt grew unbounded (every turn replays the full child
    /// log) and a `prompt_too_long` had no compactor to rescue with — the
    /// reactive drain went straight to `ReactiveCompactExhausted` and the
    /// whole subagent run died. Default `None` (test mocks / the simple
    /// engine / `[context_budget]` disabled) keeps the previous behaviour.
    ///
    /// The *config* travels, not the budget instance: each run needs its own
    /// `ContextBudget` so calibration and circuit-breaker counters never leak
    /// between the parent and its children.
    fn context_budget_config(&self) -> Option<crate::context::budget::ContextBudgetConfig> {
        None
    }

    /// The per-run budget refiner this runner applies to its OWN prompt /
    /// history budgets (`runner_impl` re-keys the chain-minimum config onto
    /// the serving model every run).
    ///
    /// Threaded into `SubagentTool` so a spawned child's **prompt** budget is
    /// re-keyed onto the model the child will actually run on: the spawner
    /// previously derived it straight from the chain-minimum `token_budget`,
    /// so a child pinned to a narrow model inherited the wider chain budget —
    /// its system prompt could grow past what its own window + the token hard
    /// gate would have allowed on the main loop. Default `None` (mocks /
    /// simple engine / `[context_budget]` disabled) keeps the unrefined
    /// chain-minimum behaviour.
    fn context_budget_refiner(
        &self,
    ) -> Option<crate::orchestrator::deps_builder::ContextBudgetRefiner> {
        None
    }

    /// The runner's configured per-provider context-window override, handed
    /// to [`Self::context_budget_refiner`]'s refinement exactly as the main
    /// loop hands it to its own (same argument, same precedence over the
    /// model catalog). Default `None` = catalog resolution only.
    fn primary_context_window(&self) -> Option<u32> {
        None
    }

    /// The cheap-tier summarization provider this runner routes its OWN
    /// compaction side-channel to (`[context_budget] summary_model`, or the
    /// primary preset's declared `default_aux_model`).
    ///
    /// Threaded into `SubagentTool` for the same reason `context_budget_config`
    /// is: the child builds its own compactor, and that second construction
    /// site inherited none of the parent's tiering — so every subagent
    /// compaction billed the operator's **main reasoning model** for
    /// read-and-condense work the root agent had already routed to a flash
    /// sibling. Silent, and worst exactly where it is worst: a swarm fan-out
    /// compacts once per child.
    ///
    /// Unlike the budget, the *provider instance* travels rather than a config:
    /// it is a stateless `Arc` handle and rebuilding it per child would clone
    /// the whole provider config N times for a byte-identical result. Default
    /// `None` (mocks / simple engine / no cheap tier resolved) leaves the child
    /// summarizing on the main LLM, exactly as before.
    fn cheap_summary_provider(&self) -> Option<Arc<dyn crate::providers::AiProvider>> {
        None
    }

    /// Hand the spawner the SAME verifier chain this runner installs on its
    /// own harness. Threaded into `SubagentTool` so a subagent that enters
    /// a tool-call death loop is caught by the same structural watchdog
    /// (ToolLoopVerifier, StopHookVerifier, ScratchpadGoalVerifier, …) the
    /// parent enforces on itself. The SpawnerBase / AgentRuntime side has
    /// always carried the field — only the SubagentTool construction site
    /// was missing, so every spawned subagent silently ran with
    /// `verifier_chain: None` (AGENTS-R4-01). Default `None` keeps test
    /// mocks / the simple engine on the no-verifier path, matching the
    /// pre-2026-09 behaviour.
    fn verifier_chain(&self) -> Option<Arc<crate::verification::VerifierChain>> {
        None
    }

    /// Estimate the context-window occupancy of this session's *next* prompt,
    /// for sessions that never ran an LLM turn (no persisted real occupancy).
    /// Deterministic token counting only — no LLM call (R7). Default `None`
    /// keeps test mocks / the simple engine gauge-less.
    async fn estimate_context(
        &self,
        _session_key: &str,
    ) -> Option<crate::orchestrator::harness_bridge::context_estimate::ContextEstimate> {
        None
    }
}

/// Clone the default-agent spec, overriding `agent` with the requested id so
/// the harness loads *that* agent's identity from disk. When the base already
/// targets the requested agent (e.g. `main` itself), return it unchanged.
fn fallback_spec_with_agent(base: Arc<FlowSpec>, agent_id: &str) -> Arc<FlowSpec> {
    if base.agent == agent_id {
        base
    } else {
        // rust-doctor-disable-next-line excessive-clone
        let mut s = (*base).clone();
        // rust-doctor-disable-next-line excessive-clone
        s.agent = agent_id.to_string();
        Arc::new(s)
    }
}

impl Orchestrator {
    pub fn new(
        flow_registry: Arc<FlowRegistry>,
        default_routing: Arc<HashMap<AgentId, FlowId>>,
        session_service: Arc<dyn crate::session::service::SessionService>,
        sandbox_factory: SandboxFactory,
        harness: Arc<dyn HarnessRunner>,
    ) -> Self {
        Self {
            flow_registry,
            default_routing,
            session_service,
            sandbox_factory,
            harness,
            subagent_routing: None,
            agent_registry: None,
            active_sessions: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Phase 3 — attach the subagent provider routing built by
    /// `build_failover_chain`. The gateway execution engine reads it to route
    /// spawned subagents through the failover chain and to honour
    /// `AgentDef.provider_hint`.
    #[must_use]
    pub fn with_subagent_routing(
        mut self,
        routing: crate::orchestrator::deps_builder::ProviderChain,
    ) -> Self {
        self.subagent_routing = Some(routing);
        self
    }

    /// Attach the shared `AgentRegistry` so `SubagentTool` instances spawned
    /// inside the gateway execution engine can resolve user-defined agents
    /// (not just built-ins). Without this, `run_loop` falls back to a fresh
    /// empty registry and subagents silently lose all custom-agent metadata.
    pub fn with_agent_registry(mut self, registry: Arc<crate::agents::AgentRegistry>) -> Self {
        self.agent_registry = Some(registry);
        self
    }

    /// Step 1 of dispatch, extracted so it can be exercised without spawning a
    /// harness: resolve `(flow_id | agent_id, channel)` to the `FlowSpec` that
    /// will actually serve the request, fallback included.
    ///
    /// The fallback is the load-bearing half. Callers assert things like "every
    /// registered agent reaches a flow that honours `session_hint`", and a test
    /// that re-derived the fallback itself would be asserting against its own
    /// copy — the failure mode CLAUDE.md files under "the second answer is the
    /// defect". This is the one derivation; `dispatch` and the guards share it.
    pub fn resolve_spec(&self, req: &FlowRequest) -> Result<Arc<FlowSpec>, FlowError> {
        // An explicit flow_id wins; otherwise route by agent_id. An agent absent
        // from the routing table is NOT an error: the gateway already resolved its
        // AgentInstance before dispatch, so the orchestrator trusts that and routes
        // the run through the canonical default-agent flow — overriding `spec.agent`
        // with the requested id so the harness loads *that* agent's identity from
        // `~/.aleph/agents/<id>/` (the default-agent preset hardcodes `agent = "main"`).
        // This is what lets every registered agent execute — config `[[agents.list]]`
        // and team-created ones live only in the gateway registry, not the
        // orchestrator's builtins.
        // The same fallback applies when routing *succeeds* but the resolved flow
        // id is unknown (a routing table entry whose flow was never registered):
        // prefer serving the request through the default agent over a hard error.
        match &req.flow_id {
            Some(id) => self
                .flow_registry
                .resolve(id)
                .ok_or_else(|| FlowError::UnknownFlow(id.clone())),
            None => match resolve_flow_id(&req.agent_id, &self.default_routing) {
                Ok(flow_id) => match self.flow_registry.resolve(&flow_id) {
                    Some(spec) => Ok(spec),
                    None => {
                        // Routing succeeded but the resolved flow id is unknown
                        // (a routing-table entry whose flow was never
                        // registered). Prefer serving through the default agent
                        // over a hard error; if the default is missing too,
                        // surface the original UnknownFlow so the caller sees
                        // what actually went wrong.
                        self.flow_registry
                            .resolve(DEFAULT_AGENT_FLOW_ID)
                            .map(|base| fallback_spec_with_agent(base, &req.agent_id))
                            .ok_or_else(|| FlowError::UnknownFlow(flow_id.clone()))
                    }
                },
                Err(FlowError::UnknownAgent(_)) => self
                    .flow_registry
                    .resolve(DEFAULT_AGENT_FLOW_ID)
                    .map(|base| fallback_spec_with_agent(base, &req.agent_id))
                    .ok_or_else(|| FlowError::UnknownFlow(DEFAULT_AGENT_FLOW_ID.to_string())),
                Err(e) => Err(e),
            },
        }
    }

    /// Seven-step dispatch. See design §6.
    pub async fn dispatch(&self, req: FlowRequest) -> Result<FlowHandle, FlowError> {
        // Step 1: resolve flow_id → FlowSpec (see `resolve_spec`).
        let spec = self.resolve_spec(&req)?;

        // Step 2: depth guard.
        depth_guard(req.depth)?;

        // Step 3: agent lookup deferred to harness (it holds the AgentRegistry).

        // Step 4: session resolve + per-session lock.
        let session_input = SessionResolveInput {
            // rust-doctor-disable-next-line excessive-clone
            strategy: spec.session_strategy.clone(),
            // rust-doctor-disable-next-line excessive-clone
            session_hint: req.session_hint.clone(),
            // rust-doctor-disable-next-line excessive-clone
            parent_session: req.parent_session.clone(),
            fresh_key_fn: || uuid::Uuid::new_v4().to_string(),
        };
        let session_res = resolve_session(session_input)?;
        // SessionStrategy::Child resolved a parent key here. The session
        // store row is created later by the gateway (`SessionStore::get_or_create`)
        // from the key alone — `parent_session_key` is exposed on `FlowHandle`
        // so the gateway can persist it onto the store row. Without this,
        // child sessions become root sessions in the store with no recovery
        // path, breaking data lineage end-to-end.
        //
        // Warn (not debug) when a parent key is dropped at the dispatch
        // boundary: the gateway is the only place that can fix it, and a
        // silent drop has cost operators real debugging time.
        if let Some(parent) = &session_res.parent_session_key {
            tracing::warn!(
                session_key = %session_res.session_key,
                parent_session_key = %parent,
                "dispatch: child session resolved; gateway MUST thread parent_session_key \
                 onto the SessionStore row or the lineage is silently lost"
            );
        }
        {
            let mut guard = self
                .active_sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            // rust-doctor-disable-next-line excessive-clone
            if !guard.insert(session_res.session_key.clone()) {
                return Err(FlowError::SessionConflict(session_res.session_key));
            }
        }

        // Step 5: brain pick deferred to HarnessRunner (ProviderRegistry lives there).

        // Step 6: sandbox provision. Per-request `sandbox_override` short-circuits
        // the factory — the team dispatcher uses it to inject a `WorktreeSandbox`
        // for per-task git isolation. Falls back to the factory when None.
        // rust-doctor-disable-next-line excessive-clone
        let sandbox = match req.sandbox_override.clone() {
            Some(sb) => sb,
            None => match (self.sandbox_factory)(&session_res.session_key) {
                Ok(sb) => sb,
                Err(e) => {
                    let mut guard = self
                        .active_sessions
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    guard.remove(&session_res.session_key);
                    return Err(e);
                }
            },
        };

        // Step 7: spawn harness, plumbing events + completion + cancel.
        let (event_tx, event_rx) = broadcast::channel::<FlowStreamEvent>(CHANNEL_BUFFER_SIZE);
        let (done_tx, done_rx) = oneshot::channel();
        let cancel = CancellationToken::new();

        // rust-doctor-disable-next-line excessive-clone
        let harness = self.harness.clone();
        // rust-doctor-disable-next-line excessive-clone
        let spec_clone = spec.clone();
        // rust-doctor-disable-next-line excessive-clone
        let input_clone = req.input.clone();
        // rust-doctor-disable-next-line excessive-clone
        let sandbox_clone = sandbox.clone();
        // rust-doctor-disable-next-line excessive-clone
        let cancel_clone = cancel.clone();
        // rust-doctor-disable-next-line excessive-clone
        let session_key = session_res.session_key.clone();
        // rust-doctor-disable-next-line excessive-clone
        let active = self.active_sessions.clone();
        // rust-doctor-disable-next-line excessive-clone
        let session_for_release = session_res.session_key.clone();
        // rust-doctor-disable-next-line excessive-clone
        let tool_service_override = req.tool_service.clone();
        // rust-doctor-disable-next-line excessive-clone
        let trace_sink = req.trace_sink.clone();
        // rust-doctor-disable-next-line excessive-clone
        let interaction_manifest = req.interaction_manifest.clone();
        // rust-doctor-disable-next-line excessive-clone
        let workspace_override = req.workspace_override.clone();
        let max_iterations_override = req.max_iterations_override;
        // rust-doctor-disable-next-line excessive-clone
        let transient_context = req.transient_context.clone();
        let think_level = req.think_level;
        // rust-doctor-disable-next-line excessive-clone
        let envelope = req.envelope.clone();
        // rust-doctor-disable-next-line excessive-clone
        let model_directive = req.model_directive.clone();
        // P1 data isolation — captured before the spawn boundary (task-locals
        // do not cross `tokio::spawn`); re-derived into a `ScopeAttribution`
        // and re-seeded inside, sibling of `with_agent_id`/`with_project_root`.
        // rust-doctor-disable-next-line excessive-clone
        let (owner_user_id, scope_id) = req.scope.clone().into_parts();
        // P2 speaker label — the SAME boundary and the same reason as the two
        // above, but this one has no `FlowRequest` field to be re-derived from,
        // so the live task-local is captured here on the caller's side. It is
        // live: `dispatch` is awaited inside `run_loop::with_request_scope`,
        // which seeds it from the request's `AUTHOR_USER_KEY`.
        //
        // Dropping it does not fail loudly. `ambient_room_author` degrades to
        // the scope's `owner_user_id`, which in a room is the ROOM's owner —
        // so every member's message would be labelled with whoever created the
        // session, on a path with no error anywhere.
        let room_author = crate::scope::current_room_author();
        // Fourth capture at the same boundary, for the same reason, and it was
        // the one this list did not know about. `TURN_ORIGINATOR` is scoped
        // ONCE per run tree (`run_agent_loop`), so unlike `TURN_CONTEXT` — which
        // `ScopedToolService::execute` re-scopes at the tool chokepoint and
        // therefore survives — it dies here and nothing downstream restores it.
        //
        // Measured, not reasoned: a real-machine run of `qa/teamchat_rooms`
        // logged `originator=Some(u-…)` inside `run_agent_loop` and
        // `orig=None` inside `OperatorApprovalRequester`, one spawn apart. The
        // consequence is that `ExecApprovalRecord::originator_user_id` is
        // `None` on EVERY card raised from inside a run, which silently
        // disarms both of its consumers: the channel button-callback gate
        // (`ManagerCallbackSink::handle_callback`, "only the human who asked
        // may press the button") and the room narrowing in
        // `approval_addressable_by_caller` (a team member's parked call is then
        // routed by session ownership alone — in a room, the CREATOR).
        //
        // Deliberately captured here rather than added to
        // `CarriedAttribution`: this site does not use that carrier (it
        // re-derives the scope from `FlowRequest`'s explicit fields on
        // purpose), and the four sites that DO use it re-enter through
        // `run_loop`, which re-seeds the originator from request metadata. A
        // sixth carrier field would have zero consumers (R10).
        let originator = crate::tools::turn_context::current_originator();

        tokio::spawn(async move {
            let _lock = SessionLockGuard {
                active,
                key: session_for_release,
            };
            // Re-establish the project-root task-local inside the spawned
            // harness task. `tokio::spawn` does NOT inherit task-locals, so the
            // `with_project_root` scope set on the gateway side (run_loop) is
            // lost across this boundary. The harness receives `workspace_override`
            // explicitly for CWD + prompt context, but the per-project memory
            // seams (`note_manage`, the retrieval assembler, session compaction)
            // read the ambient `current_project_root()` — without this they see
            // `None` and `memory.project_scoped` silently no-ops to the base
            // namespace, leaking notes across projects.
            // Re-establish the active-agent task-local too (same `tokio::spawn`
            // task-local loss as the project root): agent-scoped skill discovery
            // (`~/.aleph/agents/<id>/skills`) reads `current_agent_id()`. Capture
            // the id before `spec_clone` is moved into `harness.run`.
            // rust-doctor-disable-next-line excessive-clone
            let agent_id_for_scope = spec_clone.agent.clone();
            // P1 data isolation: re-derive the `ScopeAttribution` from the
            // captured owner/scope strings and re-seed it as a task-local
            // inside the spawn — mirrors `run_loop`'s `with_request_scope`,
            // but via `FlowRequest`'s two explicit fields since it has no
            // metadata map to carry `scope::stamp_metadata`'s keys.
            let scope_attr = {
                let mut m = std::collections::HashMap::new();
                if let Some(owner) = owner_user_id {
                    m.insert(crate::scope::OWNER_META_KEY.to_string(), owner);
                }
                if let Some(scope) = scope_id {
                    m.insert(crate::scope::SCOPE_META_KEY.to_string(), scope);
                }
                crate::scope::scope_from_metadata(&m)
            };

            // Catch panics from the entire harness task-local stack so they
            // surface on the `done_tx` channel as `FlowError::Internal`
            // instead of being swallowed by tokio's default panic hook. The
            // previous implementation discarded the `JoinHandle` outright:
            // a panic in any of `with_agent_id` / `with_scope` /
            // `with_room_author` / `with_originator` would have left the
            // caller's `FlowHandle::completion` waiting forever. `AssertUnwindSafe`
            // is sound here because every captured value is `Send + 'static`
            // and the panic itself cannot violate invariants across the
            // boundary — it just aborts this future and we convert it to an
            // error below.
            let outcome = std::panic::AssertUnwindSafe(crate::agents::with_agent_id(
                Some(agent_id_for_scope),
                crate::projects::with_project_root(
                    // rust-doctor-disable-next-line excessive-clone
                    workspace_override.clone(),
                    crate::scope::with_scope(
                        scope_attr,
                        crate::scope::with_room_author(
                            room_author,
                            crate::tools::turn_context::with_originator(
                                originator,
                                harness.run(
                                    session_key,
                                    spec_clone,
                                    input_clone,
                                    sandbox_clone,
                                    event_tx,
                                    cancel_clone,
                                    tool_service_override,
                                    trace_sink,
                                    interaction_manifest,
                                    workspace_override,
                                    max_iterations_override,
                                    transient_context,
                                    think_level,
                                    envelope,
                                    model_directive,
                                ),
                            ),
                        ),
                    ),
                ),
            ))
            .catch_unwind()
            .await;

            let outcome = match outcome {
                Ok(result) => result,
                Err(panic_payload) => {
                    let msg = panic_message(&panic_payload);
                    // `session_key` was moved into `harness.run(...)` above;
                    // we cannot reference it again after that move. The
                    // structured context for this log is already captured by
                    // the tracing span on the spawned task, so the message
                    // alone is sufficient.
                    tracing::error!(
                        "harness task panicked; surfacing as FlowError::Internal: {msg}"
                    );
                    Err(FlowError::Internal(format!(
                        "harness task panicked: {msg}"
                    )))
                }
            };

            // `done_tx.send` returns Err only when `done_rx` was dropped by the
            // caller — meaning the handle was abandoned and no one is waiting for
            // the result. Dropping the result is intentional; a `return` or `?`
            // here would propagate an error that signals nothing actionable.
            let _ = done_tx.send(outcome);
            // _lock drops here, releasing the session key regardless of panic.
        });

        Ok(FlowHandle {
            session_key: session_res.session_key.clone(),
            parent_session_key: session_res.parent_session_key,
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

    /// The harness spawn must re-establish `TURN_ORIGINATOR`.
    ///
    /// Source-level, because the thing being asserted is unobservable at
    /// runtime from inside one process: a lost task-local reads exactly like a
    /// run that never had one (both are `None`), which is why this went four
    /// rounds undetected and only a two-process real-machine run
    /// (`qa/teamchat_rooms`) could see it — `run_agent_loop` logged
    /// `Some(u-…)`, `OperatorApprovalRequester` logged `None`, one
    /// `tokio::spawn` apart.
    ///
    /// The requirement is derived, not asserted: the first half proves the run
    /// loop really does scope this task-local once per run tree (if that ever
    /// stops being true, this guard says so instead of silently outliving its
    /// subject), and only then is the spawn required to carry it.
    ///
    /// **Scope, stated so it is not mistaken for more:** this checks ONE
    /// task-local. It deliberately does not derive the full set of `with_*`
    /// combinators wrapping `run_agent_loop_inner` — several of those
    /// (`with_fs_scope`, `with_exec_workspace`, `with_pending_media`) reach the
    /// harness through explicit arguments instead, and an auto-derived list
    /// would need an exemption entry per case, i.e. exactly the name list this
    /// shape is supposed to replace. Whether each of those is genuinely carried
    /// is a separate question this guard does not answer.
    #[test]
    fn the_harness_spawn_reestablishes_the_run_tree_originator() {
        // Comments are documentation, code is the subject — without this the
        // prose above would satisfy the assertion below.
        let code_of = |src: &str| {
            src.replace('\r', "")
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        // `run_loop/mod.rs` declares its tests as a SEPARATE FILE
        // (`#[cfg(test)] mod tests;`, at the top), so the usual
        // split-on-`#[cfg(test)]` idiom would cut the production body away
        // entirely and leave seven lines of `use`. It cost this guard one red
        // run to notice; there is nothing to strip here, the whole file is
        // production.
        let run_loop = code_of(include_str!("../gateway/execution_engine/run_loop/mod.rs"));
        assert!(
            run_loop.contains("with_originator("),
            "run_agent_loop no longer scopes TURN_ORIGINATOR; this guard is \
             now describing a requirement that does not exist — delete it or \
             follow the task-local to its new home"
        );

        // This file DOES declare an inline test module, and that module holds
        // the very literal being searched for, so the prefix split is
        // load-bearing here. Routed through `production_prefix` (the same
        // CRLF-safe cut the rest of the repo uses — a hand-rolled
        // `split("#[cfg(test)]")` here is exactly what
        // `utils::source_scan`'s "no hand-rolled cut" guard says no to, and
        // a previous unanchored `"\n#[cfg(test)]\n"` form matched nothing on
        // Windows checkouts and let this test silently pass on its own
        // literal).
        let me = code_of(include_str!("dispatch.rs"));
        let me = crate::utils::source_scan::production_prefix(&me);
        let spawn_at = me
            .find("tokio::spawn(async move {")
            .expect("the harness spawn is the subject of this guard");
        let spawn_block = &me[spawn_at..];
        assert!(
            spawn_block.contains("with_originator("),
            "the harness task spawn drops TURN_ORIGINATOR. Every approval \
             record raised from inside a run then carries \
             `originator_user_id: None`, which disarms BOTH of its consumers: \
             the channel button-callback gate (only the human who asked may \
             press the button) and the room narrowing in \
             `approval_addressable_by_caller`."
        );
    }

    /// Every terminate token this enum can produce has words on the terminal
    /// surfaces — and the shared table has no row for a token that cannot.
    ///
    /// The set is **derived from `as_static_str`'s own arms**, not restated
    /// here. That matters more than it looks: `as_static_str` is an exhaustive
    /// `match`, so a sixteenth variant cannot be added without adding a line
    /// there, which means the scan below cannot miss one. A hand-written list
    /// in this test could — and the class it would miss is precisely the one
    /// that already happened: `diminishing_returns` had a label on **zero** of
    /// the five surfaces that render this field, and nothing said so.
    ///
    /// The equality runs in both directions on purpose. Left-only is a token
    /// that renders on screen as a raw wire string (`hit_max_iterations` at a
    /// person, which `aleph watch` really did print). Right-only is words for a
    /// token that no longer exists — dead weight that also has to be carried
    /// through `interfaces/webchat`'s locale files, since the panel's guard
    /// walks the same table.
    ///
    /// Not in scope, said plainly: this checks that a token is *labelled*, not
    /// that any particular surface renders it. `gateway::i18n::render_loop_halt`
    /// keeps a private table and is not measured here — it is keyed on this
    /// enum rather than on the token, and its strings are paragraphs of
    /// per-cause advice rather than a name for the halt.
    ///
    /// `reply_emitter::cap_notice_for` used to be named here too. It is not any
    /// more: the channel one-liner renders from `aleph_protocol::terminate`
    /// like the other four surfaces, so this guard does now reach it.
    #[test]
    fn every_terminate_token_has_words_on_the_terminal_surfaces() {
        let src = include_str!("dispatch.rs").replace('\r', "");
        let src = crate::utils::source_scan::production_prefix(&src);
        let at = src
            .find("pub const fn as_static_str")
            .expect("as_static_str is the fact this guard derives from");

        // The window ends at the function's own closing brace, not after N
        // lines: a fixed window is a boundary someone else's edit can move,
        // and the failure is silent in the direction that under-scans.
        let body = {
            let mut depth = 0usize;
            let mut end = None;
            for (i, c) in src[at..].char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(i);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            &src[at..at + end.expect("as_static_str's body never closes")]
        };

        let mut produced: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for (i, _) in body.match_indices("=> \"") {
            let rest = &body[i + 4..];
            let end = rest.find('"').expect("an unterminated token literal");
            produced.insert(&rest[..end]);
        }
        assert!(
            produced.len() >= 10 && produced.contains("completed"),
            "the scan found {} tokens, which means it stopped matching the \
             shape of `as_static_str` rather than that the enum shrank",
            produced.len(),
        );

        let labelled: std::collections::BTreeSet<&str> =
            aleph_protocol::terminate::labelled_tokens().collect();
        assert_eq!(
            produced, labelled,
            "left = tokens `TerminateReason::as_static_str` can produce, \
             right = tokens `aleph_protocol::terminate` has words for. A \
             left-only token renders on screen as its raw wire string; a \
             right-only one is dead words in this crate AND in \
             interfaces/webchat/locales/{{en,zh}}.json.",
        );
    }

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
        assert_eq!(
            TerminateReason::BudgetExhaustedPartialResult {
                reason: "hit_max_iterations".into(),
                partial_summary: "x".into(),
            }
            .as_static_str(),
            BUDGET_PARTIAL_RESULT_LABEL,
        );
    }

    #[test]
    fn budget_partial_result_is_hit_limit() {
        assert!(TerminateReason::BudgetExhaustedPartialResult {
            reason: "hit_max_iterations".into(),
            partial_summary: "did some work".into(),
        }
        .is_hit_limit());
    }

    #[test]
    fn escalate_partial_result_upgrades_max_iterations_when_partial_present() {
        let raw = TerminateReason::HitMaxIterations { used: 40 };
        let escalated = escalate_partial_result(raw, Some("I gathered three sources."));
        match escalated {
            TerminateReason::BudgetExhaustedPartialResult {
                reason,
                partial_summary,
            } => {
                assert_eq!(reason, "hit_max_iterations");
                assert_eq!(partial_summary, "I gathered three sources.");
            }
            other => panic!("expected PartialResult, got {other:?}"),
        }
    }

    #[test]
    fn escalate_partial_result_upgrades_context_budget_and_max_output_tokens() {
        for raw in [
            TerminateReason::ContextBudgetExhausted,
            TerminateReason::MaxOutputTokensExhausted,
        ] {
            let label = raw.as_static_str().to_string();
            let escalated = escalate_partial_result(raw, Some("partial text"));
            match escalated {
                TerminateReason::BudgetExhaustedPartialResult { reason, .. } => {
                    assert_eq!(reason, label);
                }
                other => panic!("expected PartialResult, got {other:?}"),
            }
        }
    }

    #[test]
    fn escalate_partial_result_passes_through_when_no_partial_text() {
        let raw = TerminateReason::HitMaxIterations { used: 40 };
        assert_eq!(
            escalate_partial_result(raw, None),
            TerminateReason::HitMaxIterations { used: 40 },
        );
        let raw = TerminateReason::HitMaxIterations { used: 40 };
        assert_eq!(
            escalate_partial_result(raw, Some("   \n\t  ")),
            TerminateReason::HitMaxIterations { used: 40 },
            "whitespace-only partial is treated as no partial",
        );
    }

    #[test]
    fn escalate_partial_result_passes_through_non_budget_reasons() {
        // Cancelled / StallTimeout / VerifierVeto / Completed must NEVER
        // be re-classified as PartialResult — they are not budget caps.
        let cases = [
            TerminateReason::Completed,
            TerminateReason::Cancelled,
            TerminateReason::StallTimeout { elapsed_ms: 30_000 },
            TerminateReason::VerifierVeto { vetos: 3 },
            TerminateReason::ConsecutiveFailureCap { consecutive: 5 },
            TerminateReason::EmptyResponseExhausted,
            TerminateReason::StopHookHalt {
                reason: "policy".into(),
            },
            TerminateReason::TurnTimeout {
                phase: "act".into(),
                elapsed_ms: 60_000,
            },
        ];
        for r in cases {
            let label = r.as_static_str();
            let escalated = escalate_partial_result(r.clone(), Some("nontrivial partial"));
            assert_eq!(escalated, r, "{label} must pass through unchanged",);
        }
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
