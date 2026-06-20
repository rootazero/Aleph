//! Think phase — single-turn LLM call with guardrails, budget checks, and verifier dispatch.

use std::sync::atomic::Ordering;

use tokio_util::sync::CancellationToken;

use super::{AgentHarness, InputGuardrailOutcome};
use crate::context::budget::LoopDirective;
use crate::harness::callback::HarnessCallback;
use crate::harness::trait_def::{HarnessError, TurnState, TurnStep};
use crate::providers::adapter::{NativeToolCall, ProviderResponse, RequestPayload, StopReason};
use crate::providers::message::UnifiedMessage;
use crate::session::events::{MessageContent, SessionEvent, SessionEventRecord, TurnId};
use crate::session::service::SessionId;
use crate::verification::{
    hash_tool_args, ToolCallSummary, TurnVerifyContext, VerifierVerdict, TOOL_HISTORY_WINDOW,
};

/// Bridges a borrowed [`HarnessCallback`] into the provider layer's
/// [`crate::providers::DeltaSink`] observer, so live token deltas reach the
/// callback as the provider streams them.
///
/// The callback method is `&mut self` while the sink method is `&self`, so the
/// borrow is held behind a `std::sync::Mutex` for interior mutability. The lock
/// is effectively uncontended: the provider drives one delta at a time on a
/// single task. Only text and thinking deltas are forwarded for live preview —
/// tool-call and bookkeeping deltas are folded by the provider's
/// `DeltaCollector` and surface through the assembled `ProviderResponse`.
struct CallbackSink<'c> {
    callback: crate::sync_primitives::Mutex<&'c mut dyn HarnessCallback>,
}

#[async_trait::async_trait]
impl crate::providers::DeltaSink for CallbackSink<'_> {
    async fn on_delta(&self, delta: &crate::providers::ProviderDelta) {
        // Poison-safe (P7): recover the guard rather than panicking — a delta
        // preview is never worth aborting the turn.
        let mut cb = self.callback.lock().unwrap_or_else(|e| e.into_inner());
        match delta {
            crate::providers::ProviderDelta::TextDelta(t) if !t.is_empty() => cb.on_delta(t),
            crate::providers::ProviderDelta::ThinkingDelta(t) if !t.is_empty() => {
                cb.on_reasoning(t)
            }
            _ => {}
        }
    }
}

/// Ephemeral nudge appended on the grace turn when the budget hits
/// critical — the single tool-less LLM call given when
/// `LoopDirective::FinalReply` fires and the prior assistant turn ended
/// on an unresolved `tool_use`. Tools are also stripped at the request
/// layer (no `.with_tools(...)`), so the model cannot loop further.
const GRACE_NUDGE_BUDGET: &str = "You are out of context budget and cannot call any more tools. \
     Respond now with a final summary for the user based on what you have so far.";

/// Ephemeral nudge for the grace turn fired by
/// `LoopDirective::StopDiminishing` — same shape as
/// `GRACE_NUDGE_BUDGET` but framed around lack of measurable progress
/// rather than budget exhaustion.
const GRACE_NUDGE_DIMINISHING: &str = "You have not been making measurable progress on this task. \
     Stop calling tools and summarize what you have found so far for the user.";

/// Ephemeral nudge for the grace turn fired when the `max_iterations`
/// cap trips — same shape as the other nudges but framed around the
/// iteration limit. Without this turn a runaway that ends on an
/// unresolved `tool_use` leaves the user with no terminal text.
const GRACE_NUDGE_MAX_ITERATIONS: &str =
    "You have reached the maximum number of tool-calling iterations and \
     cannot call any more tools. Respond now with a final summary for the \
     user based on what you have accomplished so far.";

/// Ephemeral nudge for the grace turn fired when the verifier-veto safety
/// cap trips — the model kept trying to finish with required steps still
/// incomplete. The remaining steps are already in context (the
/// `[verifier veto] …` messages list them), so this only tells the model to
/// stop and hand control back to the user. The model writes the actual
/// message (R7 — no hardcoded user-facing template).
const GRACE_NUDGE_VERIFIER_VETO: &str =
    "You have repeatedly tried to finish while required steps from your \
     execution list remain incomplete, and the safety cap has now stopped \
     the loop. Do NOT call any more tools. Respond now with a clear message \
     for the user: which steps remain unfinished, what is blocking you from \
     completing them, and what decision or input you need from the user to \
     proceed.";

/// Ephemeral nudge for the grace turn fired when the consecutive-failure
/// safety cap trips. The recurring error is already in context (the
/// `ToolError` events), so this only tells the model to stop and surface the
/// blocker to the user.
const GRACE_NUDGE_FAILURE_CAP: &str =
    "Your recent turns have failed repeatedly and the safety cap has now \
     stopped the loop. Do NOT call any more tools. Respond now with a clear \
     message for the user: what you were attempting, the specific error or \
     obstacle that keeps recurring, and what decision or input you need from \
     the user to proceed.";

/// Ephemeral nudge for the grace turn fired when the `ToolLoopVerifier` halts
/// an unproductive tool-call loop. The loop ran many tool calls without ever
/// converging on a deliverable (the original 116-step failure mode), so this
/// turns the dead halt into a salvage: use everything already gathered to
/// produce the best possible final answer instead of leaving the user with
/// only a "stop hook" apology. The model writes the actual content (R7 — no
/// hardcoded user-facing template).
const GRACE_NUDGE_TOOL_LOOP_HALT: &str =
    "The run was stopped to end an unproductive tool-call loop. Do NOT call any \
     more tools. Using everything you have ALREADY gathered, produce your best \
     final deliverable for the user now. If a specific piece of data is \
     genuinely missing, state that gap plainly and deliver the rest — do not \
     let one missing item block the whole response.";

/// Maximum re-issues of the LLM call when the provider returns a response
/// with no text, no `tool_calls` and no thinking. A small bound — an empty
/// response is usually transient; persistent emptiness is a broken
/// endpoint that more retries will not fix.
const EMPTY_RESPONSE_RETRIES: u32 = 2;

/// G1 (opencode-inspired): last-step soft warning. Injected as a synthetic
/// trailing user message wrapped in `<system-reminder>` on the LAST allowed
/// iteration so the model uses *this* turn to emit a final summary instead
/// of triggering the post-hoc C1 grace turn (which costs an extra LLM
/// round-trip). C1 remains as a fail-safe for the rare case where the
/// model ignores this hint and still emits `tool_use`.
///
/// Text intentionally mirrors opencode's `max-steps.txt` shape so model
/// behaviour transfers across harnesses.
const MAX_STEPS_HINT: &str = "<system-reminder>\n\
CRITICAL — MAXIMUM ITERATIONS REACHED\n\n\
This is the LAST iteration allowed for this task. Tools are effectively \
disabled — any tool_use you emit will be discarded after one more grace \
turn. You MUST respond with TEXT ONLY now.\n\n\
Your response should include:\n\
- A short statement that the iteration cap was reached\n\
- A summary of what was accomplished so far\n\
- Any tasks that remain incomplete\n\
- A recommendation for what should be done next\n\
</system-reminder>";

/// Maximum re-issues of the LLM call when the provider hits
/// `max_output_tokens` mid-stream. Mirrors claude-code's
/// `MAX_OUTPUT_TOKENS_RECOVERY_LIMIT` (query.ts:164). The retry appends
/// the partial assistant output and a "resume directly" nudge so the
/// model continues mid-thought rather than restarting.
const MAX_OUTPUT_TOKENS_RECOVERY_LIMIT: u32 = 3;

/// Meta user message appended on each `max_output_tokens` recovery
/// retry. Text mirrors claude-code's wording (query.ts:1226) so model
/// behaviour transfers across harnesses. The model is expected to pick
/// up mid-thought; "no apology, no recap" prevents wasted output tokens
/// on regenerating context the model already produced.
const MAX_OUTPUT_TOKENS_RESUME_NUDGE: &str =
    "Output token limit hit. Resume directly — no apology, no recap of \
     what you were doing. Pick up mid-thought if that is where the cut \
     happened. Break remaining work into smaller pieces.";

/// Why a grace turn is being fired. Selects the nudge text; otherwise
/// the call path is identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GraceReason {
    /// `LoopDirective::FinalReply` — context-budget critical.
    Budget,
    /// `LoopDirective::StopDiminishing` — diminishing-returns detector trip.
    Diminishing,
    /// `max_iterations` cap reached in the outer loop.
    MaxIterations,
    /// `MAX_VERIFIER_VETOS` cap reached — model kept finishing with steps left.
    VerifierVeto,
    /// `consecutive_failure_cap` reached — repeated total-failure turns.
    ConsecutiveFailureCap,
    /// `ToolLoopVerifier` halted an unproductive tool-call loop — salvage a
    /// deliverable from what was already gathered instead of cold-terminating.
    ToolLoopHalt,
}

impl GraceReason {
    const fn nudge(self) -> &'static str {
        match self {
            Self::Budget => GRACE_NUDGE_BUDGET,
            Self::Diminishing => GRACE_NUDGE_DIMINISHING,
            Self::MaxIterations => GRACE_NUDGE_MAX_ITERATIONS,
            Self::VerifierVeto => GRACE_NUDGE_VERIFIER_VETO,
            Self::ConsecutiveFailureCap => GRACE_NUDGE_FAILURE_CAP,
            Self::ToolLoopHalt => GRACE_NUDGE_TOOL_LOOP_HALT,
        }
    }
}

/// True when a provider response carries no usable content at all — no
/// text, no `tool_calls` and no thinking. This is a provider failure mode
/// (degraded endpoint, context degradation), not a legitimate terminal
/// turn, and must not be reported to the user as a clean completion.
fn is_empty_response(response: &ProviderResponse) -> bool {
    response.text_content().trim().is_empty()
        && response.tool_calls.is_empty()
        && response.thinking.as_deref().unwrap_or("").trim().is_empty()
}

/// True iff the most recent `AssistantMessage` in `events` already carries
/// displayable text. When false, the budget short-circuit will inject one
/// grace turn so the user gets a terminal text response instead of a
/// mid-thought hang.
///
/// Returns `false` when there is no `AssistantMessage` yet (budget tripped
/// on the very first turn) — that path also deserves a grace turn.
fn last_assistant_has_text(events: &[SessionEventRecord]) -> bool {
    events
        .iter()
        .rev()
        .find_map(|r| match &r.event {
            SessionEvent::AssistantMessage { content, .. } => Some(!content.text.trim().is_empty()),
            _ => None,
        })
        .unwrap_or(false)
}

/// Estimate the token cost of the tool schema sent to the provider.
///
/// Mirrors the wire shape — name + description + JSON-serialized parameters —
/// so the context-budget sensor accounts for the per-request overhead the tool
/// definitions add on top of the conversation messages. Pure arithmetic
/// scaffolding (R10): no reasoning, no decision.
fn estimate_tool_schema_tokens(
    tools: &[crate::tool_metadata::ToolDefinition],
    ratio: f64,
) -> usize {
    use crate::memory::session_compactor::context_window::estimate_tokens;
    tools
        .iter()
        .map(|t| {
            estimate_tokens(&t.name, ratio)
                + estimate_tokens(&t.description, ratio)
                + estimate_tokens(&t.parameters.to_string(), ratio)
        })
        .sum()
}

/// Build a fresh `RequestPayload` borrowing the current message vec, system
/// prompt, and tool schema. Extracted to a free function (was a stack-local
/// closure capturing `&messages`) so the reactive-compaction rescue can take
/// `&mut messages` between consecutive LLM calls without NLL conflicts. The
/// returned payload's references stay alive only for the duration of the
/// awaited `.process()` call; once dropped, `messages` is freely mutable
/// again.
fn build_request_payload<'a>(
    system_prompt: Option<&'a str>,
    system_blocks: Option<&'a [crate::thinker::prompt_builder::SystemPromptPart]>,
    messages: &'a [UnifiedMessage],
    tools_ref: Option<&'a [crate::tool_metadata::ToolDefinition]>,
    session_id: &SessionId,
) -> RequestPayload<'a> {
    // Carry the session id as provider metadata: OpenAI-family adapters use it
    // as `prompt_cache_key` for cache-routing affinity, and the cost-metering
    // hooks key on `metadata["session_id"]` for per-session attribution.
    let mut metadata = std::collections::HashMap::with_capacity(1);
    metadata.insert("session_id".to_string(), session_id.to_string());
    let base = RequestPayload::new(messages)
        .with_tools(tools_ref)
        .with_metadata(Some(metadata));
    let base = match system_prompt {
        Some(sp) => base.with_system(Some(sp)),
        None => base,
    };
    base.with_system_blocks(system_blocks)
}

/// Hard cap on reactive-compaction rescue attempts per harness run. The
/// classifier yielded `CompactAndRetry { token_gap }` and the harness ran
/// `context_compactor.compact()` on the local message vec; one retry is
/// enough — repeated overflows after summarisation mean the input is
/// fundamentally too large rather than a recoverable burst. Mirrors
/// claude-code's "already attempted" single-shot guard (query.ts:1092).
const MAX_REACTIVE_COMPACT_ATTEMPTS: u32 = 1;

/// Plain-text tool-call promotion (openclaw `tool-call-repair` parity).
///
/// Weaker / proxied models sometimes emit a tool call as assistant **text**
/// (`<tool_call>{…}</tool_call>` or `<function=…>…</function>`) instead of a
/// provider-native function-call block. Left alone the harness reads
/// `tool_calls == []`, treats the turn as a clean finish, and the agent loop
/// stalls one step short of acting. This rewrites such text into structured
/// `NativeToolCall`s so the existing Act path dispatches them normally.
///
/// Runs only when the provider returned **no** native calls — a model that used
/// the native channel is never second-guessed. Promotion is all-or-nothing and
/// gated on tool-name resolution (see [`crate::tools::text_tool_call`]), so it
/// cannot misfire on prose. Mutates `response` in place and returns the number
/// of calls promoted (0 = untouched).
///
/// R10-safe: pure mechanical text→struct rewrite, no intent classification and
/// no policy — the model already decided to call the tool; this only repairs
/// the wire encoding the provider failed to structure.
fn promote_text_tool_calls(
    response: &mut ProviderResponse,
    tools: &[crate::tool_metadata::ToolDefinition],
) -> usize {
    if !response.tool_calls.is_empty() || tools.is_empty() {
        return 0;
    }
    let text = response.text_content();
    if text.is_empty() {
        return 0;
    }
    let allowed: std::collections::HashSet<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    let Some(promotion) =
        crate::tools::text_tool_call::promote_plain_text_tool_calls(&text, &allowed)
    else {
        return 0;
    };

    let promoted: Vec<NativeToolCall> = promotion
        .calls
        .into_iter()
        .enumerate()
        .map(|(i, c)| NativeToolCall {
            id: format!("promoted_{i}"),
            name: c.name,
            arguments: c.arguments,
            thought_signature: None,
        })
        .collect();
    let count = promoted.len();
    response.tool_calls = promoted;
    // The promoted markup must not also surface as assistant prose; keep only
    // the residual text the model wrote around the call.
    response.text = if promotion.residual_text.is_empty() {
        None
    } else {
        Some(promotion.residual_text)
    };
    response.stop_reason = StopReason::ToolUse;
    count
}

impl AgentHarness {
    /// Fold a single provider response's billed tokens into the run totals.
    ///
    /// Used for *intermediate* responses that the empty-response and
    /// `max_output_tokens` recovery loops discard before they reach the
    /// once-per-turn accounting at the end of `run_turn_internal`. Each
    /// discarded call was a real round-trip the provider billed (input tokens
    /// always, plus any partial output on a `max_output_tokens` cut), so
    /// counting only the final surviving response silently dropped those
    /// tokens from `total_tokens`, the per-component `token_breakdown`, and
    /// every downstream cost / budget consumer. Mirrors the final accounting
    /// (`turn_token_total` + `accumulate_token_breakdown`) so each call is
    /// counted exactly once. R10-safe: pure arithmetic, no decision.
    fn account_intermediate_tokens(&self, response: &ProviderResponse) {
        let tokens = super::turn_token_total(&response.usage);
        self.total_tokens.fetch_add(tokens, Ordering::Relaxed);
        self.accumulate_token_breakdown(&response.usage);
    }

    /// Internal turn execution with pre-computed counters to avoid O(n²)
    /// event-log scans in the outer loop.
    ///
    /// Returns a [`TurnStep`] describing the turn outcome. `split_child` is
    /// `Some(child)` only when a `SplitSession` directive succeeded; `None`
    /// in all other cases.
    pub(crate) async fn run_turn_internal(
        &self,
        session_id: &SessionId,
        callback: &mut dyn HarnessCallback,
        iterations: usize,
        tool_calls_made: usize,
        tool_history: &mut std::collections::VecDeque<ToolCallSummary>,
        parent_cancel: &CancellationToken,
    ) -> Result<TurnStep, HarnessError> {
        // Hold a sleep-inhibit assertion for the duration of this turn so a long
        // Think→Act cycle does not get cut off by macOS idle sleep. Drop happens
        // automatically when this scope exits, releasing the IOPMAssertion.
        let _sleep_guard = self.deps.power.as_ref().and_then(|power| {
            match power.inhibit_sleep("Aleph agent loop") {
                Ok(g) => Some(g),
                Err(e) => {
                    tracing::debug!(target: "power", "sleep inhibitor unavailable: {e}");
                    None
                }
            }
        });

        self.emit(|| crate::harness::trace::LoopTraceEvent::TurnStarted {
            iteration: iterations,
        });

        // 1. Fetch full event log and compute the tail boundary.
        let events = self.deps.session.get_events(session_id, None, None).await?;
        // Record the persisted-log boundary this turn's prompt is built from, so
        // the outer loop's follow-up check can tell a steering message that
        // arrived *during* this turn (index >= watermark) from input the prompt
        // already covered. Captured on the raw log before the guardrail's
        // in-memory rewrite, which never mutates the persisted event count.
        self.last_prompt_log_len
            .store(events.len(), Ordering::Relaxed);
        let tail_start = super::tail_start_index(&events);

        // 1a. Stage 5a (#9): Input guardrail. Inspect the latest UserMessage
        // in the tail. `Block` ends the turn early via `on_safety_block`;
        // `Sanitize` rewrites the in-memory event before the prompt builder
        // sees it (the original session-log event is left intact for audit).
        let events: Vec<crate::session::events::SessionEventRecord> =
            if let Some(registry) = self.deps.guardrails.as_ref() {
                match self
                    .apply_input_guardrail(registry, events, tail_start)
                    .await?
                {
                    InputGuardrailOutcome::Allow(events) => events,
                    InputGuardrailOutcome::Sanitized(events) => events,
                    InputGuardrailOutcome::Blocked(reason) => {
                        callback.on_safety_block(&reason);
                        return Ok(TurnStep::done());
                    }
                }
            } else {
                events
            };

        // 2. Build the LLM request. `build_prompt` has access to the full log
        //    so it can reconstruct the preceding assistant tool_use turn and
        //    resolve tool names for tool_result messages.
        let mut messages = super::prompt::build_prompt(&events, tail_start);

        // Fetch the cached metadata-form tool schema once. O(1) `Arc::clone`
        // on the steady-state path. Hoisted above BOTH the preflight pass and
        // the budget check so the preflight pressure gate and the budget sensor
        // account for the real tool-schema overhead.
        let metadata_tools = self.deps.tools.metadata_schema();
        let system_prompt = self.deps.system_prompt.as_deref().unwrap_or("");

        // 2a. Preflight cheap passes (hermes-inspired). Run BEFORE the budget
        // check so token-saving transforms — tool_result pruning + historical
        // image stripping — happen even if the compactor's side-channel LLM
        // call later fails. No LLM cost in this step.
        if let Some(pipeline) = self.deps.preflight_pipeline.as_ref() {
            // Compute the REAL pressure snapshot (read-only via `peek_pressure`
            // — no calibration or circuit-breaker mutation) so the pipeline's
            // preventive-band gate (`PreflightPipeline::min_pressure_ratio`,
            // wired from `ContextBudgetConfig::preventive_floor`, default 0.60)
            // fires the lossy cheap passes only when the context is actually
            // filling up. The old `ratio: 1.0` placeholder kept the gate
            // permanently open, so the passes ran on every turn even on a
            // near-empty context — contradicting the "calm runs pay nothing"
            // contract.
            // `fresh_tail` comes from the same `ContextBudget` the compactor
            // consults, keeping the protected-tail invariant intact. Falls back
            // to a max-pressure placeholder + `CompactorConfig::default` tail of
            // 6 only when no budget is wired (degenerate config), preserving the
            // previous always-on behaviour there.
            let (pressure, fresh_tail) = match self.deps.context_budget.as_ref() {
                Some(budget) => {
                    let guard = budget.lock().await;
                    let tool_tokens =
                        estimate_tool_schema_tokens(&metadata_tools, guard.token_estimate_ratio());
                    let pressure = guard.peek_pressure(&messages, system_prompt, tool_tokens);
                    (pressure, guard.fresh_tail_count())
                }
                None => (
                    crate::context::budget::ContextPressure {
                        used_tokens: 0,
                        budget_tokens: 0,
                        ratio: 1.0,
                        overhead_tokens: 0,
                        available_for_messages: 0,
                    },
                    6usize,
                ),
            };
            let freed = pipeline.run(&mut messages, &pressure, fresh_tail).await;
            if freed > 0 {
                tracing::debug!(
                    ?session_id,
                    tokens_freed = freed,
                    "preflight cheap passes saved tokens",
                );
            }
        }

        // 2b. Task-10 budget check: evaluate context pressure before issuing
        // the LLM call. The sensor now sees the real system prompt and
        // tool-schema overhead (previously passed empty), so compaction and
        // `FinalReply` fire on the true context size, not just message tokens.
        let (budget_directive, budget_tool_tokens) =
            if let Some(budget) = self.deps.context_budget.as_ref() {
                let mut guard = budget.lock().await;
                let tool_tokens =
                    estimate_tool_schema_tokens(&metadata_tools, guard.token_estimate_ratio());
                let directive = guard.before_turn(&messages, system_prompt, tool_tokens);
                (Some(directive), tool_tokens)
            } else {
                (None, 0usize)
            };

        // 2c. Compact when directive calls for it and a compactor is wired.
        if matches!(budget_directive, Some(LoopDirective::CompactAndContinue)) {
            if let Some(compactor) = self.deps.context_compactor.as_ref() {
                // `fresh_tail = 0` lets the compactor fall back to its own
                // config default (matches Task 6 spec). The session id enables
                // the compactor's zero-API-cost reuse of hierarchical session
                // summaries when a memory backend is wired.
                let session_key_str = session_id.to_key_string();
                match compactor
                    .compact(&mut messages, 0, Some(session_key_str.as_str()))
                    .await
                {
                    Ok(_) => {
                        // Re-arm the circuit breaker only when this compaction
                        // actually reduced pressure. An ineffective compaction
                        // leaves the breaker counting, so a thrashing run still
                        // escalates to `FinalReply` (hermes anti-thrash).
                        if let Some(budget) = self.deps.context_budget.as_ref() {
                            let system_prompt = self.deps.system_prompt.as_deref().unwrap_or("");
                            budget.lock().await.note_compaction_effect(
                                &messages,
                                system_prompt,
                                budget_tool_tokens,
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            ?session_id,
                            ?e,
                            "context compactor failed; continuing with uncompacted messages",
                        );
                    }
                }
            }
        }

        // 2c-split. `SplitSession` directive — attempt compaction-driven session
        // split. On success, return `TurnState::Continue` with the child session
        // id so `run()` can rebind `current_session`. On failure or when the
        // registrar/compactor is not wired, fall back to the `FinalReply` path.
        // R10-safe: mechanical dispatch to `perform_session_split` (lives outside
        // the harness); no intent classification, no new heuristic.
        if matches!(budget_directive, Some(LoopDirective::SplitSession)) {
            let split_child = match (
                self.deps.context_compactor.as_ref(),
                self.deps.session_epoch_registrar.as_ref(),
            ) {
                (Some(compactor), Some(registrar)) => {
                    match crate::context::compact::session_split::perform_session_split(
                        self.deps.session.as_ref(),
                        registrar.as_ref(),
                        compactor.as_ref(),
                        session_id,
                        &events,
                        tail_start,
                    )
                    .await
                    {
                        Ok(outcome) => {
                            if let Some(budget) = self.deps.context_budget.as_ref() {
                                budget.lock().await.record_split();
                            }
                            tracing::info!(
                                ?session_id,
                                child = ?outcome.child_session_id,
                                "session split: continuing run in child session",
                            );
                            Some(outcome.child_session_id)
                        }
                        Err(e) => {
                            tracing::warn!(
                                ?session_id,
                                %e,
                                "session split failed; falling back to FinalReply",
                            );
                            None
                        }
                    }
                }
                _ => None, // compactor or registrar not wired — fall back to FinalReply
            };

            if let Some(child) = split_child {
                // Continue the run in the child; run() rebinds current_session.
                return Ok(TurnStep {
                    state: TurnState::Continue,
                    executed: 0,
                    vetoed: false,
                    split_child: Some(child),
                });
            }
            // Fail-soft: behave like the FinalReply branch.
            self.hit_limit.store(true, Ordering::Relaxed);
            self.set_terminate_reason(
                crate::orchestrator::dispatch::TerminateReason::ContextBudgetExhausted,
            );
            self.fire_grace_turn(
                session_id,
                &events,
                &messages,
                callback,
                iterations,
                GraceReason::Budget,
                parent_cancel,
            )
            .await;
            return Ok(TurnStep::done());
        }

        // 2d. `FinalReply` directive — record hit_limit and short-circuit to
        // Done. Hermes-inspired grace turn: if the most recent assistant
        // turn ended without text (unresolved tool_use, or no assistant
        // turn yet), issue exactly one tool-less LLM call so the user
        // gets a terminal text response instead of a mid-thought hang.
        // Tools are stripped both via `.with_tools(None)` (implicit by
        // omitting the call) and via the grace nudge message, so the LLM
        // cannot recurse. Fail-soft: any error falls through silently.
        // R10-safe: one extra LLM call gated by an existing directive,
        // no new policy, no state machine.
        if matches!(budget_directive, Some(LoopDirective::FinalReply)) {
            self.hit_limit.store(true, Ordering::Relaxed);
            self.set_terminate_reason(
                crate::orchestrator::dispatch::TerminateReason::ContextBudgetExhausted,
            );
            self.fire_grace_turn(
                session_id,
                &events,
                &messages,
                callback,
                iterations,
                GraceReason::Budget,
                parent_cancel,
            )
            .await;
            return Ok(TurnStep::done());
        }

        // 2d-G1. Last-step soft hint (opencode parity). When the iteration
        // cap is set AND this is the LAST allowed turn, append a synthetic
        // `<system-reminder>` user message warning the model to respond with
        // text only. Saves the post-hoc C1 grace-turn LLM round-trip in the
        // common case; C1 remains as fail-safe if the model still emits a
        // tool_use. R10-safe: static text, no reasoning, no policy.
        if let Some(cap) = self.deps.max_iterations {
            if cap > 0 && iterations.saturating_add(1) >= cap {
                messages.push(crate::providers::message::UnifiedMessage::User {
                    content: vec![crate::providers::message::ContentBlock::Text {
                        text: MAX_STEPS_HINT.to_string(),
                        cache_control: None,
                    }],
                });
                tracing::debug!(iterations, cap, "max-iterations soft hint injected (G1)",);
            }
        }

        // 2d. Derive the optional tool-schema reference for the request payload
        // from the metadata tools fetched above (Stage 2). Cache invalidation
        // is owned by `ToolService` impls; see `to_metadata_form`.
        let tools_ref: Option<&[crate::tool_metadata::ToolDefinition]> =
            if metadata_tools.is_empty() {
                None
            } else {
                Some(metadata_tools.as_ref())
            };

        // The request payload is rebuilt fresh on each call — H3's
        // empty-response retry re-issues it, and the rescue helper may
        // mutate `messages` via the compactor between calls. The dedicated
        // `build_request_payload` free function (above) avoids the
        // closure-borrows-messages NLL conflict that would otherwise block
        // `&mut messages` inside `try_reactive_compact_and_retry`.

        self.emit(|| crate::harness::trace::LoopTraceEvent::TurnStateEntered {
            iteration: iterations,
            state: crate::harness::trace::LoopTraceState::Think,
        });

        // 3. Call the LLM, racing against cancel + turn-timeout. Provider-tier
        // failover — the ordered chain, model-level fallback, and the circuit
        // breaker — lives inside `deps.llm` itself (`providers::FailoverProvider`),
        // so the harness simply propagates whatever error survives it.
        let started = std::time::Instant::now();
        let payload = build_request_payload(
            self.deps.system_prompt.as_deref(),
            self.deps.system_prompt_parts.as_deref(),
            &messages,
            tools_ref,
            session_id,
        );
        // Streaming gate: live token deltas require an HTTP provider seam and
        // no output guardrail. The output guardrail (Stage 5a below) sanitises
        // the FINAL assembled text, which cannot be applied retroactively to an
        // already-emitted incremental stream — so guardrailed turns keep the
        // one-shot emit. Mock/non-HTTP providers also fall through. When the
        // gate is open, `stream_llm_call` forwards text/thinking deltas live
        // and we suppress the once-per-turn `on_delta` further down.
        let provider_streams =
            self.deps.guardrails.is_none() && self.deps.llm.as_http_provider().is_some();
        let primary_call = if provider_streams {
            self.stream_llm_call(payload, callback, parent_cancel, started)
                .await?
        } else {
            self.race_llm_call(self.deps.llm.process(payload), parent_cancel, started)
                .await?
        };
        // Whether the SURVIVING `response` was produced by the live streaming
        // call. Every retry/rescue below re-issues the request through the
        // non-streaming `race_llm_call`, so its text was never delta-streamed
        // and must keep the one-shot `on_delta` emit further down — otherwise
        // recovered text silently never reaches live stream consumers
        // (BroadcastCallback → FlowStreamEvent::Delta has no other source).
        let mut response_was_streamed = provider_streams;
        let mut response = match primary_call {
            Ok(r) => r,
            Err(primary_err) => {
                response_was_streamed = false;
                // Reactive-compaction rescue (Phase A): when the classifier
                // tags the error as `CompactAndRetry`, summarise `messages`
                // and retry once. Returns `Err(HarnessError::Llm)` when the
                // verdict is anything else (transparent passthrough).
                self.try_reactive_compact_and_retry(
                    primary_err,
                    session_id,
                    &mut messages,
                    tools_ref,
                    budget_tool_tokens,
                    parent_cancel,
                    started,
                )
                .await?
            }
        };

        // 3a. Empty-response guard (H3). A response with no text, no
        // tool_calls and no thinking is a provider failure mode, not a
        // terminal turn — left unchecked it is misreported to the user as a
        // clean completion. Re-issue the call up to EMPTY_RESPONSE_RETRIES
        // times (pure round scheduling, no reasoning). If still empty after
        // retries, a distinct terminate reason keeps the trace honest.
        let mut empty_retries = 0u32;
        while is_empty_response(&response) && empty_retries < EMPTY_RESPONSE_RETRIES {
            empty_retries += 1;
            // The retry below replaces `response` via the non-streaming path.
            response_was_streamed = false;
            // Count the empty call we are about to discard — it was still a
            // billed round-trip (input tokens), and the final once-per-turn
            // accounting below only sees the surviving response.
            self.account_intermediate_tokens(&response);
            tracing::warn!(
                ?session_id,
                empty_retries,
                "provider returned an empty response; retrying",
            );
            let retry_payload = build_request_payload(
                self.deps.system_prompt.as_deref(),
                self.deps.system_prompt_parts.as_deref(),
                &messages,
                tools_ref,
                session_id,
            );
            response = match self
                .race_llm_call(self.deps.llm.process(retry_payload), parent_cancel, started)
                .await?
            {
                Ok(r) => r,
                Err(primary_err) => {
                    self.try_reactive_compact_and_retry(
                        primary_err,
                        session_id,
                        &mut messages,
                        tools_ref,
                        budget_tool_tokens,
                        parent_cancel,
                        started,
                    )
                    .await?
                }
            };
        }
        // 3b-pre. Context-window-exceeded recovery (claude-code parity).
        // `model_context_window_exceeded` means the *context window* — not
        // the output cap — filled mid-generation. The resume-nudge loop in
        // 3b would append more messages and re-hit the wall, so route this
        // to the same reactive-compaction rescue that `prompt_too_long`
        // errors take: synthesize an error carrying the overflow marker
        // (which `llm_retry::classify` maps to `CompactAndRetry`) and let
        // the helper reuse its one-shot cap, trace, and budget plumbing.
        // The loop is bounded by that cap: each pass either returns a
        // compacted-and-retried response or errors out when rescues are
        // exhausted, so a model that keeps overflowing cannot spin.
        while matches!(
            response.stop_reason,
            crate::providers::adapter::StopReason::ContextWindowExceeded
        ) {
            // The overflowed call still billed input plus the partial output;
            // count it before the retry replaces `response`.
            self.account_intermediate_tokens(&response);
            // The rescue below replaces `response` via the non-streaming path.
            response_was_streamed = false;
            tracing::warn!(
                ?session_id,
                "provider stopped with model_context_window_exceeded; \
                 routing to reactive compaction",
            );
            let overflow_err = crate::error::AlephError::ProviderError {
                message: "model_context_window_exceeded: provider stopped because \
                          the context window is full"
                    .to_string(),
                suggestion: None,
            };
            response = self
                .try_reactive_compact_and_retry(
                    overflow_err,
                    session_id,
                    &mut messages,
                    tools_ref,
                    budget_tool_tokens,
                    parent_cancel,
                    started,
                )
                .await?;
        }

        // 3b. max_output_tokens recovery (claude-code parity, query.ts:1188).
        // When the provider hits its output-token cap mid-stream we get
        // `stop_reason == MaxTokens` plus whatever partial text it managed
        // to emit. A clean reissue with the same messages would re-prompt
        // from scratch (model re-doing work). Instead append the partial
        // assistant text + a meta "resume directly" user nudge to the
        // local message vec and retry up to MAX_OUTPUT_TOKENS_RECOVERY_LIMIT
        // times. The nudge text mirrors claude-code's wording so model
        // behaviour transfers across harnesses. R10-safe: pure round
        // scheduling around a specific provider failure mode, no policy.
        // The retry pushes onto `messages` (local, never persisted to the
        // session log). The closure `build_payload` would hold an immutable
        // borrow that conflicts with the push; the retry inlines payload
        // construction so the borrow is scoped to each LLM call.
        let mut max_tokens_retries = 0u32;
        while matches!(
            response.stop_reason,
            crate::providers::adapter::StopReason::MaxTokens
        ) && max_tokens_retries < MAX_OUTPUT_TOKENS_RECOVERY_LIMIT
        {
            max_tokens_retries += 1;
            // The retry below replaces `response` via the non-streaming path;
            // only the truncated partial was delta-streamed, so the surviving
            // continuation must get the one-shot emit.
            response_was_streamed = false;
            // Count the partial (max_output_tokens-cut) call before discarding
            // it: it billed input tokens plus the partial output the model
            // already emitted. The once-per-turn accounting below only sees
            // the final response, so without this those tokens are lost.
            self.account_intermediate_tokens(&response);
            tracing::warn!(
                ?session_id,
                max_tokens_retries,
                "provider hit max_output_tokens; retrying with resume nudge",
            );
            let partial = response.text_content();
            if !partial.trim().is_empty() {
                messages.push(UnifiedMessage::assistant(partial));
            }
            messages.push(UnifiedMessage::user(MAX_OUTPUT_TOKENS_RESUME_NUDGE));
            let payload = build_request_payload(
                self.deps.system_prompt.as_deref(),
                self.deps.system_prompt_parts.as_deref(),
                &messages,
                tools_ref,
                session_id,
            );
            response = match self
                .race_llm_call(self.deps.llm.process(payload), parent_cancel, started)
                .await?
            {
                Ok(r) => r,
                Err(primary_err) => {
                    // Even mid-recovery, a `prompt_too_long` may slip
                    // through after the resume nudge — rescue applies the
                    // same way; on non-overflow errors the helper returns
                    // the wrapped error unchanged.
                    self.try_reactive_compact_and_retry(
                        primary_err,
                        session_id,
                        &mut messages,
                        tools_ref,
                        budget_tool_tokens,
                        parent_cancel,
                        started,
                    )
                    .await?
                }
            };
        }
        // Exit-point reason fidelity. Capture the surviving response's
        // degradation verdicts here — after every bounded recovery loop has
        // had its chance to replace `response`, and before tool-call
        // promotion / output guardrails may rewrite it. The Done branch below
        // converts a verdict into a terminate reason only when this turn
        // actually ends the run: setting them mid-turn (the previous shape)
        // left a stale `MaxOutputTokensExhausted` behind whenever a truncated
        // response still carried tool calls and the run later completed
        // cleanly, and a stale `EmptyResponseExhausted` whenever a rescue
        // replaced an empty response or a verifier veto forced a continue.
        let empty_after_retries = is_empty_response(&response);
        let max_tokens_after_recovery = matches!(
            response.stop_reason,
            crate::providers::adapter::StopReason::MaxTokens
        );

        // 3c. Plain-text tool-call promotion (openclaw tool-call-repair
        // parity). When the provider returned text that *encodes* a tool call
        // but no native tool_calls, rewrite it into structured calls so the
        // Act path below dispatches them instead of mistaking the turn for a
        // clean finish. No-op on the common native path. Runs after the
        // empty-response and max_output_tokens recovery so it sees the final
        // text the model actually produced.
        let promoted = promote_text_tool_calls(&mut response, &metadata_tools);
        if promoted > 0 {
            tracing::info!(
                ?session_id,
                promoted,
                "promoted plain-text tool call(s) to native tool_calls",
            );
        }

        // Accumulate this turn's provider-reported token usage. Counted here
        // — right after the LLM call — so a turn whose output is later
        // blocked by a guardrail still reflects the tokens the provider
        // billed. Excludes `thinking_tokens`; see `turn_token_total`.
        let turn_tokens = super::turn_token_total(&response.usage);
        self.total_tokens.fetch_add(turn_tokens, Ordering::Relaxed);
        // P2: per-component breakdown — captures cache hit ratio and
        // reasoning-token spend that the single `total_tokens` sum hides.
        // Reasoning is folded as `thinking_tokens` even when `total_tokens`
        // excludes it (Anthropic already includes it in `output`; Gemini
        // reports it separately).
        self.accumulate_token_breakdown(&response.usage);
        // Cycle 3 — OUTPUT tokens only (not total), required by
        // DiminishingReturnsDetector's window threshold semantics.
        let output_tokens = response
            .usage
            .as_ref()
            .map_or(0, |u| u.output_tokens as usize);

        // Calibrate the context-budget token estimator against the provider's
        // ground-truth prompt size. `last_pressure` (set by `before_turn`, or
        // refreshed by `note_compaction_effect`) is the calibrated estimate of
        // exactly this prompt, so feeding back the real `prompt_tokens_total`
        // converges the estimate to this conversation's true tokenizer ratio.
        // R10-safe: pure accuracy feedback, no new decision category.
        //
        // Skipped when the `max_output_tokens` recovery loop ran: that path
        // appends the partial assistant text + resume nudge to `messages`
        // AFTER `before_turn` snapshotted `last_pressure`, so the surviving
        // response's `prompt_tokens_total` no longer measures the same prompt
        // the estimate was taken on. Feeding that mismatched ratio into the
        // EWMA would inject a spurious inflationary correction. (Empty-response
        // retries re-issue the identical `messages`, so they stay valid samples
        // and are intentionally NOT excluded here.)
        if max_tokens_retries == 0 {
            if let (Some(budget), Some(usage)) =
                (self.deps.context_budget.as_ref(), response.usage.as_ref())
            {
                let observed = usage.prompt_tokens_total() as usize;
                budget.lock().await.observe_actual_usage(observed);
            }
        }

        // 4. Emit AssistantMessage preserving any tool_use intent in `blocks`.
        let turn_id = super::current_turn_id(&events);
        let text = response.text_content();

        // 4a. Stage 5a (#9): Output guardrail. `Block` aborts the turn with a
        // terminal `HarnessError::Llm`. The decision's `class` is preserved
        // through the wrapped `AlephError` so `HarnessError::class()` (and the
        // security-block trace) reflects whether this was a content-policy
        // block (`Fixable`) or a fail-closed security-infra error
        // (`Unexpected`). NOTE: the orchestrator's retry-vs-terminal decision
        // is currently message-based (`harness_bridge/error.rs`) and does not
        // yet branch on `class` — both currently terminate. `Sanitize` rewrites
        // the text before persistence and stream. Tool-use blocks are not
        // rewritten here — Stage 5b's `ToolCallGuardrail` covers their args.
        let text = if let Some(registry) = self.deps.guardrails.as_ref() {
            match registry.evaluate_output(&text).await {
                crate::guardrails::GuardrailDecision::Allow => text,
                crate::guardrails::GuardrailDecision::Warn { reason } => {
                    tracing::warn!(?session_id, reason = %reason, "output guardrail warned");
                    text
                }
                crate::guardrails::GuardrailDecision::Sanitize(rep) => {
                    callback.on_safety_block(&format!("output sanitized by {}", rep.source));
                    rep.text
                }
                crate::guardrails::GuardrailDecision::Block { reason, class } => {
                    callback.on_safety_block(&reason);
                    let msg = format!("output guardrail blocked: {reason}");
                    // Preserve the guardrail's ErrorClass through the wrapped
                    // AlephError: `Fixable` (model-correctable content/leak
                    // policy) → a Fixable-classed error; everything else
                    // (e.g. a fail-closed security-infra failure) → Unexpected.
                    let err = match class {
                        crate::error::ErrorClass::Fixable => {
                            crate::error::AlephError::Validation(msg)
                        }
                        _ => crate::error::AlephError::other(msg),
                    };
                    return Err(HarnessError::Llm(err));
                }
            }
        } else {
            text
        };

        if !text.is_empty() {
            // For streamed turns the incremental text deltas were already
            // forwarded live during the LLM call (`stream_llm_call`), so a
            // second full-text `on_delta` here would double the content in
            // append-only consumers (BroadcastCallback → FlowStreamEvent::Delta).
            // Non-streaming turns (mock/non-HTTP providers, or guardrailed turns
            // where streaming is suppressed so the output guardrail can sanitise
            // the final text first) keep the one-shot emit — as does a response
            // that survived from a non-streaming retry/rescue, whose text was
            // never delta-streamed. Either way the authoritative
            // AssistantMessage is persisted just below.
            if !response_was_streamed {
                callback.on_delta(&text);
            }
            self.emit(|| crate::harness::trace::LoopTraceEvent::TextEmitted {
                iteration: iterations,
                stream: crate::harness::trace::LoopTraceTextKind::Final,
                text: text.clone(),
            });
        }
        let blocks = super::tool_use_blocks(&response.tool_calls);
        let assistant_event = SessionEvent::AssistantMessage {
            turn_id,
            content: MessageContent {
                text: text.clone(),
                blocks,
                thinking: response.thinking.clone(),
                thinking_signature: response.thinking_signature.clone(),
            },
            at: crate::session::events::now_ms(),
        };
        self.deps
            .session
            .emit_event(session_id, assistant_event)
            .await?;

        // Record activity after Think completes so a long Think doesn't
        // falsely trip the stall detector on the next iteration.
        if let Some(ref tracker) = self.stall_tracker {
            tracker.record_activity().await;
        }

        // 5. Stage 6a (#10): VerifierChain runs every turn — StopHook + ToolLoop.
        for tc in &response.tool_calls {
            if tool_history.len() == TOOL_HISTORY_WINDOW {
                tool_history.pop_front();
            }
            tool_history.push_back(ToolCallSummary {
                name: tc.name.clone(),
                args_hash: hash_tool_args(&tc.arguments),
            });
        }
        let stop_reason = response.tool_calls.is_empty().then_some("end_turn");
        let session_key = session_id.to_key_string();
        let verdict = self
            .run_verifiers(
                iterations,
                tool_calls_made,
                &text,
                tool_history,
                stop_reason,
                &session_key,
                parent_cancel,
            )
            .await;
        // Metrics for the non-executing verdict turns (verifier Halt / Veto and
        // the empty-tool-call Stop). `executed`/`productive` are genuinely zero
        // — nothing ran — but the model still *requested* tool calls on a
        // Halt/Veto turn (the verifier rejected them). Reporting `requested: 0`
        // there silently erased the attempt from trace telemetry. The Stop
        // branch naturally lands `len() == 0` since its tool list is empty.
        let verdict_metrics = crate::harness::trace::LoopTraceTurnMetrics {
            requested_tool_calls: response.tool_calls.len(),
            executed_tool_calls: 0,
            productive: false,
            consecutive_errors: 0,
            total_tokens: turn_tokens as usize,
        };
        let outcome_for_trace;
        let metrics_for_trace;
        let result;
        if let VerifierVerdict::Halt { reason, .. } = verdict {
            tracing::info!(?session_id, reason = %reason, "verifier halted loop");
            // Persist the halt reason as a UserMessage event so transcript
            // consumers see the same termination message claude-code's
            // `preventContinuation` surfaces. Mirror the Veto pattern; the
            // difference is we set TerminateReason::StopHookHalt and exit
            // immediately (no further turns).
            let new_turn = uuid::Uuid::new_v4();
            let halt_event = SessionEvent::UserMessage {
                turn_id: new_turn,
                content: MessageContent {
                    text: format!("[stop hook halt] {reason}"),
                    blocks: Vec::new(),
                    thinking: None,
                    thinking_signature: None,
                },
                at: crate::session::events::now_ms(),
                synthetic: true,
            };
            self.deps.session.emit_event(session_id, halt_event).await?;
            // ② Close the tool_use blocks this halted turn emitted but never ran
            // — otherwise the prompt builder drops them as orphans and erases the
            // turn's context from the salvage prompt below.
            let pending_tool_use_ids: Vec<String> =
                response.tool_calls.iter().map(|c| c.id.clone()).collect();
            self.close_unexecuted_tool_uses(
                session_id,
                turn_id,
                &pending_tool_use_ids,
                "tool-loop halt",
            )
            .await;
            // ① Salvage: turn the dead halt into a final deliverable built from
            // what was already gathered, so the user gets an answer instead of
            // only the "stop hook" apology. Skips itself if the turn already has
            // terminal text. Fail-soft on any LLM error.
            self.fire_boundary_grace_turn(
                session_id,
                callback,
                iterations,
                GraceReason::ToolLoopHalt,
                parent_cancel,
            )
            .await;
            self.set_terminate_reason(
                crate::orchestrator::dispatch::TerminateReason::StopHookHalt {
                    reason: reason.clone(),
                },
            );
            self.hit_limit.store(true, Ordering::Relaxed);
            self.emit(|| crate::harness::trace::LoopTraceEvent::TurnCompleted {
                iteration: iterations,
                outcome: crate::harness::trace::LoopTraceTurnOutcome::Stop,
                metrics: verdict_metrics,
            });
            return Ok(TurnStep::done());
        } else if let VerifierVerdict::Veto { reason, .. } = verdict {
            tracing::info!(?session_id, reason = %reason, "verifier vetoed; forcing continue");
            // Surface the interception reason on the user-facing trace stream
            // (Panel / CLI follow / channel push), not just the model-facing
            // synthetic `[verifier veto]` message below. Pure observability —
            // mirrors an already-computed reason, adds no loop cognition.
            self.emit(|| crate::harness::trace::LoopTraceEvent::VerifierVeto {
                iteration: iterations,
                reason: reason.clone(),
            });
            let new_turn = uuid::Uuid::new_v4();
            let block_event = SessionEvent::UserMessage {
                turn_id: new_turn,
                content: MessageContent {
                    text: format!("[verifier veto] {reason}"),
                    blocks: Vec::new(),
                    thinking: None,
                    thinking_signature: None,
                },
                at: crate::session::events::now_ms(),
                synthetic: true,
            };
            self.deps
                .session
                .emit_event(session_id, block_event)
                .await?;
            // ② A vetoed turn re-enters the loop without executing its tool
            // calls, so close them out — an orphaned tool_use would be dropped
            // by the next prompt build, taking this turn's context (and the veto
            // feedback's pairing) with it.
            let pending_tool_use_ids: Vec<String> =
                response.tool_calls.iter().map(|c| c.id.clone()).collect();
            self.close_unexecuted_tool_uses(
                session_id,
                turn_id,
                &pending_tool_use_ids,
                "verifier veto",
            )
            .await;
            outcome_for_trace = crate::harness::trace::LoopTraceTurnOutcome::Continue;
            metrics_for_trace = verdict_metrics;
            result = Ok(TurnStep {
                state: TurnState::Continue,
                executed: 0,
                vetoed: true,
                split_child: None,
            });
        } else if response.tool_calls.is_empty() {
            // This turn ends the run, so a degraded final response IS the
            // loop-exit cause. Both flags were captured right after their
            // bounded recovery loops; a clean final turn leaves the default
            // `Completed` untouched.
            if max_tokens_after_recovery {
                self.set_terminate_reason(
                    crate::orchestrator::dispatch::TerminateReason::MaxOutputTokensExhausted,
                );
            } else if empty_after_retries {
                self.set_terminate_reason(
                    crate::orchestrator::dispatch::TerminateReason::EmptyResponseExhausted,
                );
            }
            outcome_for_trace = crate::harness::trace::LoopTraceTurnOutcome::Stop;
            metrics_for_trace = verdict_metrics;
            result = Ok(TurnStep::done());
        } else {
            self.emit(|| crate::harness::trace::LoopTraceEvent::TurnStateEntered {
                iteration: iterations,
                state: crate::harness::trace::LoopTraceState::Act,
            });
            let requested = response.tool_calls.len();
            let executed = self
                .act(
                    session_id,
                    turn_id,
                    response.tool_calls,
                    callback,
                    iterations,
                    parent_cancel,
                )
                .await?;
            outcome_for_trace = crate::harness::trace::LoopTraceTurnOutcome::Continue;
            metrics_for_trace = crate::harness::trace::LoopTraceTurnMetrics {
                requested_tool_calls: requested,
                executed_tool_calls: executed,
                productive: executed > 0,
                consecutive_errors: 0,
                total_tokens: turn_tokens as usize,
            };
            result = Ok(TurnStep::cont(executed));
        }

        // Cycle 3 — wire DiminishingReturnsDetector. `after_turn` had zero
        // production callsites before this commit. Skipped on a verifier veto:
        // a veto is already a guardrail intervention and must not also feed the
        // diminishing-returns window. StopDiminishing reuses the Task-5
        // grace-turn helper to give the user a terminal summary, mirroring
        // the FinalReply path. R10-safe: no new directive variant, no new
        // decision category — `StopDiminishing` already existed.
        //
        // The veto flag is the 3rd element of `result`; `verdict` was moved
        // into the if-let binding above and is no longer in scope.
        let is_verifier_veto = matches!(&result, Ok(step) if step.vetoed);
        if !is_verifier_veto {
            let after_directive = if let Some(budget) = self.deps.context_budget.as_ref() {
                let mut guard = budget.lock().await;
                Some(guard.after_turn(crate::context::budget::TurnMetrics {
                    output_tokens,
                    tool_calls: metrics_for_trace.requested_tool_calls,
                    productive: metrics_for_trace.productive,
                }))
            } else {
                None
            };
            if matches!(after_directive, Some(LoopDirective::StopDiminishing)) {
                self.hit_limit.store(true, Ordering::Relaxed);
                // Every other cap path records its exit cause; without this
                // the run reported whatever reason was last set (usually the
                // default `Completed`) despite exiting on diminishing returns.
                self.set_terminate_reason(
                    crate::orchestrator::dispatch::TerminateReason::DiminishingReturns,
                );
                self.fire_grace_turn(
                    session_id,
                    &events,
                    &messages,
                    callback,
                    iterations,
                    GraceReason::Diminishing,
                    parent_cancel,
                )
                .await;
                self.emit(|| crate::harness::trace::LoopTraceEvent::TurnCompleted {
                    iteration: iterations,
                    outcome: crate::harness::trace::LoopTraceTurnOutcome::Stop,
                    metrics: metrics_for_trace.clone(),
                });
                return Ok(TurnStep {
                    state: TurnState::Done,
                    executed: metrics_for_trace.executed_tool_calls,
                    vetoed: false,
                    split_child: None,
                });
            }
        }

        self.emit(|| crate::harness::trace::LoopTraceEvent::TurnCompleted {
            iteration: iterations,
            outcome: outcome_for_trace,
            metrics: metrics_for_trace,
        });
        result
    }

    /// Reactive-compaction rescue (Phase A — claude-code parity,
    /// query.ts:1092-1162; codex `mid-turn auto-compact` parity).
    ///
    /// When the LLM call returned a provider error, classify it via
    /// [`crate::providers::llm_retry::classify`]. If the verdict is
    /// `RetryVerdict::CompactAndRetry { token_gap }`, run
    /// `context_compactor.compact()` on the in-flight `messages` vec and
    /// retry the LLM call ONCE. On any other verdict — or if the
    /// compactor is not wired, the rescue cap is already exhausted, the
    /// compactor itself fails, or the retried call still errors — return
    /// the wrapped error so the caller surfaces it.
    ///
    /// This closes the dead wire flagged at `failover.rs:183` ("the
    /// harness context-compactor owns this recovery path") — before this
    /// helper existed the verdict was generated but no harness path
    /// consumed it.
    ///
    /// R10 discipline: the helper is **scaffolding** — it picks no policy
    /// and makes no judgements. The decision to retry is fully encoded in
    /// the classifier verdict; the helper just dispatches the
    /// pre-existing compactor on the pre-existing message vec and emits
    /// one trace event for observability.
    async fn try_reactive_compact_and_retry(
        &self,
        primary_err: crate::error::AlephError,
        session_id: &SessionId,
        messages: &mut Vec<UnifiedMessage>,
        tools_ref: Option<&[crate::tool_metadata::ToolDefinition]>,
        budget_tool_tokens: usize,
        parent_cancel: &CancellationToken,
        started: std::time::Instant,
    ) -> Result<ProviderResponse, HarnessError> {
        use crate::providers::llm_retry::{classify, RetryVerdict};

        // 1. Classify. Anything that isn't `CompactAndRetry` is a clean
        //    pass-through — preserve the original error so the orchestrator
        //    sees identical pre-rescue semantics.
        let token_gap = match classify(&primary_err.to_string()) {
            RetryVerdict::CompactAndRetry { token_gap } => token_gap,
            _ => return Err(HarnessError::Llm(primary_err)),
        };

        // 2. The compactor must be wired AND we must still have a rescue
        //    slot. `try_reserve_reactive_compact` is a one-shot
        //    `compare_exchange` so concurrent paths can never both rescue.
        let Some(compactor) = self.deps.context_compactor.as_ref() else {
            self.emit(
                || crate::harness::trace::LoopTraceEvent::ReactiveCompactionAttempted {
                    token_gap,
                    succeeded: false,
                },
            );
            self.set_terminate_reason(
                crate::orchestrator::dispatch::TerminateReason::ReactiveCompactExhausted,
            );
            return Err(HarnessError::Llm(primary_err));
        };
        if !self.try_reserve_reactive_compact() {
            tracing::warn!(
                ?session_id,
                MAX_REACTIVE_COMPACT_ATTEMPTS,
                "reactive-compaction rescue cap reached; surfacing original error",
            );
            self.emit(
                || crate::harness::trace::LoopTraceEvent::ReactiveCompactionAttempted {
                    token_gap,
                    succeeded: false,
                },
            );
            self.set_terminate_reason(
                crate::orchestrator::dispatch::TerminateReason::ReactiveCompactExhausted,
            );
            return Err(HarnessError::Llm(primary_err));
        }

        // 3. Run the compactor on the in-flight message vec. Failure here
        //    is fail-soft: the original provider error is what the user
        //    needs to see, the compactor's own error is secondary noise.
        tracing::warn!(
            ?session_id,
            ?token_gap,
            "provider hit context overflow; running reactive compaction",
        );
        let session_id_str = session_id.to_string();
        if let Err(e) = compactor
            .compact(messages, 0, Some(session_id_str.as_str()))
            .await
        {
            tracing::warn!(
                ?session_id,
                error = %e,
                "reactive compactor failed; surfacing original provider error",
            );
            self.emit(
                || crate::harness::trace::LoopTraceEvent::ReactiveCompactionAttempted {
                    token_gap,
                    succeeded: false,
                },
            );
            self.set_terminate_reason(
                crate::orchestrator::dispatch::TerminateReason::ReactiveCompactExhausted,
            );
            return Err(HarnessError::Llm(primary_err));
        }

        // 3a. Refresh the budget's `last_pressure` snapshot to the compacted
        //     message vec. `before_turn` snapshotted the *pre*-compaction
        //     prompt; without this refresh the post-retry
        //     `observe_actual_usage` calibration would divide the surviving
        //     response's real `prompt_tokens_total` (compacted) by the stale
        //     uncompacted estimate, injecting a spurious shrink into the EWMA
        //     and corrupting every later compaction decision this run. Mirrors
        //     the `CompactAndContinue` path's `note_compaction_effect` call.
        if let Some(budget) = self.deps.context_budget.as_ref() {
            let system_prompt = self.deps.system_prompt.as_deref().unwrap_or("");
            budget
                .lock()
                .await
                .note_compaction_effect(messages, system_prompt, budget_tool_tokens);
        }

        // 4. Retry the LLM call once with the summarised history.
        let payload = build_request_payload(
            self.deps.system_prompt.as_deref(),
            self.deps.system_prompt_parts.as_deref(),
            messages,
            tools_ref,
            session_id,
        );
        match self
            .race_llm_call(self.deps.llm.process(payload), parent_cancel, started)
            .await?
        {
            Ok(resp) => {
                self.emit(
                    || crate::harness::trace::LoopTraceEvent::ReactiveCompactionAttempted {
                        token_gap,
                        succeeded: true,
                    },
                );
                Ok(resp)
            }
            Err(retry_err) => {
                tracing::warn!(
                    ?session_id,
                    error = %retry_err,
                    "reactive-compaction retry still failed; surfacing retry error",
                );
                self.emit(
                    || crate::harness::trace::LoopTraceEvent::ReactiveCompactionAttempted {
                        token_gap,
                        succeeded: false,
                    },
                );
                self.set_terminate_reason(
                    crate::orchestrator::dispatch::TerminateReason::ReactiveCompactExhausted,
                );
                Err(HarnessError::Llm(retry_err))
            }
        }
    }

    /// Race a single LLM call against `parent_cancel` and the optional
    /// per-turn timeout. Outer `Result` is harness-fatal; inner is provider.
    /// Used by primary + Stage 5b fallback paths.
    pub(crate) async fn race_llm_call<F>(
        &self,
        fut: F,
        parent_cancel: &CancellationToken,
        started: std::time::Instant,
    ) -> Result<Result<ProviderResponse, crate::error::AlephError>, HarnessError>
    where
        F: std::future::Future<Output = Result<ProviderResponse, crate::error::AlephError>>,
    {
        let fut = std::pin::pin!(fut);
        match self.deps.turn_timeout {
            Some(budget) => tokio::select! {
                biased;
                _ = parent_cancel.cancelled() => Err(HarnessError::Cancelled),
                _ = tokio::time::sleep(budget) => Err(HarnessError::StalledTurn {
                    phase: crate::harness::trait_def::TurnPhase::Think,
                    elapsed: started.elapsed(),
                }),
                r = fut => Ok(r),
            },
            None => tokio::select! {
                biased;
                _ = parent_cancel.cancelled() => Err(HarnessError::Cancelled),
                r = fut => Ok(r),
            },
        }
    }

    /// Primary LLM call with live token streaming.
    ///
    /// When the provider exposes an HTTP delta seam
    /// ([`AiProvider::as_http_provider`]), this drives
    /// [`HttpProvider::execute_streaming`] so each text/thinking delta is
    /// forwarded to `callback` (via [`CallbackSink`]) as it arrives, while the
    /// assembled `ProviderResponse` still runs the full non-streaming pipeline
    /// (cost metering, error promotion, validation, inbound leak scan).
    /// Providers without an HTTP seam (mock providers in tests, non-HTTP
    /// backends) fall back to the one-shot `process()` path — no live deltas,
    /// byte-identical to before.
    ///
    /// Raced against cancel + turn-timeout exactly like [`Self::race_llm_call`].
    /// Caller gates this behind `provider_streams` (HTTP seam present AND no
    /// output guardrail wired) and must suppress the once-per-turn `on_delta`
    /// for streamed turns to avoid double-emitting into append-only consumers.
    ///
    /// [`AiProvider::as_http_provider`]: crate::providers::AiProvider::as_http_provider
    /// [`HttpProvider::execute_streaming`]: crate::providers::http_provider::HttpProvider::execute_streaming
    pub(crate) async fn stream_llm_call(
        &self,
        payload: RequestPayload<'_>,
        callback: &mut dyn HarnessCallback,
        parent_cancel: &CancellationToken,
        started: std::time::Instant,
    ) -> Result<Result<ProviderResponse, crate::error::AlephError>, HarnessError> {
        let Some(http) = self.deps.llm.as_http_provider() else {
            // No HTTP delta seam — keep the existing one-shot path.
            return self
                .race_llm_call(self.deps.llm.process(payload), parent_cancel, started)
                .await;
        };
        let sink = CallbackSink {
            callback: crate::sync_primitives::Mutex::new(callback),
        };
        self.race_llm_call(
            http.execute_streaming(payload, &sink),
            parent_cancel,
            started,
        )
        .await
    }

    /// Fire one tool-less LLM call so the user gets a terminal text
    /// response on a forced termination (budget critical or diminishing
    /// returns). The nudge text is selected by `reason`; the call path is
    /// identical otherwise. Skips entirely when the latest assistant turn
    /// already produced displayable text. Fail-soft on any LLM error —
    /// logs at WARN and returns without persisting.
    ///
    /// Caller is responsible for setting `hit_limit` and returning
    /// `TurnState::Done`; the loop fires `on_complete()` on `Done`, so the
    /// grace paths must not call it themselves.
    async fn fire_grace_turn(
        &self,
        session_id: &SessionId,
        events: &[SessionEventRecord],
        messages: &[UnifiedMessage],
        callback: &mut dyn HarnessCallback,
        iterations: usize,
        reason: GraceReason,
        parent_cancel: &CancellationToken,
    ) {
        if last_assistant_has_text(events) {
            return; // user already has terminal text; skip.
        }
        let mut grace_messages = messages.to_vec();
        grace_messages.push(UnifiedMessage::user(reason.nudge()));
        // Reuse the shared payload builder so the grace call threads the same
        // cache-split `system_blocks` (and `session_id` cache-key metadata) as
        // every other LLM call in the harness. The grace turn fires *right
        // after* a primary turn that already warmed the prompt-cache prefix, so
        // sending the flat string alone re-billed the whole system prompt;
        // threading the parts turns that into a cache hit. No tools — the grace
        // turn salvages terminal text, it must not act.
        let grace_payload = build_request_payload(
            self.deps.system_prompt.as_deref(),
            self.deps.system_prompt_parts.as_deref(),
            &grace_messages,
            None,
            session_id,
        );
        // Race the grace call against cancel + turn-timeout, like every
        // other LLM call in the harness. The grace turn fires precisely
        // when things are already degraded, so a hung provider here must
        // not hang the whole harness or ignore a user cancel.
        let started = std::time::Instant::now();
        let resp = match self
            .race_llm_call(self.deps.llm.process(grace_payload), parent_cancel, started)
            .await
        {
            Ok(Ok(resp)) => resp,
            Ok(Err(e)) => {
                tracing::warn!(
                    ?session_id,
                    ?e,
                    "grace turn LLM call failed; falling through to short-circuit",
                );
                return;
            }
            Err(e) => {
                tracing::warn!(
                    ?session_id,
                    ?e,
                    "grace turn cancelled or timed out; falling through to short-circuit",
                );
                return;
            }
        };
        let text = resp.text_content();
        if text.trim().is_empty() {
            return;
        }
        let turn_id = super::current_turn_id(events);
        callback.on_delta(&text);
        let grace_event = SessionEvent::AssistantMessage {
            turn_id,
            content: MessageContent {
                text: text.clone(),
                blocks: Vec::new(),
                thinking: resp.thinking.clone(),
                thinking_signature: resp.thinking_signature.clone(),
            },
            at: crate::session::events::now_ms(),
        };
        let grace_tokens = super::turn_token_total(&resp.usage);
        self.total_tokens.fetch_add(grace_tokens, Ordering::Relaxed);
        // Keep the per-component breakdown in lockstep with `total_tokens`
        // — the documented `breakdown.total() == total_tokens()` invariant.
        self.accumulate_token_breakdown(&resp.usage);
        if let Err(e) = self.deps.session.emit_event(session_id, grace_event).await {
            tracing::warn!(?session_id, ?e, "grace turn assistant emit failed");
        }
        self.emit(|| crate::harness::trace::LoopTraceEvent::TextEmitted {
            iteration: iterations,
            stream: crate::harness::trace::LoopTraceTextKind::Final,
            text,
        });
    }

    /// Fire a grace turn from the outer loop's cap sites (`max_iterations`,
    /// verifier-veto, consecutive-failure), where the per-turn `events` /
    /// `messages` are no longer in scope. Re-fetches the session log and
    /// re-assembles the prompt, then delegates to
    /// [`AgentHarness::fire_grace_turn`]. Fail-soft: any error logs at WARN
    /// and returns. Skips entirely when the last assistant turn already
    /// produced text — well-behaved capped runs pay nothing.
    pub(crate) async fn fire_boundary_grace_turn(
        &self,
        session_id: &SessionId,
        callback: &mut dyn HarnessCallback,
        iterations: usize,
        reason: GraceReason,
        parent_cancel: &CancellationToken,
    ) {
        let events = match self.deps.session.get_events(session_id, None, None).await {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(?session_id, ?e, "boundary grace turn: get_events failed");
                return;
            }
        };
        if last_assistant_has_text(&events) {
            return; // user already has terminal text; skip.
        }
        let tail_start = super::tail_start_index(&events);
        let messages = super::prompt::build_prompt(&events, tail_start);
        self.fire_grace_turn(
            session_id,
            &events,
            &messages,
            callback,
            iterations,
            reason,
            parent_cancel,
        )
        .await;
    }

    /// Close out `tool_use` blocks the model emitted on a turn the verifier then
    /// vetoed/halted. Those calls are never executed (Act is skipped on a
    /// non-Continue verdict), so without a matching result the prompt builder
    /// drops the orphaned `tool_use` block to avoid an Anthropic 400 — which
    /// silently erases the whole assistant turn (text + remaining context) from
    /// the next prompt. Emitting a synthetic `ToolError` per call preserves the
    /// `tool_use↔result` invariant and lets the model see *why* the call did not
    /// run. Fail-soft: a failed emit logs at WARN and continues.
    async fn close_unexecuted_tool_uses(
        &self,
        session_id: &SessionId,
        turn_id: TurnId,
        call_ids: &[String],
        reason: &str,
    ) {
        for id in call_ids {
            let event = SessionEvent::ToolError {
                turn_id,
                call_id: id.clone(),
                error: format!("call not executed — {reason}"),
                at: crate::session::events::now_ms(),
            };
            if let Err(e) = self.deps.session.emit_event(session_id, event).await {
                tracing::warn!(?session_id, ?e, "close_unexecuted_tool_uses emit failed");
            }
        }
    }

    /// Stage 6a (#10): dispatch the per-turn verifier chain. `None` chain → noop.
    pub(crate) async fn run_verifiers(
        &self,
        iterations: usize,
        tool_calls_made: usize,
        final_text: &str,
        tool_history: &std::collections::VecDeque<ToolCallSummary>,
        stop_reason: Option<&str>,
        session_key: &str,
        cancel: &CancellationToken,
    ) -> VerifierVerdict {
        let Some(chain) = self.deps.verifier_chain.as_ref() else {
            return VerifierVerdict::Continue;
        };
        let snapshot: Vec<ToolCallSummary> = tool_history.iter().cloned().collect();
        let ctx = TurnVerifyContext {
            iterations,
            tool_calls_made,
            final_text: if final_text.is_empty() {
                None
            } else {
                Some(final_text)
            },
            recent_tool_calls: &snapshot,
            stop_reason,
            session_id: Some(session_key),
            robustness_profile: self.deps.robustness_profile,
        };
        chain.verify(&ctx, cancel).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::{now_ms, MessageContent, SessionEvent, SessionEventRecord};

    fn mk(seq: u64, event: SessionEvent) -> SessionEventRecord {
        SessionEventRecord {
            seq,
            event,
            created_at_ms: now_ms(),
        }
    }

    fn assistant(text: &str, with_tool_use: bool) -> SessionEvent {
        let blocks = if with_tool_use {
            vec![serde_json::json!({
                "type": "tool_use",
                "id": "tu",
                "name": "x",
                "input": {},
            })]
        } else {
            Vec::new()
        };
        SessionEvent::AssistantMessage {
            turn_id: uuid::Uuid::new_v4(),
            content: MessageContent {
                text: text.to_string(),
                blocks,
                thinking: None,
                thinking_signature: None,
            },
            at: now_ms(),
        }
    }

    fn user(text: &str) -> SessionEvent {
        SessionEvent::UserMessage {
            turn_id: uuid::Uuid::new_v4(),
            content: MessageContent {
                text: text.to_string(),
                blocks: Vec::new(),
                thinking: None,
                thinking_signature: None,
            },
            at: now_ms(),
            synthetic: false,
        }
    }

    #[test]
    fn last_assistant_has_text_true_for_non_empty_text() {
        let events = vec![
            mk(0, user("hi")),
            mk(1, assistant("here is your answer", false)),
        ];
        assert!(last_assistant_has_text(&events));
    }

    #[test]
    fn last_assistant_has_text_false_for_empty_text_with_tool_use_only() {
        // The rescue path: model wanted to call a tool but was budget-cut,
        // leaving an assistant turn with text="" and only tool_use blocks.
        let events = vec![mk(0, user("hi")), mk(1, assistant("", true))];
        assert!(!last_assistant_has_text(&events));
    }

    #[test]
    fn last_assistant_has_text_false_when_no_assistant_message_at_all() {
        // Budget tripped on turn 1, before the model produced anything.
        let events = vec![mk(0, user("hi"))];
        assert!(!last_assistant_has_text(&events));
    }

    #[test]
    fn last_assistant_has_text_uses_most_recent_assistant() {
        // Older assistant message had text; newer one is empty tool_use —
        // the grace turn must rescue based on the LATEST turn.
        let events = vec![
            mk(0, user("first")),
            mk(1, assistant("old answer", false)),
            mk(2, user("follow up")),
            mk(3, assistant("", true)),
        ];
        assert!(!last_assistant_has_text(&events));
    }

    #[test]
    fn last_assistant_has_text_treats_whitespace_only_text_as_empty() {
        // "  \n\t" alone does not constitute a terminal text response.
        let events = vec![mk(0, user("hi")), mk(1, assistant("   \n\t", false))];
        assert!(!last_assistant_has_text(&events));
    }

    #[test]
    fn grace_reason_budget_uses_budget_nudge() {
        assert_eq!(GraceReason::Budget.nudge(), GRACE_NUDGE_BUDGET);
    }

    #[test]
    fn grace_reason_diminishing_uses_diminishing_nudge() {
        assert_eq!(GraceReason::Diminishing.nudge(), GRACE_NUDGE_DIMINISHING);
    }

    #[test]
    fn grace_nudge_budget_and_diminishing_are_distinct_strings() {
        assert_ne!(GRACE_NUDGE_BUDGET, GRACE_NUDGE_DIMINISHING);
    }

    #[test]
    fn verifier_veto_nudge_is_distinct_and_set() {
        assert_eq!(GraceReason::VerifierVeto.nudge(), GRACE_NUDGE_VERIFIER_VETO);
        assert_ne!(GRACE_NUDGE_VERIFIER_VETO, GRACE_NUDGE_MAX_ITERATIONS);
        assert!(GRACE_NUDGE_VERIFIER_VETO.contains("user"));
    }

    #[test]
    fn consecutive_failure_nudge_is_distinct_and_set() {
        assert_eq!(
            GraceReason::ConsecutiveFailureCap.nudge(),
            GRACE_NUDGE_FAILURE_CAP
        );
        assert_ne!(GRACE_NUDGE_FAILURE_CAP, GRACE_NUDGE_VERIFIER_VETO);
        assert!(GRACE_NUDGE_FAILURE_CAP.contains("user"));
    }

    /// `CallbackSink` must forward text deltas to `on_delta`, thinking deltas
    /// to `on_reasoning`, drop empty deltas, and ignore bookkeeping deltas
    /// (tool-call starts etc. are folded by the provider's collector).
    #[tokio::test]
    async fn callback_sink_forwards_text_and_thinking_deltas() {
        use crate::providers::{DeltaSink, ProviderDelta};

        #[derive(Default)]
        struct RecordingCb {
            text: Vec<String>,
            reasoning: Vec<String>,
        }
        impl crate::harness::callback::HarnessCallback for RecordingCb {
            fn on_delta(&mut self, text: &str) {
                self.text.push(text.to_string());
            }
            fn on_reasoning(&mut self, text: &str) {
                self.reasoning.push(text.to_string());
            }
        }

        let mut cb = RecordingCb::default();
        {
            let sink = CallbackSink {
                callback: crate::sync_primitives::Mutex::new(&mut cb),
            };
            sink.on_delta(&ProviderDelta::TextDelta("Hello ".into()))
                .await;
            sink.on_delta(&ProviderDelta::ThinkingDelta("hmm".into()))
                .await;
            sink.on_delta(&ProviderDelta::TextDelta("world".into()))
                .await;
            // Empty text delta is dropped.
            sink.on_delta(&ProviderDelta::TextDelta(String::new()))
                .await;
            // Bookkeeping delta is ignored by the live preview.
            sink.on_delta(&ProviderDelta::ToolCallStart {
                id: "t1".into(),
                name: "read".into(),
                signature: None,
            })
            .await;
        }
        assert_eq!(cb.text, vec!["Hello ".to_string(), "world".to_string()]);
        assert_eq!(cb.reasoning, vec!["hmm".to_string()]);
    }
}
