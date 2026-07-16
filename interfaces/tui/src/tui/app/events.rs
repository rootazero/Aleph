//! `StreamEvent` → `AppState` mutations + quit signal.
//!
//! Pulled out of [`mod`] to keep the orchestrator file under the 1 kLOC
//! soft cap. Lives in a second `impl AppState { … }` block.

use std::time::{Duration, Instant};

use aleph_protocol::{
    summarize_tool_input, AgentTracePresentationPreset, AgentTraceToolResult, StreamEvent,
};

use super::super::slash::ToolProgressMode;
use super::{Action, AppState, ChatMessage};

impl AppState {
    /// Request application quit. Sets `should_quit` flag.
    pub const fn request_quit(&mut self) {
        self.should_quit = true;
    }

    // -- Gateway event handling -----------------------------------------

    /// Handle a `StreamEvent` from the gateway. Returns an Action if the event
    /// should trigger further side effects (e.g. scrolling to bottom).
    pub fn handle_gateway_event(&mut self, event: StreamEvent) -> Action {
        match event {
            StreamEvent::RunAccepted { run_id, .. } => {
                self.current_run = Some(run_id);
                self.run_started_at = Some(Instant::now());
                self.current_run_uses_agent_trace = false;
                self.current_run_trace_summary_applied = false;
                self.is_connected = true;
                Action::None
            }

            StreamEvent::AgentTrace { event, .. } => {
                self.current_run_uses_agent_trace = true;
                self.apply_agent_trace_event(&event)
            }

            StreamEvent::Reasoning { content, .. } => {
                self.ensure_assistant_message();
                if let ChatMessage::Assistant { reasoning, .. } = self.current_assistant_mut() {
                    match reasoning {
                        Some(existing) => existing.push_str(&content),
                        None => *reasoning = Some(content),
                    }
                }
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::ToolStart {
                tool_name,
                tool_id,
                params,
                ..
            } => {
                if self.current_run_uses_agent_trace {
                    return Action::None;
                }
                if matches!(self.tool_progress_mode, ToolProgressMode::Off) {
                    return Action::None;
                }
                self.start_tool_execution(
                    tool_id,
                    tool_name,
                    summarize_tool_input(&params, AgentTracePresentationPreset::TuiDebug.options()),
                );
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::ToolUpdate {
                tool_id, progress, ..
            } => {
                // /tools off|new — suppress mid-execution progress updates entirely.
                // /tools all|verbose — surface them.
                match self.tool_progress_mode {
                    ToolProgressMode::Off | ToolProgressMode::New => return Action::None,
                    ToolProgressMode::All | ToolProgressMode::Verbose => {}
                }
                if let Some(tool) = self.find_tool_mut(&tool_id) {
                    tool.progress = Some(progress);
                }
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::ToolEnd {
                tool_id,
                result,
                duration_ms,
                ..
            } => {
                if self.current_run_uses_agent_trace {
                    return Action::None;
                }
                if matches!(self.tool_progress_mode, ToolProgressMode::Off) {
                    return Action::None;
                }
                let result = if result.success {
                    AgentTraceToolResult::Success {
                        output: serde_json::json!(result.output),
                    }
                } else {
                    AgentTraceToolResult::Error {
                        error: result.error.unwrap_or_default(),
                        retryable: false,
                    }
                };
                self.finish_tool_execution(&tool_id, &result, duration_ms);
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::ResponseChunk { content, .. } => {
                self.append_assistant_content(&content);
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::RunComplete {
                summary,
                total_duration_ms,
                ..
            } => {
                self.current_run = None;
                self.run_started_at = None;
                self.dismiss_pending_approval();
                self.last_run_duration = Some(Duration::from_millis(total_duration_ms));
                if !self.current_run_trace_summary_applied {
                    self.update_token_usage(&summary);
                }
                self.current_run_uses_agent_trace = false;
                self.current_run_trace_summary_applied = false;
                self.mark_current_assistant_complete();

                // Surface non-clean terminations (a hit cap / exhausted budget)
                // so a truncated answer doesn't read as a clean finish.
                // `terminate_detail` carries the granular cap inside the budget
                // umbrella; fall back to the reason token when absent.
                if let Some(reason) = summary.terminate_reason.as_deref() {
                    if reason != "completed" {
                        let raw = summary.terminate_detail.as_deref().unwrap_or(reason);
                        let label = match raw {
                            "hit_max_iterations" => "hit max iterations",
                            "context_budget_exhausted" => "context budget exhausted",
                            "max_output_tokens_exhausted" => "max output tokens reached",
                            "budget_exhausted_partial_result" => {
                                "budget exhausted (partial result)"
                            }
                            other => other,
                        };
                        self.add_system_message(format!("Run stopped: {label}"));
                    }
                }

                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::RunError { error, .. } => {
                self.current_run = None;
                self.run_started_at = None;
                self.dismiss_pending_approval();
                self.current_run_uses_agent_trace = false;
                self.current_run_trace_summary_applied = false;
                self.mark_current_assistant_complete();

                self.add_system_message(format!("Error: {error}"));
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::AskUser {
                session_key,
                question,
                options,
                ..
            } => {
                self.show_dialog(session_key, question, options);
                Action::None
            }

            StreamEvent::ReasoningBlock { content, .. } => {
                if self.current_run_uses_agent_trace {
                    return Action::None;
                }
                // Treated same as Reasoning — append to reasoning buffer
                self.append_reasoning_entry(content);
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::UncertaintySignal {
                uncertainty,
                suggested_action,
                ..
            } => {
                let msg = format!(
                    "Uncertainty: {} ({})",
                    uncertainty,
                    suggested_action.description()
                );
                self.add_system_message(msg);
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::RunRetrying {
                provider,
                attempt,
                max_attempts,
                reason,
                ..
            } => {
                // Surface transient provider failures instead of leaving the
                // thinking indicator spinning silently through the retry
                // ladder (mirrors the Panel's stream.run_retrying notice).
                self.add_system_message(format!(
                    "Provider {provider} unreachable, retrying ({attempt}/{max_attempts}): {reason}"
                ));
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::ModelResolved { model_info, .. } => {
                // Only fallback selections are worth a system line — the
                // happy path would announce the model on every run.
                if model_info.is_fallback {
                    let requested = model_info
                        .original_model
                        .as_deref()
                        .filter(|orig| *orig != model_info.model);
                    let line = match requested {
                        Some(orig) => format!(
                            "Model fallback: {orig} → {} (via {})",
                            model_info.model, model_info.provider
                        ),
                        None => format!(
                            "Model fallback: {} (via {})",
                            model_info.model, model_info.provider
                        ),
                    };
                    self.add_system_message(line);
                    return Action::ScrollToBottomIfAutoScroll;
                }
                Action::None
            }

            StreamEvent::ContextGauge {
                context_tokens,
                context_window,
                ..
            } => {
                // Live context-window occupancy (mirrors the Panel gauge). Only
                // the numerator/denominator are stored; the event's own
                // `total_tokens` is deliberately ignored — RunComplete's summary
                // already owns the running token tally, and adding both would
                // double-count. Skip windowless frames so the bar never divides
                // by zero and keeps the last good reading.
                if context_window > 0 {
                    self.context_gauge = Some((context_tokens, context_window));
                }
                Action::None
            }
        }
    }
}
