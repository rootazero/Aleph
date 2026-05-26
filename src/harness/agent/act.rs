//! Act phase — tool-call execution with caching, guardrails, and an
//! opencode-parity parallel fast path.
//!
//! The default loop is serial in input order (every existing test relies on
//! that). When all of the following hold for a single Act batch, the harness
//! routes through [`AgentHarness::act_parallel`] instead, dispatching the
//! actual `tools.execute(...)` futures concurrently via
//! [`futures::stream::FuturesOrdered`] while keeping every side effect — event
//! emit, trace, layer-3 budget, timeline push — strictly in input order:
//!
//! * [`HarnessDeps::parallel_tool_concurrency`](crate::harness::deps::HarnessDeps)
//!   is `Some(n)` with `n >= 2`.
//! * the batch has at least two calls.
//! * every call returns `true` from
//!   [`ToolService::is_call_concurrent_safe`](crate::tools::service::ToolService::is_call_concurrent_safe)
//!   for its concrete arguments.
//! * no two calls in the batch carry the same canonical `(name, args)` —
//!   parallel mode skips within-batch dedup, so duplicates fall back to the
//!   serial path where the memo correctly emits a cached result for the
//!   second occurrence.
//! * no guardrail registry is wired — the serial path's three-way Block /
//!   Sanitize / Pass machinery is intentionally not duplicated here; batches
//!   with guardrails always run serially.
//!
//! Any failing precondition falls through to the existing serial loop with no
//! observable behavior change.

use std::collections::HashMap;
use std::time::Instant;

use futures::future::BoxFuture;
use futures::stream;
use futures::StreamExt;

/// RAII guard that calls `end_turn` on drop so per-turn budget state is
/// always released even when `act()` exits early via `?`.
struct TurnBudgetGuard<'a> {
    budget: Option<&'a crate::tools::turn_budget::TurnResultBudget>,
    turn_id: &'a crate::tools::turn_budget::TurnId,
}

impl<'a> Drop for TurnBudgetGuard<'a> {
    fn drop(&mut self) {
        if let Some(budget) = self.budget {
            budget.end_turn(self.turn_id);
        }
    }
}

use super::{AgentHarness, ToolCallGuardOutcome};
use crate::harness::callback::HarnessCallback;
use crate::harness::trait_def::{HarnessError, TurnPhase};
use crate::providers::adapter::NativeToolCall;
use crate::session::events::{now_ms, SessionEvent, ToolOutput, TurnId};
use crate::session::service::SessionId;
use tokio_util::sync::CancellationToken;

/// Pick the effective wall-clock budget for a tool call. Per-tool
/// metadata wins over the harness-wide `turn_timeout` fallback. Both
/// unset → no timeout (legacy behaviour).
fn resolve_effective_budget(
    per_tool: Option<std::time::Duration>,
    harness_fallback: Option<std::time::Duration>,
) -> Option<std::time::Duration> {
    per_tool.or(harness_fallback)
}

impl AgentHarness {
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
    pub(crate) async fn act(
        &self,
        session_id: &SessionId,
        turn_id: TurnId,
        tool_calls: Vec<NativeToolCall>,
        callback: &mut dyn HarnessCallback,
        iteration: usize,
        run_cancel: &CancellationToken,
    ) -> Result<usize, HarnessError> {
        let mut executed_count: usize = 0;

        // Within-batch idempotency memo. Scoped to this single `act()` call:
        // duplicate calls inside one tool batch are deduplicated, but a
        // legitimate cross-turn repeat (e.g. `read_file` after `write_file`,
        // or any time-varying tool such as `get_current_time`) always
        // re-executes against fresh state instead of replaying a stale result.
        let mut tool_call_cache: HashMap<(String, String), ToolOutput> = HashMap::new();

        // Layer 3 turn-budget boundary. `begin_turn` is idempotent — re-entering
        // the same `TurnId` is a no-op, so this is safe even if the caller
        // somehow loops on the same turn. `end_turn` is always called via the
        // RAII `TurnBudgetGuard` so per-turn state is released on every exit.
        let budget_turn_id = crate::tools::turn_budget::TurnId::new(turn_id);
        if let Some(budget) = self.deps.turn_budget.as_ref() {
            budget.begin_turn(budget_turn_id);
        }
        let _budget_guard = TurnBudgetGuard {
            budget: self.deps.turn_budget.as_ref().map(|v| v.as_ref()),
            turn_id: &budget_turn_id,
        };

        // opencode-parity parallel fast path. Falls through to the serial loop
        // below when any precondition fails (see module docs).
        if self.can_parallel_dispatch(&tool_calls).await {
            return self
                .act_parallel(
                    session_id,
                    turn_id,
                    tool_calls,
                    callback,
                    iteration,
                    &budget_turn_id,
                    run_cancel,
                )
                .await;
        }

        for mut call in tool_calls {
            // G3 (opencode-inspired): mechanical tool-name auto-repair. Models
            // occasionally emit `Read` when the tool is registered as `read`
            // (case drift) — without this, the call would be dispatched as
            // unknown and bounce through ToolError before the model self-
            // corrects. Pure string handling: lowercase only, and only when
            // the original is absent AND a lowercase variant exists. Anything
            // ambiguous falls through to the normal unknown-tool path.
            // R10-safe: no intent inference, no fuzzy matching.
            if !call.name.is_empty() && call.name.chars().any(|c| c.is_ascii_uppercase()) {
                let lower = call.name.to_ascii_lowercase();
                if self.deps.tools.describe(&call.name).await.is_none()
                    && self.deps.tools.describe(&lower).await.is_some()
                {
                    tracing::debug!(
                        original = %call.name,
                        repaired = %lower,
                        "tool name auto-repaired (case mismatch)",
                    );
                    call.name = lower;
                }
            }
            callback.on_tool_call(&call.name);
            let started = Instant::now();
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

            // Stage 5b (#9): Tool-call guardrail (Block skips THIS call).
            if let Some(registry) = self.deps.guardrails.as_ref() {
                match self
                    .apply_tool_call_guardrail(
                        registry, session_id, turn_id, &call, started, iteration, callback,
                    )
                    .await?
                {
                    ToolCallGuardOutcome::Pass => {}
                    ToolCallGuardOutcome::Sanitize(args) => call.arguments = args,
                    ToolCallGuardOutcome::Block => {
                        if let Some(ref tracker) = self.stall_tracker {
                            tracker.record_activity().await;
                        }
                        continue;
                    }
                }
            }

            // Within-batch dedup: identical (tool_name, canonical_args) pairs
            // inside this single tool batch return the first result without
            // re-executing. The memo is per-`act()` call, so a cross-turn
            // repeat always re-runs against fresh state.
            let cache_key = (
                call.name.clone(),
                super::canonical_json_string(&call.arguments),
            );

            // Cross-batch dedup: refuse identical (tool, args) that already
            // failed earlier in this run. The synthesized ToolError nudges
            // the LLM to pivot instead of looping on the same deterministic
            // failure (sandbox-blocked URL, quota-exhausted API, etc.). The
            // failure set is cleared whenever any tool succeeds.
            if self.is_recent_failure(&call.name, &cache_key.1) {
                tracing::warn!(
                    tool = %call.name,
                    call_id = %call.id,
                    "cross-batch dedup: refusing identical repeat of a previously-failed call",
                );
                let synthetic = crate::tools::service::ToolError::Execution {
                    name: call.name.clone(),
                    cause: "this exact call already failed earlier in the run; \
                            change inputs or try a different tool"
                        .to_string(),
                };
                self.emit_tool_error(session_id, turn_id, &call, synthetic, started, iteration)
                    .await;
                if let Some(ref tracker) = self.stall_tracker {
                    tracker.record_activity().await;
                }
                continue;
            }

            if let Some(cached) = tool_call_cache.get(&cache_key) {
                tracing::warn!(
                    tool = %call.name,
                    call_id = %call.id,
                    "duplicate tool call deduplicated within batch (no re-execution)",
                );
                executed_count = executed_count.saturating_add(1);
                let output = cached.clone();
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
                            duration_ms: 0,
                        },
                        result: crate::tools::runtime::ToolResult::Success {
                            output: output_value,
                        },
                    },
                );
                // Memo-hit invocations cost 0 ms; record them anyway so the
                // tool count on the timeline matches what the agent actually
                // observed (the model saw a result come back).
                self.push_tool_invocation(call.id.clone(), call.name.clone(), 0, true, None);
                if let Some(ref tracker) = self.stall_tracker {
                    tracker.record_activity().await;
                }
                continue;
            }

            // Fork a per-call cancel token from the run's parent so a
            // selective tool-level abort (or run-wide cancel) drops THIS
            // call's future without affecting earlier completed events.
            let call_cancel = run_cancel.child_token();
            // Gap B follow-up — surface this call's per-call token to the
            // gateway via the in-flight registry so `tools.cancel_call` can
            // fire it by `tool_call_id`. The guard removes the entry on drop
            // (normal completion, `?` early-return, or panic alike).
            let _in_flight_guard = self
                .deps
                .in_flight_tool_calls
                .as_ref()
                .map(|reg| reg.register(&call.id, &call.name, call_cancel.clone()));
            let exec_fut = self.deps.tools.execute_with_cancel(
                &call.name,
                call.arguments.clone(),
                call_cancel,
            );

            // Resolve effective wall-clock budget: per-tool metadata > global fallback.
            let per_tool_budget = self
                .deps
                .tools
                .describe(&call.name)
                .await
                .and_then(|d| d.metadata.max_duration_ms)
                .map(std::time::Duration::from_millis);
            let effective_budget =
                resolve_effective_budget(per_tool_budget, self.deps.turn_timeout);

            let exec_result: Result<
                Result<ToolOutput, crate::tools::service::ToolError>,
                HarnessError,
            > = match effective_budget {
                Some(budget) => {
                    let started_call = Instant::now();
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
                Ok(mut output) => {
                    executed_count = executed_count.saturating_add(1);
                    self.apply_turn_budget(&budget_turn_id, &call, &mut output);
                    tool_call_cache.insert(cache_key.clone(), output.clone());
                    // Cross-batch dedup: a single success clears the failure
                    // set — the LLM has demonstrably pivoted to a working
                    // strategy.
                    self.clear_failures();
                    self.emit_tool_success(
                        session_id, turn_id, &call, output, started, iteration,
                    )
                    .await?;
                }
                Err(e) => {
                    // Do NOT abort — continue processing remaining tool calls.
                    // The error is persisted to session log; the next Think
                    // turn will see it as tool_result(is_error=true).
                    self.record_failure(call.name.clone(), cache_key.1.clone());
                    self.emit_tool_error(session_id, turn_id, &call, e, started, iteration)
                        .await;
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
}

// =============================================================================
// Parallel fast path
// =============================================================================

impl AgentHarness {
    /// Whether the current batch is eligible for the parallel fast path.
    ///
    /// All preconditions are checked in O(n) for batch size n; the
    /// `is_call_concurrent_safe` query is one trait await per call. Returns
    /// `false` cheaply when concurrency is disabled, when guardrails are
    /// wired, or when there are fewer than two calls.
    async fn can_parallel_dispatch(&self, tool_calls: &[NativeToolCall]) -> bool {
        let Some(par_n) = self.deps.parallel_tool_concurrency else {
            return false;
        };
        if par_n < 2 || tool_calls.len() < 2 {
            return false;
        }
        // Guardrails (Block / Sanitize / Pass) gate dispatch in the serial
        // loop. Replicating that surface inside the parallel pipeline would
        // double the failure modes of the fast path; for now any batch with
        // guardrails wired falls through to serial.
        if self.deps.guardrails.is_some() {
            return false;
        }
        // Reject batches with within-batch duplicates so the serial dedup
        // memo continues to own that semantics. (Duplicates are rare; the
        // common LLM pattern is N distinct read-only calls.)
        let mut seen = std::collections::HashSet::new();
        for call in tool_calls {
            let key = (
                call.name.clone(),
                super::canonical_json_string(&call.arguments),
            );
            if !seen.insert(key) {
                return false;
            }
        }
        // Every call must self-report concurrent-safe for its concrete input.
        for call in tool_calls {
            if !self
                .deps
                .tools
                .is_call_concurrent_safe(&call.name, &call.arguments)
                .await
            {
                return false;
            }
        }
        true
    }

    /// Parallel fast path. Pre-emits `ToolCallRequested` events in input
    /// order, dispatches all executes concurrently via `FuturesOrdered`, and
    /// then walks the completion results in input order to emit
    /// `ToolResult` / `ToolError`, record the Layer 3 turn budget, and push
    /// the timeline entry. Side-effect ordering matches the serial loop.
    ///
    /// Per-tool wall-clock budget is wrapped around each individual future
    /// (same as the serial path). A timeout in this path is bubbled up as
    /// `HarnessError::StalledTurn` AFTER all already-completed results have
    /// been emitted in input order — strictly more information than the
    /// serial path (which exits without emitting later calls' results).
    #[allow(clippy::too_many_arguments)]
    async fn act_parallel(
        &self,
        session_id: &SessionId,
        turn_id: TurnId,
        tool_calls: Vec<NativeToolCall>,
        callback: &mut dyn HarnessCallback,
        iteration: usize,
        budget_turn_id: &crate::tools::turn_budget::TurnId,
        run_cancel: &CancellationToken,
    ) -> Result<usize, HarnessError> {
        let mut executed_count: usize = 0;

        // Cross-batch dedup preflight: index calls whose (tool, args) already
        // failed earlier in this run. Skipped calls receive a synthetic
        // ToolError below alongside the normal request-event emission so the
        // session timeline stays linear. PASS 1 then dispatches ONLY the
        // surviving calls in parallel, preserving original input order.
        let canonical_args: Vec<String> = tool_calls
            .iter()
            .map(|c| super::canonical_json_string(&c.arguments))
            .collect();
        let skip: Vec<bool> = tool_calls
            .iter()
            .zip(canonical_args.iter())
            .map(|(c, args)| self.is_recent_failure(&c.name, args))
            .collect();

        // PASS 0 — serial: notify callback, emit ToolCallStarted trace,
        // emit ToolCallRequested SessionEvent. Resolve effective per-tool
        // wall-clock budget. Capture started Instant for duration metrics.
        // Skipped calls take the synthetic-error fast path here and are
        // omitted from PASS 1 dispatch.
        let mut started_at: Vec<Instant> = Vec::with_capacity(tool_calls.len());
        let mut budgets: Vec<Option<std::time::Duration>> = Vec::with_capacity(tool_calls.len());
        for (idx, call) in tool_calls.iter().enumerate() {
            callback.on_tool_call(&call.name);
            started_at.push(Instant::now());
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

            if skip[idx] {
                tracing::warn!(
                    tool = %call.name,
                    call_id = %call.id,
                    "cross-batch dedup (parallel): refusing identical repeat of a previously-failed call",
                );
                let synthetic = crate::tools::service::ToolError::Execution {
                    name: call.name.clone(),
                    cause: "this exact call already failed earlier in the run; \
                            change inputs or try a different tool"
                        .to_string(),
                };
                self.emit_tool_error(
                    session_id,
                    turn_id,
                    call,
                    synthetic,
                    started_at[idx],
                    iteration,
                )
                .await;
                if let Some(ref tracker) = self.stall_tracker {
                    tracker.record_activity().await;
                }
                budgets.push(None);
                continue;
            }

            let per_tool_budget = self
                .deps
                .tools
                .describe(&call.name)
                .await
                .and_then(|d| d.metadata.max_duration_ms)
                .map(std::time::Duration::from_millis);
            budgets.push(resolve_effective_budget(per_tool_budget, self.deps.turn_timeout));
        }

        // PASS 1 — parallel: dispatch up to `parallelism` execute futures
        // concurrently via `stream::iter(...).buffered(n)`. `buffered` polls
        // at most `n` futures at a time AND yields completions in input
        // order — semantically identical to opencode's
        // `Effect.forEach({ concurrency: n })`. Per-call timeout is wrapped
        // INSIDE each future so the timeout is owned by the call, not the
        // batch.
        let parallelism = self
            .deps
            .parallel_tool_concurrency
            .unwrap_or(0)
            .max(2);
        type ExecOutcome =
            Result<Result<ToolOutput, crate::tools::service::ToolError>, std::time::Duration>;
        // Build per-original-index futures, leaving None at skipped indices
        // (cross-batch dedup already emitted synthetic ToolError in PASS 0).
        // Run only the live futures through `.buffered()` and re-assemble
        // results in original input order via `live_indices`, so PASS 2 keeps
        // its existing `for (idx, exec_result) in results.iter().enumerate()`
        // contract against `tool_calls`.
        let mut boxed_futs_opt: Vec<Option<BoxFuture<'static, ExecOutcome>>> =
            Vec::with_capacity(tool_calls.len());
        // Gap B follow-up — keep one InFlightGuard per call alive for the
        // duration of the whole parallel dispatch. Each guard drops when this
        // Vec goes out of scope after PASS 2 finishes, which is strictly
        // later than every future resolves.
        let mut in_flight_guards: Vec<crate::tools::in_flight::InFlightGuard> = Vec::new();
        for (idx, call) in tool_calls.iter().enumerate() {
            if skip[idx] {
                boxed_futs_opt.push(None);
                continue;
            }
            let tools = self.deps.tools.clone();
            let name = call.name.clone();
            let args = call.arguments.clone();
            let budget = budgets[idx];
            let started = started_at[idx];
            // Each parallel call owns a fresh child token forked from the
            // run-level cancel. If the run is cancelled mid-batch, every
            // in-flight call short-circuits without waiting for the entire
            // batch to drain.
            let call_cancel = run_cancel.child_token();
            if let Some(reg) = self.deps.in_flight_tool_calls.as_ref() {
                in_flight_guards.push(reg.register(&call.id, &call.name, call_cancel.clone()));
            }
            boxed_futs_opt.push(Some(Box::pin(async move {
                let exec_fut = tools.execute_with_cancel(&name, args, call_cancel);
                match budget {
                    Some(b) => match tokio::time::timeout(b, exec_fut).await {
                        Ok(inner) => Ok(inner),
                        Err(_) => Err(started.elapsed()),
                    },
                    None => Ok(exec_fut.await),
                }
            })));
        }
        let live_indices: Vec<usize> = boxed_futs_opt
            .iter()
            .enumerate()
            .filter_map(|(i, f)| f.as_ref().map(|_| i))
            .collect();
        let live_futs: Vec<BoxFuture<'static, ExecOutcome>> =
            boxed_futs_opt.into_iter().flatten().collect();
        let live_results: Vec<ExecOutcome> = stream::iter(live_futs)
            .buffered(parallelism)
            .collect()
            .await;
        // Reassemble per-original-index results. `None` slots are skipped
        // calls — already emitted as synthetic errors in PASS 0; PASS 2 below
        // ignores them via the same `skip[idx]` flag.
        let mut results: Vec<Option<ExecOutcome>> = (0..tool_calls.len()).map(|_| None).collect();
        for (live_idx, exec) in live_results.into_iter().enumerate() {
            results[live_indices[live_idx]] = Some(exec);
        }
        // PASS 1 complete — every future has resolved, so the in-flight
        // registry entries are no longer cancellable in any meaningful sense.
        // Drop the guards now (vs end-of-function) so `tools.cancel_call`
        // doesn't quietly target a token attached to a future that's already
        // returned. PASS 2 below only emits events; it never touches the
        // tokens or the registry.
        drop(in_flight_guards);

        // PASS 2 — serial in input order: apply Layer 3 budget, emit
        // ToolResult/ToolError, trace, push timeline entry. The first
        // timeout encountered is remembered and bubbled up at the end so
        // later already-completed results still reach the session log.
        // Skipped indices (cross-batch dedup hits, already errored in PASS 0)
        // are passed through with no further action.
        let mut first_stall: Option<(String, std::time::Duration)> = None;
        for (idx, exec_slot) in results.into_iter().enumerate() {
            let Some(exec_result) = exec_slot else {
                continue; // PASS-0 dedup-rejected; already emitted synthetic error.
            };
            let call = &tool_calls[idx];
            let started = started_at[idx];
            match exec_result {
                Err(elapsed) => {
                    if first_stall.is_none() {
                        first_stall = Some((call.name.clone(), elapsed));
                    }
                    // No SessionEvent::ToolError emitted — matches serial
                    // semantics where timeout returns StalledTurn without
                    // emitting a per-call error event.
                    if let Some(ref tracker) = self.stall_tracker {
                        tracker.record_activity().await;
                    }
                }
                Ok(Ok(mut output)) => {
                    executed_count = executed_count.saturating_add(1);
                    self.apply_turn_budget(budget_turn_id, call, &mut output);
                    // Cross-batch dedup: any success clears the failure set —
                    // the LLM has demonstrably pivoted to a working strategy.
                    self.clear_failures();
                    self.emit_tool_success(
                        session_id, turn_id, call, output, started, iteration,
                    )
                    .await?;
                    if let Some(ref tracker) = self.stall_tracker {
                        tracker.record_activity().await;
                    }
                }
                Ok(Err(e)) => {
                    // Cross-batch dedup: record the (tool, args) signature so
                    // the next turn refuses an identical repeat.
                    self.record_failure(call.name.clone(), canonical_args[idx].clone());
                    self.emit_tool_error(session_id, turn_id, call, e, started, iteration)
                        .await;
                    if let Some(ref tracker) = self.stall_tracker {
                        tracker.record_activity().await;
                    }
                }
            }
        }

        if let Some((tool_name, elapsed)) = first_stall {
            return Err(HarnessError::StalledTurn {
                phase: TurnPhase::Act { tool_name },
                elapsed,
            });
        }

        Ok(executed_count)
    }
}

// =============================================================================
// Shared helpers — used by both the serial path (`act`) and the parallel
// fast path (`act_parallel`). Extracted to eliminate ~240 LOC of dual-
// maintenance risk: fixes to budget-spill semantics or event emission only
// have to land in one place.
// =============================================================================

impl AgentHarness {
    /// Apply Layer-3 turn budget: record this result, persist any spills via
    /// the shared result store, and rewrite `output.value` to a marker string
    /// when THIS call's result was the one spilled (so the LLM's next Think
    /// sees the marker, not the full text). No-op when budget is unset.
    fn apply_turn_budget(
        &self,
        budget_turn_id: &crate::tools::turn_budget::TurnId,
        call: &NativeToolCall,
        output: &mut ToolOutput,
    ) {
        let Some(budget) = self.deps.turn_budget.as_ref() else {
            return;
        };
        let text = match &output.value {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let already_persisted = text.starts_with("[Full output persisted: ");
        let tokens = crate::context::budget::pressure::estimate_tokens_smart(&text);
        let record = crate::tools::turn_budget::TurnResult {
            call_id: call.id.clone(),
            tool_name: call.name.clone(),
            tokens_in_context: tokens,
            in_context_text: text,
            already_persisted,
        };
        let spills = budget.record(budget_turn_id, record);
        if spills.is_empty() {
            return;
        }
        let Some(store) = self.deps.result_store.as_ref() else {
            return;
        };
        for spill in spills {
            if spill.call_id != call.id {
                // Earlier-iteration spill: its SessionEvent::ToolResult is
                // already persisted, so post-hoc rewrite from here isn't
                // possible. The marker file is still written for recovery;
                // cheap_passes surfaces it on the next preflight.
                let _ = store.persist_if_large(
                    &spill.call_id,
                    &spill.tool_name,
                    &spill.original_text,
                    0,
                );
                continue;
            }
            // Same-turn newest spill: rewrite output BEFORE the SessionEvent
            // is emitted so the LLM sees the marker instead of the full text.
            if let Some(marker) = store.persist_if_large(
                &spill.call_id,
                &spill.tool_name,
                &spill.original_text,
                0,
            ) {
                output.value = serde_json::Value::String(marker);
                output.metadata.truncated = true;
            }
        }
    }

    /// Persist a successful tool call: emit `SessionEvent::ToolResult`,
    /// trace event, and timeline entry.
    async fn emit_tool_success(
        &self,
        session_id: &SessionId,
        turn_id: TurnId,
        call: &NativeToolCall,
        output: ToolOutput,
        started: Instant,
        iteration: usize,
    ) -> Result<(), HarnessError> {
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
        let dur_ms: u64 = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
        self.emit(
            || crate::harness::trace::LoopTraceEvent::ToolCallCompleted {
                iteration,
                call: crate::harness::trace::ToolCallEndEvent {
                    tool_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    input: call.arguments.clone(),
                    duration_ms: dur_ms,
                },
                result: crate::tools::runtime::ToolResult::Success {
                    output: output_value,
                },
            },
        );
        self.push_tool_invocation(call.id.clone(), call.name.clone(), dur_ms, true, None);
        Ok(())
    }

    /// Persist a failed tool call: emit `SessionEvent::ToolError` (best
    /// effort — failure to persist logs at WARN, never aborts the batch),
    /// trace event, timeline entry.
    async fn emit_tool_error(
        &self,
        session_id: &SessionId,
        turn_id: TurnId,
        call: &NativeToolCall,
        e: crate::tools::service::ToolError,
        started: Instant,
        iteration: usize,
    ) {
        let retryable = e.is_retryable();
        let error_msg = e.to_string();
        let error_event = SessionEvent::ToolError {
            turn_id,
            call_id: call.id.clone(),
            error: error_msg.clone(),
            at: now_ms(),
        };
        if let Err(emit_err) = self.deps.session.emit_event(session_id, error_event).await {
            tracing::warn!(
                ?session_id,
                call_id = %call.id,
                ?emit_err,
                "failed to persist ToolError event",
            );
        }
        let dur_ms: u64 = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
        let error_for_timeline = error_msg.clone();
        self.emit(
            || crate::harness::trace::LoopTraceEvent::ToolCallCompleted {
                iteration,
                call: crate::harness::trace::ToolCallEndEvent {
                    tool_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    input: call.arguments.clone(),
                    duration_ms: dur_ms,
                },
                result: crate::tools::runtime::ToolResult::Error {
                    error: error_msg,
                    retryable,
                },
            },
        );
        self.push_tool_invocation(
            call.id.clone(),
            call.name.clone(),
            dur_ms,
            false,
            Some(error_for_timeline),
        );
    }
}

#[cfg(test)]
mod per_tool_budget_tests {
    use super::*;

    #[test]
    fn resolve_effective_budget_prefers_per_tool_over_global() {
        let per_tool = Some(std::time::Duration::from_millis(50));
        let global = Some(std::time::Duration::from_secs(60));
        assert_eq!(
            resolve_effective_budget(per_tool, global),
            Some(std::time::Duration::from_millis(50)),
        );
    }

    #[test]
    fn resolve_effective_budget_falls_back_to_global() {
        let global = Some(std::time::Duration::from_secs(60));
        assert_eq!(
            resolve_effective_budget(None, global),
            Some(std::time::Duration::from_secs(60)),
        );
    }

    #[test]
    fn resolve_effective_budget_returns_none_when_both_unset() {
        assert_eq!(resolve_effective_budget(None, None), None);
    }
}
