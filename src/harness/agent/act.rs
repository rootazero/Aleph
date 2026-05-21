//! Act phase — sequential tool-call execution with caching and guardrails.

use std::collections::HashMap;
use std::time::Instant;

use super::{AgentHarness, ToolCallGuardOutcome};
use crate::harness::callback::HarnessCallback;
use crate::harness::trait_def::{HarnessError, TurnPhase};
use crate::providers::adapter::NativeToolCall;
use crate::session::events::{now_ms, SessionEvent, ToolOutput, TurnId};
use crate::session::service::SessionId;

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
        // somehow loops on the same turn. `end_turn` runs at the bottom of
        // this function (and at every early return) to clear per-turn state.
        let budget_turn_id = crate::tools::turn_budget::TurnId::new(turn_id);
        if let Some(budget) = self.deps.turn_budget.as_ref() {
            budget.begin_turn(budget_turn_id);
        }

        for mut call in tool_calls {
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
            let cache_key = (call.name.clone(), super::canonical_json_string(&call.arguments));
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

            let exec_fut = self.deps.tools.execute(&call.name, call.arguments.clone());

            // Resolve effective wall-clock budget: per-tool metadata > global fallback.
            let per_tool_budget = self
                .deps
                .tools
                .describe(&call.name)
                .await
                .and_then(|d| d.metadata.max_duration_ms)
                .map(std::time::Duration::from_millis);
            let effective_budget = resolve_effective_budget(per_tool_budget, self.deps.turn_timeout);

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
                Err(stalled) => {
                    if let Some(budget) = self.deps.turn_budget.as_ref() {
                        budget.end_turn(&budget_turn_id);
                    }
                    return Err(stalled);
                }
            };
            match inner {
                Ok(mut output) => {
                    executed_count = executed_count.saturating_add(1);

                    // Layer 3 — per-turn aggregate budget. Record the result
                    // and, if the running total exceeds the configured cap,
                    // persist the LIFO-newest non-persisted entries to disk
                    // via the shared `result_store` and rewrite the in-flight
                    // `output.value` to the marker the LLM will see. This
                    // happens BEFORE `emit_event` so the persisted SessionEvent
                    // matches what the next Think turn will read back.
                    if let Some(budget) = self.deps.turn_budget.as_ref() {
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
                        let spills = budget.record(&budget_turn_id, record);
                        if !spills.is_empty() {
                            if let Some(store) = self.deps.result_store.as_ref() {
                                for spill in spills {
                                    if spill.call_id != call.id {
                                        // The spilled entry was recorded on an
                                        // earlier iteration; the corresponding
                                        // SessionEvent::ToolResult is already
                                        // persisted in the session store, so
                                        // rewriting it post-emit is not
                                        // possible from here. The marker file
                                        // is still written for recovery, and
                                        // cheap_passes will surface it on the
                                        // next preflight pass.
                                        let _ = store.persist_if_large(
                                            &spill.call_id,
                                            &spill.tool_name,
                                            &spill.original_text,
                                            0,
                                        );
                                        continue;
                                    }
                                    // Same-turn newest spill: rewrite `output`
                                    // before the SessionEvent is emitted so
                                    // the LLM sees the marker instead of the
                                    // full text on its next Think.
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
                        }
                    }

                    tool_call_cache.insert(cache_key.clone(), output.clone());
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
                    let dur_ms: u64 = started
                        .elapsed()
                        .as_millis()
                        .try_into()
                        .unwrap_or(u64::MAX);
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
                    self.push_tool_invocation(
                        call.id.clone(),
                        call.name.clone(),
                        dur_ms,
                        true,
                        None,
                    );
                }
                Err(e) => {
                    // Preserve the error's retryability for the trace before
                    // the variant is collapsed into a flat string.
                    let retryable = e.is_retryable();
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
                    let dur_ms: u64 = started
                        .elapsed()
                        .as_millis()
                        .try_into()
                        .unwrap_or(u64::MAX);
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

        // Layer 3 — release per-turn state. Failure here is impossible
        // (`end_turn` is infallible) but the option-guard mirrors the
        // begin-turn site for symmetry.
        if let Some(budget) = self.deps.turn_budget.as_ref() {
            budget.end_turn(&budget_turn_id);
        }

        Ok(executed_count)
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
