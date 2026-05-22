//! Think phase — single-turn LLM call with guardrails, budget checks, and verifier dispatch.

use std::sync::atomic::Ordering;

use tokio_util::sync::CancellationToken;

use super::{AgentHarness, InputGuardrailOutcome};
use crate::context::budget::LoopDirective;
use crate::harness::callback::HarnessCallback;
use crate::harness::trait_def::{HarnessError, TurnState};
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::message::UnifiedMessage;
use crate::session::events::{MessageContent, SessionEvent, SessionEventRecord};
use crate::session::service::SessionId;
use crate::verification::{hash_tool_args, ToolCallSummary, TurnVerifyContext, VerifierVerdict};

/// Ephemeral nudge appended on the grace turn when the budget hits
/// critical — the single tool-less LLM call given when
/// `LoopDirective::FinalReply` fires and the prior assistant turn ended
/// on an unresolved tool_use. Tools are also stripped at the request
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
/// unresolved tool_use leaves the user with no terminal text.
const GRACE_NUDGE_MAX_ITERATIONS: &str =
    "You have reached the maximum number of tool-calling iterations and \
     cannot call any more tools. Respond now with a final summary for the \
     user based on what you have accomplished so far.";

/// Maximum re-issues of the LLM call when the provider returns a response
/// with no text, no tool_calls and no thinking. A small bound — an empty
/// response is usually transient; persistent emptiness is a broken
/// endpoint that more retries will not fix.
const EMPTY_RESPONSE_RETRIES: u32 = 2;

/// Why a grace turn is being fired. Selects the nudge text; otherwise
/// the call path is identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GraceReason {
    /// `LoopDirective::FinalReply` — context-budget critical.
    Budget,
    /// `LoopDirective::StopDiminishing` — diminishing-returns detector trip.
    Diminishing,
    /// `max_iterations` cap reached in the outer loop.
    MaxIterations,
}

impl GraceReason {
    fn nudge(self) -> &'static str {
        match self {
            Self::Budget => GRACE_NUDGE_BUDGET,
            Self::Diminishing => GRACE_NUDGE_DIMINISHING,
            Self::MaxIterations => GRACE_NUDGE_MAX_ITERATIONS,
        }
    }
}

/// True when a provider response carries no usable content at all — no
/// text, no tool_calls and no thinking. This is a provider failure mode
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

impl AgentHarness {
    /// Internal turn execution with pre-computed counters to avoid O(n²)
    /// event-log scans in the outer loop.
    ///
    /// Returns `(TurnState, tool_calls_executed, is_verifier_veto, split_child_session_id)`.
    /// The 4th element is `Some(child)` only when a `SplitSession` directive
    /// succeeded; `None` in all other cases.
    pub(crate) async fn run_turn_internal(
        &self,
        session_id: &SessionId,
        callback: &mut dyn HarnessCallback,
        iterations: usize,
        tool_calls_made: usize,
        tool_history: &mut std::collections::VecDeque<ToolCallSummary>,
        parent_cancel: &CancellationToken,
    ) -> Result<(TurnState, usize, bool, Option<SessionId>), HarnessError> {
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
                        return Ok((TurnState::Done, 0, false, None));
                    }
                }
            } else {
                events
            };

        // 2. Build the LLM request. `prompt_builder` has access to the full log
        //    so it can reconstruct the preceding assistant tool_use turn and
        //    resolve tool names for tool_result messages.
        let ctx = crate::harness::prompt::TurnContext::new(&events, tail_start);
        let mut messages = self.deps.prompt_builder.assemble(&ctx).await?;

        // 2a. Preflight cheap passes (hermes-inspired). Run BEFORE the budget
        // check so token-saving transforms — tool_result pruning + historical
        // image stripping — happen unconditionally, even if the compactor's
        // side-channel LLM call later fails. No LLM cost in this step.
        if let Some(pipeline) = self.deps.preflight_pipeline.as_ref() {
            // Stages mostly key off `fresh_tail_count`; the pressure ratio is
            // not currently consulted by the two shipped stages. Pass a max-
            // pressure placeholder so any future pressure-gated stage still
            // fires when wired in.
            let placeholder_pressure = crate::context::budget::ContextPressure {
                used_tokens: 0,
                budget_tokens: 0,
                ratio: 1.0,
                overhead_tokens: 0,
                available_for_messages: 0,
            };
            // Mirror the compactor's fresh_tail default when no explicit
            // config is in scope; 6 matches `CompactorConfig::default`.
            let fresh_tail = 6usize;
            let freed = pipeline
                .run(&mut messages, &placeholder_pressure, fresh_tail)
                .await;
            if freed > 0 {
                tracing::debug!(
                    ?session_id,
                    tokens_freed = freed,
                    "preflight cheap passes saved tokens",
                );
            }
        }

        // Fetch the cached dispatcher-form tool schema once. O(1) `Arc::clone`
        // on the steady-state path. Hoisted above the budget check so the
        // pressure sensor accounts for the real tool-schema overhead.
        let dispatcher_tools = self.deps.tools.dispatcher_schema();

        // 2b. Task-10 budget check: evaluate context pressure before issuing
        // the LLM call. The sensor now sees the real system prompt and
        // tool-schema overhead (previously passed empty), so compaction and
        // `FinalReply` fire on the true context size, not just message tokens.
        let (budget_directive, budget_tool_tokens) =
            if let Some(budget) = self.deps.context_budget.as_ref() {
                let mut guard = budget.lock().await;
                let tool_tokens =
                    estimate_tool_schema_tokens(&dispatcher_tools, guard.token_estimate_ratio());
                let system_prompt = self.deps.system_prompt.as_deref().unwrap_or("");
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
                return Ok((TurnState::Continue, 0, false, Some(child)));
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
            return Ok((TurnState::Done, 0, false, None));
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
            return Ok((TurnState::Done, 0, false, None));
        }

        // 2d. Derive the optional tool-schema reference for the request payload
        // from the dispatcher tools fetched above (Stage 2). Cache invalidation
        // is owned by `ToolService` impls; see `to_dispatcher_form`.
        let tools_ref: Option<&[crate::tool_metadata::ToolDefinition]> =
            if dispatcher_tools.is_empty() {
                None
            } else {
                Some(dispatcher_tools.as_ref())
            };

        // Build the request fresh on each call — H3's empty-response retry
        // re-issues it, and `RequestPayload` is a cheap borrow of `messages`.
        let build_payload = || match self.deps.system_prompt.as_deref() {
            Some(sp) => RequestPayload::new(&messages)
                .with_system(Some(sp))
                .with_tools(tools_ref),
            None => RequestPayload::new(&messages).with_tools(tools_ref),
        };

        self.emit(|| crate::harness::trace::LoopTraceEvent::TurnStateEntered {
            iteration: iterations,
            state: crate::harness::trace::LoopTraceState::Think,
        });

        // 3. Call the LLM, racing against cancel + turn-timeout. Provider-tier
        // failover — the ordered chain, model-level fallback, and the circuit
        // breaker — lives inside `deps.llm` itself (`providers::FailoverProvider`),
        // so the harness simply propagates whatever error survives it.
        let started = std::time::Instant::now();
        let mut response = match self
            .race_llm_call(
                self.deps.llm.process(build_payload()),
                parent_cancel,
                started,
            )
            .await?
        {
            Ok(r) => r,
            Err(primary_err) => return Err(HarnessError::Llm(primary_err)),
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
            tracing::warn!(
                ?session_id,
                empty_retries,
                "provider returned an empty response; retrying",
            );
            response = match self
                .race_llm_call(
                    self.deps.llm.process(build_payload()),
                    parent_cancel,
                    started,
                )
                .await?
            {
                Ok(r) => r,
                Err(primary_err) => return Err(HarnessError::Llm(primary_err)),
            };
        }
        if is_empty_response(&response) {
            self.set_terminate_reason(
                crate::orchestrator::dispatch::TerminateReason::EmptyResponseExhausted,
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
            .map(|u| u.output_tokens as usize)
            .unwrap_or(0);

        // 4. Emit AssistantMessage preserving any tool_use intent in `blocks`.
        let turn_id = super::current_turn_id(&events);
        let text = response.text_content();

        // 4a. Stage 5a (#9): Output guardrail. `Block` aborts with
        // `HarnessError::Llm(ErrorClass::Fixable)` so the orchestrator can
        // retry; `Sanitize` rewrites the text before persistence and stream.
        // Tool-use blocks are not rewritten here — Stage 5b's
        // `ToolCallGuardrail` covers their args.
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
                crate::guardrails::GuardrailDecision::Block { reason, class: _ } => {
                    callback.on_safety_block(&reason);
                    return Err(HarnessError::Llm(crate::error::AlephError::other(format!(
                        "output guardrail blocked: {reason}"
                    ))));
                }
            }
        } else {
            text
        };

        if !text.is_empty() {
            // Non-streaming LLM layer emits one chunk per turn; the callback
            // shape permits finer chunking once `process_stream` is wired.
            callback.on_delta(&text);
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
            if tool_history.len() == 8 {
                tool_history.pop_front();
            }
            tool_history.push_back(ToolCallSummary {
                name: tc.name.clone(),
                args_hash: hash_tool_args(&tc.arguments),
            });
        }
        let stop_reason = response.tool_calls.is_empty().then_some("end_turn");
        let verdict = self
            .run_verifiers(
                iterations,
                tool_calls_made,
                &text,
                tool_history,
                stop_reason,
                parent_cancel,
            )
            .await;
        let zero_metrics = crate::harness::trace::LoopTraceTurnMetrics {
            requested_tool_calls: 0,
            executed_tool_calls: 0,
            productive: false,
            consecutive_errors: 0,
            total_tokens: turn_tokens as usize,
        };
        let outcome_for_trace;
        let metrics_for_trace;
        let result;
        if let VerifierVerdict::Veto { reason, .. } = verdict {
            tracing::info!(?session_id, reason = %reason, "verifier vetoed; forcing continue");
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
            };
            self.deps
                .session
                .emit_event(session_id, block_event)
                .await?;
            outcome_for_trace = crate::harness::trace::LoopTraceTurnOutcome::Continue;
            metrics_for_trace = zero_metrics;
            result = Ok((TurnState::Continue, 0, true, None));
        } else if response.tool_calls.is_empty() {
            outcome_for_trace = crate::harness::trace::LoopTraceTurnOutcome::Stop;
            metrics_for_trace = zero_metrics;
            result = Ok((TurnState::Done, 0, false, None));
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
            result = Ok((TurnState::Continue, executed, false, None));
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
        let is_verifier_veto = matches!(result, Ok((_, _, true, _)));
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
                return Ok((
                    TurnState::Done,
                    metrics_for_trace.executed_tool_calls,
                    false,
                    None,
                ));
            }
        }

        self.emit(|| crate::harness::trace::LoopTraceEvent::TurnCompleted {
            iteration: iterations,
            outcome: outcome_for_trace,
            metrics: metrics_for_trace,
        });
        result
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
        let grace_payload = match self.deps.system_prompt.as_deref() {
            Some(sp) => RequestPayload::new(&grace_messages).with_system(Some(sp)),
            None => RequestPayload::new(&grace_messages),
        };
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

    /// Fire a grace turn from the outer loop's `max_iterations` cap site,
    /// where the per-turn `events` / `messages` are no longer in scope.
    /// Re-fetches the session log and re-assembles the prompt, then
    /// delegates to [`AgentHarness::fire_grace_turn`]. Fail-soft: any error
    /// logs at WARN and returns. Skips entirely when the last assistant
    /// turn already produced text — well-behaved capped runs pay nothing.
    pub(crate) async fn fire_max_iterations_grace_turn(
        &self,
        session_id: &SessionId,
        callback: &mut dyn HarnessCallback,
        iterations: usize,
        parent_cancel: &CancellationToken,
    ) {
        let events = match self.deps.session.get_events(session_id, None, None).await {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(?session_id, ?e, "max-iter grace turn: get_events failed");
                return;
            }
        };
        if last_assistant_has_text(&events) {
            return; // user already has terminal text; skip.
        }
        let tail_start = super::tail_start_index(&events);
        let ctx = crate::harness::prompt::TurnContext::new(&events, tail_start);
        let messages = match self.deps.prompt_builder.assemble(&ctx).await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    ?session_id,
                    ?e,
                    "max-iter grace turn: prompt assembly failed"
                );
                return;
            }
        };
        self.fire_grace_turn(
            session_id,
            &events,
            &messages,
            callback,
            iterations,
            GraceReason::MaxIterations,
            parent_cancel,
        )
        .await;
    }

    /// Stage 6a (#10): dispatch the per-turn verifier chain. `None` chain → noop.
    pub(crate) async fn run_verifiers(
        &self,
        iterations: usize,
        tool_calls_made: usize,
        final_text: &str,
        tool_history: &std::collections::VecDeque<ToolCallSummary>,
        stop_reason: Option<&str>,
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
}
