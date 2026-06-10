//! `StreamEvent` → `AppState` mutations + quit signal.
//!
//! Pulled out of [`mod`] to keep the orchestrator file under the 1 kLOC
//! soft cap. Lives in a second `impl AppState { … }` block.

use std::time::Duration;

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
                self.last_run_duration = Some(Duration::from_millis(total_duration_ms));
                if !self.current_run_trace_summary_applied {
                    self.update_token_usage(&summary);
                }
                self.current_run_uses_agent_trace = false;
                self.current_run_trace_summary_applied = false;
                self.mark_current_assistant_complete();

                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::RunError { error, .. } => {
                self.current_run = None;
                self.current_run_uses_agent_trace = false;
                self.current_run_trace_summary_applied = false;
                self.mark_current_assistant_complete();

                self.add_system_message(format!("Error: {error}"));
                Action::ScrollToBottomIfAutoScroll
            }

            StreamEvent::AskUser {
                run_id,
                question,
                options,
                ..
            } => {
                self.show_dialog(run_id, question, options);
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
        }
    }
}
