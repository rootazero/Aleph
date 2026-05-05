//! AgentHarness — the concrete Think→Act implementation.
//!
//! Task 8 implemented the Think half of the loop. Task 9 added:
//!   * Act dispatch (executing tool_calls sequentially, emitting ToolResult /
//!     ToolError events).
//!   * Preservation of assistant `tool_use` intent inside `AssistantMessage`
//!     events so later Think cycles can reconstruct the conversation.
//!   * Full-history prompt assembly (now in `prompt.rs`) that re-emits the
//!     preceding assistant tool_use turn and resolves real tool names for
//!     `ToolResult` messages.
//!
//! Task 10 (Phase 6b) additionally consumes the optional triad on
//! `HarnessDeps`:
//!   * `context_budget.before_turn(...)` — drives compaction / hit_limit.
//!   * `context_compactor.compact(...)` — fires when budget directs warning.
//!   * `stop_hooks` — consulted before an early `TurnState::Done` handoff;
//!     a blocking verdict forces one more `Continue` so the model reacts.

use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::context::budget::LoopDirective;
use crate::harness::callback::{HarnessCallback, NoopHarnessCallback};
use crate::harness::deps::HarnessDeps;
use crate::harness::trait_def::{Harness, HarnessError, TurnPhase, TurnState};
use crate::providers::adapter::{NativeToolCall, RequestPayload};

use crate::session::events::{
    now_ms, MessageContent, SessionEvent, SessionEventRecord, ToolOutput, TurnId,
};
use crate::session::service::SessionId;
use crate::verification::stop_hooks::{execute_stop_hooks, StopHookContext, StopHookHandler};

pub struct AgentHarness {
    deps: HarnessDeps,
    /// Tracks agent activity for stall detection. `None` when stall detection
    /// is disabled (no `stall_config` in deps).
    stall_tracker: Option<crate::harness::deps::StallTracker>,
    /// Set when `context_budget.before_turn` returns `FinalReply`. Surfaced
    /// through [`AgentHarness::hit_limit`] so the orchestrator bridge can
    /// populate `FlowOutcome::hit_limit`.
    hit_limit: AtomicBool,
}

impl AgentHarness {
    pub fn new(deps: HarnessDeps) -> Self {
        let stall_tracker = deps.stall_config.as_ref().map(|config| {
            crate::harness::deps::StallTracker::new(
                config.clone(),
                tokio_util::sync::CancellationToken::new(),
            )
        });
        Self {
            deps,
            stall_tracker,
            hit_limit: AtomicBool::new(false),
        }
    }

    /// `true` if a budget directive forced an early exit during this run.
    /// Cleared by [`AgentHarness::reset_hit_limit`] before a fresh run.
    pub fn hit_limit(&self) -> bool {
        self.hit_limit.load(Ordering::Relaxed)
    }

    /// Reset the hit_limit flag. Called before a fresh session drive so a
    /// previous run's budget trip does not leak into the next outcome.
    pub fn reset_hit_limit(&self) {
        self.hit_limit.store(false, Ordering::Relaxed);
    }

    /// Read-only accessor for this harness's position in the subagent chain.
    /// Returns the root context for top-level agents (the `HarnessDeps`
    /// default). The subagent spawner overrides this with the descended
    /// chain when assembling a child harness. Stage 4 seam (#11).
    pub fn chain_context(&self) -> &crate::harness::chain_context::ChainContext {
        &self.deps.chain_context
    }

    /// Convenience: wrap this harness as an `Arc<dyn SessionDriver>` so it
    /// can be stored in containers that don't depend on the concrete type.
    pub fn into_session_driver(self) -> std::sync::Arc<dyn crate::session::SessionDriver> {
        std::sync::Arc::new(self)
    }

    /// Max consecutive stop-hook vetos before the harness gives up and
    /// forces Done. Prevents infinite loops when a hook permanently blocks.
    const MAX_STOP_HOOK_VETOS: usize = 10;

    /// Lazy-construct a `LoopTraceEvent` and forward to `trace_sink`.
    /// Returns immediately when no sink is wired — the closure is not invoked.
    fn emit<F>(&self, build: F)
    where
        F: FnOnce() -> crate::harness::trace::LoopTraceEvent,
    {
        if let Some(ref sink) = self.deps.trace_sink {
            sink.on_trace(&build());
        }
    }

    /// Internal turn execution with pre-computed counters to avoid O(n²)
    /// event-log scans in the outer loop.
    ///
    /// Returns `(TurnState, tool_calls_executed, is_stop_hook_veto)`.
    async fn run_turn_internal(
        &self,
        session_id: &SessionId,
        callback: &mut dyn HarnessCallback,
        iterations: usize,
        tool_calls_made: usize,
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

        // Kick off a throttled skill prefetch scan before the LLM call. The
        // scan runs in a background task; its result surfaces on the next
        // turn rather than blocking this one.
        if let Some(prefetcher) = self.deps.skill_prefetcher.as_ref() {
            let _ = prefetcher.start_scan();
        }

        // 1. Fetch full event log and compute the tail boundary.
        let events = self.deps.session.get_events(session_id, None, None).await?;
        let tail_start = tail_start_index(&events);

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

        // 3. Call the LLM, racing against parent cancel and per-turn timeout.
        let response = {
            let llm_fut = self.deps.llm.process(payload);
            let started = std::time::Instant::now();
            match self.deps.turn_timeout {
                Some(budget) => {
                    tokio::select! {
                        biased;
                        _ = parent_cancel.cancelled() => {
                            return Err(HarnessError::Cancelled);
                        }
                        _ = tokio::time::sleep(budget) => {
                            return Err(HarnessError::StalledTurn {
                                phase: TurnPhase::Think,
                                elapsed: started.elapsed(),
                            });
                        }
                        r = llm_fut => match r {
                            Ok(r) => r,
                            Err(e) => return Err(HarnessError::Llm(e)),
                        },
                    }
                }
                None => {
                    tokio::select! {
                        biased;
                        _ = parent_cancel.cancelled() => {
                            return Err(HarnessError::Cancelled);
                        }
                        r = llm_fut => match r {
                            Ok(r) => r,
                            Err(e) => return Err(HarnessError::Llm(e)),
                        },
                    }
                }
            }
        };

        // 4. Emit AssistantMessage preserving any tool_use intent in `blocks`.
        let turn_id = current_turn_id(&events);
        let text = response.text_content();
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
        let blocks = tool_use_blocks(&response.tool_calls);
        let assistant_event = SessionEvent::AssistantMessage {
            turn_id,
            content: MessageContent {
                text: text.clone(),
                blocks,
                thinking: response.thinking.clone(),
                thinking_signature: response.thinking_signature.clone(),
            },
            at: now_ms(),
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

        // 5. If the LLM produced tool_calls, run the Act phase; otherwise
        //    evaluate stop hooks before declaring Done.
        let outcome_for_trace;
        let metrics_for_trace;
        let result;

        if response.tool_calls.is_empty() {
            let block = self
                .evaluate_stop_hooks(iterations, tool_calls_made, Some(text.clone()))
                .await;
            if let Some(reason) = block {
                tracing::info!(
                    ?session_id,
                    reason = %reason,
                    "stop hook vetoed; forcing continue",
                );
                let new_turn = uuid::Uuid::new_v4();
                let block_event = SessionEvent::UserMessage {
                    turn_id: new_turn,
                    content: MessageContent {
                        text: format!("[stop-hook veto] {reason}"),
                        blocks: Vec::new(),
                        thinking: None,
                        thinking_signature: None,
                    },
                    at: now_ms(),
                };
                self.deps
                    .session
                    .emit_event(session_id, block_event)
                    .await?;
                outcome_for_trace = crate::harness::trace::LoopTraceTurnOutcome::Continue;
                metrics_for_trace = crate::harness::trace::LoopTraceTurnMetrics {
                    requested_tool_calls: 0,
                    executed_tool_calls: 0,
                    productive: false,
                    consecutive_errors: 0,
                    total_tokens: 0,
                };
                result = Ok((TurnState::Continue, 0, true));
            } else {
                outcome_for_trace = crate::harness::trace::LoopTraceTurnOutcome::Stop;
                metrics_for_trace = crate::harness::trace::LoopTraceTurnMetrics {
                    requested_tool_calls: 0,
                    executed_tool_calls: 0,
                    productive: false,
                    consecutive_errors: 0,
                    total_tokens: 0,
                };
                result = Ok((TurnState::Done, 0, false));
            }
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
                total_tokens: 0,
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

    /// Act phase: execute each tool_call sequentially, emitting a
    /// `ToolCallRequested` event before every call and either a `ToolResult`
    /// or `ToolError` event after.
    ///
    /// Tool failures are persisted as `SessionEvent::ToolError` and do NOT
    /// abort the batch — all tool calls in the batch are attempted. The next
    /// Think turn will see failures via `tool_result.is_error=true` (produced
    /// by the prompt assembler) and can decide whether to retry or give up.
    ///
    /// Returns the number of tool calls that succeeded (not errored).
    async fn act(
        &self,
        session_id: &SessionId,
        turn_id: TurnId,
        tool_calls: Vec<NativeToolCall>,
        callback: &mut dyn HarnessCallback,
        iteration: usize,
    ) -> Result<usize, HarnessError> {
        let mut executed_count: usize = 0;

        for call in tool_calls {
            callback.on_tool_call(&call.name);
            let started = std::time::Instant::now();
            self.emit(|| crate::harness::trace::LoopTraceEvent::ToolCallStarted {
                iteration,
                call: crate::harness::trace::ToolCallStartEvent {
                    tool_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    input: call.arguments.clone(),
                },
            });
            let requested = SessionEvent::ToolCallRequested {
                turn_id,
                call_id: call.id.clone(),
                name: call.name.clone(),
                input: call.arguments.clone(),
                at: now_ms(),
            };
            self.deps.session.emit_event(session_id, requested).await?;

            let exec_fut = self.deps.tools.execute(&call.name, call.arguments.clone());
            let exec_result: Result<
                Result<ToolOutput, crate::tools::service::ToolError>,
                HarnessError,
            > = match self.deps.turn_timeout {
                Some(budget) => {
                    let started_call = std::time::Instant::now();
                    match tokio::time::timeout(budget, exec_fut).await {
                        Ok(inner) => Ok(inner),
                        Err(_) => Err(HarnessError::StalledTurn {
                            phase: TurnPhase::Act {
                                tool_name: call.name.clone(),
                            },
                            elapsed: started_call.elapsed(),
                        }),
                    }
                }
                None => Ok(exec_fut.await),
            };
            let inner = match exec_result {
                Ok(r) => r,
                Err(stalled) => return Err(stalled),
            };
            match inner {
                Ok(output) => {
                    executed_count = executed_count.saturating_add(1);
                    let output_value = output.value.clone();
                    let result_event = SessionEvent::ToolResult {
                        turn_id,
                        call_id: call.id.clone(),
                        output,
                        at: now_ms(),
                    };
                    self.deps
                        .session
                        .emit_event(session_id, result_event)
                        .await?;
                    self.emit(
                        || crate::harness::trace::LoopTraceEvent::ToolCallCompleted {
                            iteration,
                            call: crate::harness::trace::ToolCallEndEvent {
                                tool_id: call.id.clone(),
                                tool_name: call.name.clone(),
                                input: call.arguments.clone(),
                                duration_ms: started.elapsed().as_millis() as u64,
                            },
                            result: crate::tools::runtime::ToolResult::Success {
                                output: output_value,
                            },
                        },
                    );
                }
                Err(e) => {
                    let error_msg = e.to_string();
                    let error_event = SessionEvent::ToolError {
                        turn_id,
                        call_id: call.id.clone(),
                        error: error_msg.clone(),
                        at: now_ms(),
                    };
                    if let Err(emit_err) =
                        self.deps.session.emit_event(session_id, error_event).await
                    {
                        tracing::warn!(
                            ?session_id,
                            call_id = %call.id,
                            ?emit_err,
                            "failed to persist ToolError event",
                        );
                    }
                    self.emit(
                        || crate::harness::trace::LoopTraceEvent::ToolCallCompleted {
                            iteration,
                            call: crate::harness::trace::ToolCallEndEvent {
                                tool_id: call.id.clone(),
                                tool_name: call.name.clone(),
                                input: call.arguments.clone(),
                                duration_ms: started.elapsed().as_millis() as u64,
                            },
                            result: crate::tools::runtime::ToolResult::Error {
                                error: error_msg,
                                retryable: false,
                            },
                        },
                    );
                    // Do NOT abort — continue processing remaining tool calls.
                    // The error is persisted to session log; the next Think
                    // turn will see it as tool_result(is_error=true).
                }
            }

            // Record activity after each tool execution completes so the stall
            // tracker is reset for each progress event.
            if let Some(ref tracker) = self.stall_tracker {
                tracker.record_activity().await;
            }
        }

        Ok(executed_count)
    }

    /// Evaluate stop hooks and return the blocking reason if any hook
    /// vetoes the stop. Returns `None` when `stop_hooks` is unset, empty,
    /// or when every hook allows the stop.
    async fn evaluate_stop_hooks(
        &self,
        iterations: usize,
        tool_calls_made: usize,
        final_text: Option<String>,
    ) -> Option<String> {
        let hooks = self.deps.stop_hooks.as_ref()?;
        if hooks.is_empty() {
            return None;
        }
        // `execute_stop_hooks` wants `&[Box<dyn StopHookHandler>]`; we hold
        // `Arc`s for shareability. Adapt by wrapping each Arc in a forwarding
        // Box — avoids cloning the hook implementations.
        struct ArcHook(std::sync::Arc<dyn StopHookHandler>);
        #[async_trait::async_trait]
        impl StopHookHandler for ArcHook {
            fn name(&self) -> &str {
                self.0.name()
            }
            async fn evaluate(
                &self,
                ctx: &StopHookContext,
                cancel: &CancellationToken,
            ) -> crate::verification::stop_hooks::StopHookVerdict {
                self.0.evaluate(ctx, cancel).await
            }
        }
        let boxed: Vec<Box<dyn StopHookHandler>> = hooks
            .iter()
            .map(|h| Box::new(ArcHook(h.clone())) as Box<dyn StopHookHandler>)
            .collect();
        let ctx = StopHookContext {
            final_text,
            iterations,
            tool_calls_made,
            stop_reason: "end_turn".into(),
        };
        let cancel = CancellationToken::new();
        let result = execute_stop_hooks(&boxed, &ctx, &cancel).await;
        result.blocking_reason().map(|s| s.to_string())
    }
}

#[async_trait]
impl crate::session::SessionDriver for AgentHarness {
    async fn drive(&self, session_id: &SessionId) -> crate::error::Result<()> {
        // Preserve `HarnessError` variant semantics when lifting into the
        // crate-wide `AlephError`:
        //   * `Cancelled` → `AlephError::Cancelled` (downstream UI branches
        //     on this to render "Operation cancelled." rather than a
        //     provider-failure banner).
        //   * `Llm(inner)` → unwrap: `inner` is already an `AlephError`, so
        //     re-wrapping would hide the structured `AuthenticationError`
        //     / `NetworkError` / etc. from callers that `match` on it.
        //   * `Tool` / `Session` → no better variant exists on `AlephError`;
        //     stringify through `provider` with a discriminating prefix.
        // Exhaustive match (no wildcard) so new `HarnessError` variants
        // force a review here.
        let mut cb = NoopHarnessCallback;
        // SessionDriver path has no external cancel source (legacy entry
        // point); construct a never-cancelled token so the Harness loop
        // behaves identically to pre-Task-5 runs.
        let cancel = tokio_util::sync::CancellationToken::new();
        self.run(session_id, &mut cb, &cancel)
            .await
            .map_err(|e| match e {
                HarnessError::Cancelled => crate::error::AlephError::Cancelled,
                HarnessError::Llm(inner) => inner,
                HarnessError::Tool(tool_err) => {
                    crate::error::AlephError::provider(format!("harness tool error: {tool_err}"))
                }
                HarnessError::Session(sess_err) => {
                    crate::error::AlephError::provider(format!("harness session error: {sess_err}"))
                }
                HarnessError::Stalled { elapsed } => {
                    crate::error::AlephError::provider(format!("agent stalled after {:?}", elapsed))
                }
                HarnessError::StalledTurn { phase, elapsed } => crate::error::AlephError::provider(
                    format!("agent turn stalled in {phase} after {elapsed:?}"),
                ),
            })
    }
}

#[async_trait]
impl Harness for AgentHarness {
    /// Overrides the trait default to surface this harness's chain position.
    /// Stage 4 seam (#11) — see `AgentHarness::chain_context` (inherent).
    fn chain_context(&self) -> Option<&crate::harness::chain_context::ChainContext> {
        Some(self.chain_context())
    }

    /// Overrides the trait default to enforce `HarnessDeps.max_iterations`.
    /// When the cap is reached, sets `hit_limit=true`, fires `on_complete`,
    /// and returns `Ok(())` — the orchestrator bridge promotes `hit_limit`
    /// into `FlowOutcome::hit_limit`. `None` falls through to the unbounded
    /// default used by the Gateway path.
    async fn run(
        &self,
        session_id: &SessionId,
        callback: &mut dyn HarnessCallback,
        cancel: &CancellationToken,
    ) -> Result<(), HarnessError> {
        let cap = self.deps.max_iterations;
        let mut iterations: usize = 0;
        let mut tool_calls_made: usize = 0;
        let mut stop_hook_veto_count: usize = 0;
        let mut consecutive_failure_turns: usize = 0;
        let result: Result<crate::harness::trace::LoopTraceSessionOutcome, HarnessError> = loop {
            if cancel.is_cancelled() {
                break Err(HarnessError::Cancelled);
            }
            if let Some(ref tracker) = self.stall_tracker {
                if tracker.is_stalled() {
                    let elapsed = tracker.elapsed().await;
                    break Err(HarnessError::Stalled { elapsed });
                }
            }
            match self
                .run_turn_internal(session_id, callback, iterations, tool_calls_made, cancel)
                .await
            {
                Err(e) => break Err(e),
                Ok((TurnState::Continue, executed, is_veto)) => {
                    if let Some(ref tracker) = self.stall_tracker {
                        tracker.record_activity().await;
                    }
                    iterations = iterations.saturating_add(1);
                    tool_calls_made = tool_calls_made.saturating_add(executed);
                    if executed == 0 && !is_veto {
                        let events = self
                            .deps
                            .session
                            .get_events(session_id, None, None)
                            .await
                            .map_err(HarnessError::Session)?;
                        let last_assistant_idx = events
                            .iter()
                            .rposition(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
                            .unwrap_or(0);
                        let had_failure = events[last_assistant_idx..]
                            .iter()
                            .any(|r| matches!(r.event, SessionEvent::ToolError { .. }));
                        if had_failure {
                            consecutive_failure_turns = consecutive_failure_turns.saturating_add(1);
                            if let Some(cap) = self.deps.consecutive_failure_cap {
                                if consecutive_failure_turns >= cap {
                                    tracing::warn!(
                                        ?session_id,
                                        cap,
                                        "consecutive total-failure cap reached; forcing Done",
                                    );
                                    self.hit_limit.store(true, Ordering::Relaxed);
                                    callback.on_complete();
                                    break Ok(
                                        crate::harness::trace::LoopTraceSessionOutcome::HitLimit,
                                    );
                                }
                            }
                        } else {
                            consecutive_failure_turns = 0;
                        }
                    } else if executed > 0 {
                        consecutive_failure_turns = 0;
                    }
                    if is_veto {
                        stop_hook_veto_count = stop_hook_veto_count.saturating_add(1);
                        if stop_hook_veto_count >= Self::MAX_STOP_HOOK_VETOS {
                            tracing::warn!(
                                ?session_id,
                                max_vetos = Self::MAX_STOP_HOOK_VETOS,
                                "stop-hook veto limit reached; forcing Done to prevent infinite loop",
                            );
                            callback.on_complete();
                            break Ok(crate::harness::trace::LoopTraceSessionOutcome::HitLimit);
                        }
                    } else {
                        stop_hook_veto_count = 0;
                    }
                    if let Some(limit) = cap {
                        if iterations >= limit {
                            self.hit_limit.store(true, Ordering::Relaxed);
                            callback.on_complete();
                            break Ok(crate::harness::trace::LoopTraceSessionOutcome::HitLimit);
                        }
                    }
                }
                Ok((TurnState::Done, _, _)) => {
                    callback.on_complete();
                    break Ok(crate::harness::trace::LoopTraceSessionOutcome::Completed);
                }
            }
        };

        match result {
            Ok(outcome) => {
                self.emit(|| crate::harness::trace::LoopTraceEvent::SessionCompleted {
                    outcome,
                    iterations,
                    tool_calls_made,
                    total_tokens: 0,
                    hit_limit: matches!(
                        outcome,
                        crate::harness::trace::LoopTraceSessionOutcome::HitLimit,
                    ),
                    final_text: None,
                });
                Ok(())
            }
            Err(e) => {
                let error_class = e.class();
                tracing::warn!(
                    ?session_id,
                    ?error_class,
                    error = %e,
                    "harness session ended in error",
                );
                // Stage 1: all classes still map to Cancelled (preserves P0
                // rescue behavior). Future stages will distinguish (e.g.
                // Stage 6 Verification may map Unexpected -> Failed). The
                // match is exhaustive without a wildcard so any new
                // ErrorClass variant forces an explicit decision here.
                #[allow(clippy::match_same_arms)]
                let session_outcome = match error_class {
                    crate::error::ErrorClass::Recoverable
                    | crate::error::ErrorClass::Transient
                    | crate::error::ErrorClass::Fixable
                    | crate::error::ErrorClass::Unexpected => {
                        crate::harness::trace::LoopTraceSessionOutcome::Cancelled
                    }
                };
                self.emit(|| crate::harness::trace::LoopTraceEvent::SessionCompleted {
                    outcome: session_outcome,
                    iterations,
                    tool_calls_made,
                    total_tokens: 0,
                    hit_limit: false,
                    final_text: None,
                });
                Err(e)
            }
        }
    }

    /// Trait-required entry point. Computes counters from the event log so
    /// standalone callers (e.g. unit tests) don't need to pre-compute them.
    async fn run_turn(
        &self,
        session_id: &SessionId,
        callback: &mut dyn HarnessCallback,
    ) -> Result<TurnState, HarnessError> {
        let events = self.deps.session.get_events(session_id, None, None).await?;
        let iterations = count_assistant_messages(&events).saturating_add(1);
        let tool_calls_made = count_tool_calls(&events);
        let cancel = tokio_util::sync::CancellationToken::new();
        self.run_turn_internal(session_id, callback, iterations, tool_calls_made, &cancel)
            .await
            .map(|(state, _, _)| state)
    }
}

/// Extension trait on HarnessCallback so we can call `on_complete` even when
/// holding `&mut dyn HarnessCallback`. The direct fn call works on the
/// trait object; this is just a named shim to keep the call site readable.
trait HarnessCallbackExt {
    fn on_complete_via_harness(&mut self);
}

impl HarnessCallbackExt for dyn HarnessCallback + '_ {
    fn on_complete_via_harness(&mut self) {
        self.on_complete();
    }
}

/// Index at which events "since the last AssistantMessage" begin.
///
/// Returns `events.len()` when the log ends with an AssistantMessage and
/// `0` when there is none. The caller uses this both as the start of the
/// tail slice and as the boundary for reconstructing the prior assistant
/// turn.
fn tail_start_index(events: &[SessionEventRecord]) -> usize {
    events
        .iter()
        .rposition(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
        .map(|idx| idx + 1)
        .unwrap_or(0)
}

/// Count `AssistantMessage` events in the log — used as "iterations so far"
/// when composing stop-hook context.
fn count_assistant_messages(events: &[SessionEventRecord]) -> usize {
    events
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }))
        .count()
}

/// Count `ToolCallRequested` events — used as "tool_calls_made so far"
/// when composing stop-hook context.
fn count_tool_calls(events: &[SessionEventRecord]) -> usize {
    events
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::ToolCallRequested { .. }))
        .count()
}

/// Serialize each `NativeToolCall` as a JSON `tool_use` block so the full
/// assistant intent is preserved in the session log.
///
/// `pub(crate)` so round-trip tests can exercise the writer/reader pair.
pub(crate) fn tool_use_blocks(tool_calls: &[NativeToolCall]) -> Vec<Value> {
    tool_calls
        .iter()
        .map(|c| {
            json!({
                "type": "tool_use",
                "id": c.id,
                "name": c.name,
                "input": c.arguments,
            })
        })
        .collect()
}

/// Find the most recent `TurnStarted` id; generate a fresh one if none exists.
fn current_turn_id(events: &[SessionEventRecord]) -> TurnId {
    events
        .iter()
        .rev()
        .find_map(|r| match &r.event {
            SessionEvent::TurnStarted { turn_id, .. } => Some(*turn_id),
            _ => None,
        })
        .unwrap_or_else(uuid::Uuid::new_v4)
}

#[cfg(test)]
mod tests {
    //! Inline tests for `AgentHarness` behaviours that assert on the exact
    //! `RequestPayload` handed to the provider. The broader Think/Act/Driver
    //! suites live in `harness::tests::{think,act,driver,task10_wiring}`.

    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    use crate::error::Result as AlephResult;
    use crate::harness::callback::NoopHarnessCallback;
    use crate::harness::deps::HarnessDeps;
    use crate::harness::trait_def::Harness;
    use crate::providers::adapter::{ProviderResponse, RequestPayload};
    use crate::providers::AiProvider;
    use crate::routing::session_key::SessionKey;
    use crate::session::events::{now_ms, MessageContent, SessionEvent, TurnTrigger};
    use crate::session::in_process::InProcessActorSessionService;
    use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};

    /// Provider that records the `system_prompt` it saw on each `process`
    /// call, then returns a text-only response so the harness loop finishes
    /// in one Think turn.
    struct RecordingProvider {
        captured: Arc<Mutex<Option<String>>>,
    }

    impl AiProvider for RecordingProvider {
        fn process<'a>(
            &'a self,
            payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            let captured = self.captured.clone();
            *captured.lock().unwrap() = payload.system_prompt.map(|s| s.to_string());
            Box::pin(async move { Ok(ProviderResponse::text_only("ok".to_string())) })
        }

        fn name(&self) -> &str {
            "recording"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }

    /// Tool service with no registered tools. The harness never dispatches
    /// a tool in this test (the provider returns text-only) so `execute`
    /// returning `NotFound` is safe — it is simply never called.
    struct EmptyTools;

    #[async_trait::async_trait]
    impl crate::tools::service::ToolService for EmptyTools {
        async fn execute(
            &self,
            name: &str,
            _input: serde_json::Value,
        ) -> Result<crate::session::events::ToolOutput, crate::tools::service::ToolError> {
            Err(crate::tools::service::ToolError::NotFound {
                name: name.to_string(),
            })
        }

        async fn list(&self) -> Vec<crate::tools::service::ToolDefinition> {
            Vec::new()
        }

        async fn describe(&self, _name: &str) -> Option<crate::tools::service::ToolDefinition> {
            None
        }
        fn dispatcher_schema(&self) -> std::sync::Arc<[crate::dispatcher::ToolDefinition]> {
            std::sync::Arc::from([])
        }
    }

    #[tokio::test]
    async fn system_prompt_flows_into_request_payload() {
        let captured = Arc::new(Mutex::new(None));
        let provider: Arc<dyn AiProvider> = Arc::new(RecordingProvider {
            captured: captured.clone(),
        });

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        let session: Arc<dyn crate::session::service::SessionService> =
            Arc::new(InProcessActorSessionService::new(store));

        let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(EmptyTools);
        // NoopSandbox never fires in this test — the provider returns
        // text-only, so no tool call → no sandbox dispatch.
        let sandbox: Arc<dyn crate::sandbox::Sandbox> = Arc::new(crate::sandbox::NoopSandbox);

        let sid = SessionKey::ephemeral("test-syspr");
        session.attach(sid.clone()).await.unwrap();
        let turn = uuid::Uuid::new_v4();
        session
            .emit_event(
                &sid,
                SessionEvent::TurnStarted {
                    turn_id: turn,
                    trigger: TurnTrigger::UserMessage,
                    at: now_ms(),
                },
            )
            .await
            .unwrap();
        session
            .emit_event(
                &sid,
                SessionEvent::UserMessage {
                    turn_id: turn,
                    content: MessageContent {
                        text: "hello".into(),
                        blocks: vec![],
                        thinking: None,
                        thinking_signature: None,
                    },
                    at: now_ms(),
                },
            )
            .await
            .unwrap();

        let deps = HarnessDeps {
            session: session.clone(),
            tools,
            sandbox,
            llm: provider,
            stop_hooks: None,
            context_budget: None,
            context_compactor: None,
            skill_prefetcher: None,
            trace_sink: None,
            system_prompt: Some("ROLE: SPEC-BOT".into()),
            prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
            chain_context: crate::harness::chain_context::ChainContext::default(),
            max_iterations: None,
            power: None,
            stall_config: None,
            consecutive_failure_cap: None,
            turn_timeout: None,
        };
        let harness = super::AgentHarness::new(deps);
        let mut cb = NoopHarnessCallback;
        harness.run_turn(&sid, &mut cb).await.expect("run_turn");

        let got = captured.lock().unwrap().clone();
        assert_eq!(got.as_deref(), Some("ROLE: SPEC-BOT"));
    }

    /// Provider that always returns one tool_call, forcing `TurnState::Continue`
    /// forever. Used to verify the `max_iterations` cap cuts the loop off.
    struct LoopingProvider {
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl AiProvider for LoopingProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            let calls = self.calls.clone();
            Box::pin(async move {
                let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(ProviderResponse {
                    text: None,
                    tool_calls: vec![crate::providers::adapter::NativeToolCall {
                        id: format!("call-{n}"),
                        name: "noop".into(),
                        arguments: serde_json::json!({}),
                    }],
                    thinking: None,
                    thinking_signature: None,
                    stop_reason: crate::providers::adapter::StopReason::ToolUse,
                    usage: None,
                })
            })
        }

        fn name(&self) -> &str {
            "looping"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }

    /// Provider that returns a tool_call on calls 1..=tool_turns, then a final
    /// text-only response (stop_reason = EndTurn) so the harness reaches
    /// `TurnState::Done` naturally.
    struct CountingProvider {
        calls: Arc<std::sync::atomic::AtomicUsize>,
        tool_turns: usize,
    }

    impl AiProvider for CountingProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            let calls = self.calls.clone();
            let tool_turns = self.tool_turns;
            Box::pin(async move {
                let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n < tool_turns {
                    Ok(ProviderResponse {
                        text: None,
                        tool_calls: vec![crate::providers::adapter::NativeToolCall {
                            id: format!("call-{n}"),
                            name: "noop".into(),
                            arguments: serde_json::json!({}),
                        }],
                        thinking: None,
                        thinking_signature: None,
                        stop_reason: crate::providers::adapter::StopReason::ToolUse,
                        usage: None,
                    })
                } else {
                    Ok(ProviderResponse::text_only("done".to_string()))
                }
            })
        }

        fn name(&self) -> &str {
            "counting"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }

    /// Tool service whose `execute` always succeeds with an empty JSON payload.
    /// The real tool name is irrelevant — the harness only needs `execute` to
    /// return `Ok` so the tool_result is persisted and the next Think runs.
    struct AlwaysOkTools;

    #[async_trait::async_trait]
    impl crate::tools::service::ToolService for AlwaysOkTools {
        async fn execute(
            &self,
            _name: &str,
            _input: serde_json::Value,
        ) -> Result<crate::session::events::ToolOutput, crate::tools::service::ToolError> {
            Ok(crate::session::events::ToolOutput {
                value: serde_json::json!({}),
                metadata: crate::session::events::ToolOutputMetadata::default(),
            })
        }

        async fn list(&self) -> Vec<crate::tools::service::ToolDefinition> {
            Vec::new()
        }

        async fn describe(&self, _name: &str) -> Option<crate::tools::service::ToolDefinition> {
            None
        }
        fn dispatcher_schema(&self) -> std::sync::Arc<[crate::dispatcher::ToolDefinition]> {
            std::sync::Arc::from([])
        }
    }

    /// Build a freshly-attached session with a single TurnStarted + UserMessage
    /// pair so `AgentHarness::run_turn` has work to do on the first call.
    async fn fresh_session(
        tag: &str,
    ) -> (
        Arc<dyn crate::session::service::SessionService>,
        crate::session::service::SessionId,
    ) {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        let session: Arc<dyn crate::session::service::SessionService> =
            Arc::new(InProcessActorSessionService::new(store));

        let sid = SessionKey::ephemeral(tag);
        session.attach(sid.clone()).await.unwrap();
        let turn = uuid::Uuid::new_v4();
        session
            .emit_event(
                &sid,
                SessionEvent::TurnStarted {
                    turn_id: turn,
                    trigger: TurnTrigger::UserMessage,
                    at: now_ms(),
                },
            )
            .await
            .unwrap();
        session
            .emit_event(
                &sid,
                SessionEvent::UserMessage {
                    turn_id: turn,
                    content: MessageContent {
                        text: "go".into(),
                        blocks: vec![],
                        thinking: None,
                        thinking_signature: None,
                    },
                    at: now_ms(),
                },
            )
            .await
            .unwrap();
        (session, sid)
    }

    #[tokio::test]
    async fn max_iterations_stops_runaway_loop() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider: Arc<dyn AiProvider> = Arc::new(LoopingProvider {
            calls: calls.clone(),
        });

        let (session, sid) = fresh_session("test-cap").await;
        let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(AlwaysOkTools);
        let sandbox: Arc<dyn crate::sandbox::Sandbox> = Arc::new(crate::sandbox::NoopSandbox);

        let deps = HarnessDeps {
            session,
            tools,
            sandbox,
            llm: provider,
            stop_hooks: None,
            context_budget: None,
            context_compactor: None,
            skill_prefetcher: None,
            trace_sink: None,
            system_prompt: None,
            prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
            chain_context: crate::harness::chain_context::ChainContext::default(),
            max_iterations: Some(3),
            power: None,
            stall_config: None,
            consecutive_failure_cap: None,
            turn_timeout: None,
        };
        let harness = super::AgentHarness::new(deps);
        let mut cb = NoopHarnessCallback;
        let cancel = tokio_util::sync::CancellationToken::new();

        // Hard timeout: without the cap implemented, `run` would spin forever.
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            harness.run(&sid, &mut cb, &cancel),
        )
        .await;

        outcome
            .expect("harness.run exceeded 2s timeout — max_iterations cap not enforced")
            .expect("harness.run returned an error");

        assert!(harness.hit_limit(), "expected hit_limit=true after cap");
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "provider should be called exactly max_iterations (3) times",
        );
    }

    #[tokio::test]
    async fn max_iterations_none_keeps_unbounded() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider: Arc<dyn AiProvider> = Arc::new(CountingProvider {
            calls: calls.clone(),
            tool_turns: 4,
        });

        let (session, sid) = fresh_session("test-unbounded").await;
        let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(AlwaysOkTools);
        let sandbox: Arc<dyn crate::sandbox::Sandbox> = Arc::new(crate::sandbox::NoopSandbox);

        let deps = HarnessDeps {
            session,
            tools,
            sandbox,
            llm: provider,
            stop_hooks: None,
            context_budget: None,
            context_compactor: None,
            skill_prefetcher: None,
            trace_sink: None,
            system_prompt: None,
            prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
            chain_context: crate::harness::chain_context::ChainContext::default(),
            max_iterations: None,
            power: None,
            stall_config: None,
            consecutive_failure_cap: None,
            turn_timeout: None,
        };
        let harness = super::AgentHarness::new(deps);
        let mut cb = NoopHarnessCallback;
        let cancel = tokio_util::sync::CancellationToken::new();

        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            harness.run(&sid, &mut cb, &cancel),
        )
        .await
        .expect("harness.run exceeded 2s timeout")
        .expect("harness.run returned an error");

        assert!(
            !harness.hit_limit(),
            "hit_limit must be false for unbounded run"
        );
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            5,
            "provider should run 4 tool turns + 1 final text turn = 5 calls",
        );
    }
}
