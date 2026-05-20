//! Act phase — sequential tool-call execution with caching and guardrails.

use std::collections::HashMap;
use std::time::Instant;

use super::{AgentHarness, ToolCallGuardOutcome};
use crate::harness::callback::HarnessCallback;
use crate::harness::trait_def::{HarnessError, TurnPhase};
use crate::providers::adapter::NativeToolCall;
use crate::session::events::{now_ms, SessionEvent, ToolOutput, TurnId};
use crate::session::service::SessionId;

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
        tool_call_cache: &mut HashMap<(String, String), ToolOutput>,
    ) -> Result<usize, HarnessError> {
        let mut executed_count: usize = 0;

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

            // Idempotent same-run memo: identical (tool_name, canonical_args)
            // pairs return the previous result without re-executing the tool.
            // Scope is per-`run` (one user request) so cross-request retries
            // are unaffected. Skips the per-call provider round-trip when the
            // model loops on the same call.
            let cache_key = (call.name.clone(), super::canonical_json_string(&call.arguments));
            if let Some(cached) = tool_call_cache.get(&cache_key) {
                tracing::warn!(
                    tool = %call.name,
                    call_id = %call.id,
                    "tool call deduplicated from same-run memo (no re-execution)",
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
            let exec_result: Result<
                Result<ToolOutput, crate::tools::service::ToolError>,
                HarnessError,
            > = match self.deps.turn_timeout {
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
                Ok(output) => {
                    executed_count = executed_count.saturating_add(1);
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
                                retryable: false,
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

        Ok(executed_count)
    }
}
