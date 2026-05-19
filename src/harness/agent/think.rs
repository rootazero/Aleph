//! Think phase — single-turn LLM call with guardrails, budget checks, and verifier dispatch.

use std::sync::atomic::Ordering;

use tokio_util::sync::CancellationToken;

use super::{AgentHarness, HarnessCallbackExt, InputGuardrailOutcome};
use crate::context::budget::LoopDirective;
use crate::harness::callback::HarnessCallback;
use crate::harness::trait_def::{HarnessError, TurnState};
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::session::events::{MessageContent, SessionEvent, ToolOutput};
use crate::session::service::SessionId;
use crate::verification::{hash_tool_args, ToolCallSummary, TurnVerifyContext, VerifierVerdict};

impl AgentHarness {
    /// Internal turn execution with pre-computed counters to avoid O(n²)
    /// event-log scans in the outer loop.
    ///
    /// Returns `(TurnState, tool_calls_executed, is_verifier_veto)`.
    pub(crate) async fn run_turn_internal(
        &self,
        session_id: &SessionId,
        callback: &mut dyn HarnessCallback,
        iterations: usize,
        tool_calls_made: usize,
        tool_history: &mut std::collections::VecDeque<ToolCallSummary>,
        tool_call_cache: &mut std::collections::HashMap<(String, String), ToolOutput>,
        parent_cancel: &CancellationToken,
    ) -> Result<(TurnState, usize, bool), HarnessError> {
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
                        return Ok((TurnState::Done, 0, false));
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

        // 2a. Task-10 budget check: evaluate context pressure before issuing
        // the LLM call. `FinalReply` forces a terminal turn with no tools.
        let budget_directive = if let Some(budget) = self.deps.context_budget.as_ref() {
            let mut guard = budget.lock().await;
            Some(guard.before_turn(&messages, "", &[]))
        } else {
            None
        };

        // 2b. Compact when directive calls for it and a compactor is wired.
        if matches!(budget_directive, Some(LoopDirective::CompactAndContinue)) {
            if let Some(compactor) = self.deps.context_compactor.as_ref() {
                // `fresh_tail = 0` lets the compactor fall back to its own
                // config default (matches Task 6 spec).
                if let Err(e) = compactor.compact(&mut messages, 0, None).await {
                    tracing::warn!(
                        ?session_id,
                        ?e,
                        "context compactor failed; continuing with uncompacted messages",
                    );
                }
            }
        }

        // 2c. `FinalReply` directive — record hit_limit and short-circuit to
        // Done without calling the LLM or running tools. The last assistant
        // message already on the session log is the final text.
        if matches!(budget_directive, Some(LoopDirective::FinalReply)) {
            self.hit_limit.store(true, Ordering::Relaxed);
            callback.on_complete_via_harness();
            return Ok((TurnState::Done, 0, false));
        }

        // 2d. Fetch the cached dispatcher-form tool schema. This is an O(1)
        // `Arc::clone` on the steady-state path (Stage 2). Cache invalidation
        // is owned by `ToolService` impls; see `to_dispatcher_form`.
        let dispatcher_tools = self.deps.tools.dispatcher_schema();
        let tools_ref: Option<&[crate::dispatcher::ToolDefinition]> = if dispatcher_tools.is_empty()
        {
            None
        } else {
            Some(dispatcher_tools.as_ref())
        };

        let payload = match self.deps.system_prompt.as_deref() {
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
        let response = match self
            .race_llm_call(self.deps.llm.process(payload), parent_cancel, started)
            .await?
        {
            Ok(r) => r,
            Err(primary_err) => return Err(HarnessError::Llm(primary_err)),
        };

        // Accumulate this turn's provider-reported token usage. Counted here
        // — right after the LLM call — so a turn whose output is later
        // blocked by a guardrail still reflects the tokens the provider
        // billed. Excludes `thinking_tokens`; see `turn_token_total`.
        let turn_tokens = super::turn_token_total(&response.usage);
        self.total_tokens.fetch_add(turn_tokens, Ordering::Relaxed);

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
            result = Ok((TurnState::Continue, 0, true));
        } else if response.tool_calls.is_empty() {
            outcome_for_trace = crate::harness::trace::LoopTraceTurnOutcome::Stop;
            metrics_for_trace = zero_metrics;
            result = Ok((TurnState::Done, 0, false));
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
                    tool_call_cache,
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
            result = Ok((TurnState::Continue, executed, false));
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
