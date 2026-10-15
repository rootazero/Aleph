//! Non-transcript effects of trace frames. The shared Transcript is the only
//! writer of conversation rows, for both live frames and replay.

use aleph_protocol::{AgentTraceEvent, AgentTraceReplay, AgentTraceToolResult};

use super::{Action, AppState, Focus};

impl AppState {
    pub(super) fn apply_agent_trace_event(&mut self, event: &AgentTraceEvent) -> Action {
        match event {
            AgentTraceEvent::ToolCallCompleted {
                call,
                result: AgentTraceToolResult::Success { output },
                ..
            } => {
                self.maybe_apply_plan_from_tool(&call.tool_name, output);
            }
            AgentTraceEvent::SessionCompleted { total_tokens, .. } => {
                if !self.current_run_trace_summary_applied && !self.replaying_trace {
                    self.total_tokens = self
                        .total_tokens
                        .saturating_add(u64::try_from(*total_tokens).unwrap_or(u64::MAX));
                    self.current_run_trace_summary_applied = true;
                }
                // Keep correlation until RunComplete: that authoritative frame
                // carries pricing and plan fields absent from this trace.
                self.run_started_at = None;
                if !self.replaying_trace {
                    self.dismiss_pending_approval();
                }
                self.current_run_uses_agent_trace = false;
            }
            AgentTraceEvent::ProviderUsage {
                agent_id,
                input_tokens,
                cache_read_tokens,
                cache_creation_tokens,
                ..
            } if !self.replaying_trace => {
                if self.cache_root_agent.is_none() {
                    self.cache_root_agent = Some(agent_id.clone());
                }
                let read = u64::from(cache_read_tokens.unwrap_or(0));
                let creation = u64::from(cache_creation_tokens.unwrap_or(0));
                if read > 0 || creation > 0 {
                    let ratio = aleph_protocol::cache_hit_ratio(
                        u64::from(*input_tokens),
                        cache_read_tokens.map(u64::from),
                    )
                    .unwrap_or(0.0);
                    self.cache_stat = Some((ratio * 100.0).round() as u64);
                    self.cache_stat_agent = match self.cache_root_agent.as_deref() {
                        Some(root) if root == agent_id => None,
                        _ => Some(agent_id.clone()),
                    };
                }
            }
            _ => {}
        }
        Action::None
    }

    pub fn load_trace_replay(&mut self, replay: &AgentTraceReplay) {
        let summary = format!(
            "Loaded replay {} from session {} [{}] via {}.",
            replay.task.task_id, replay.task.session_id, replay.task.status, replay.task.agent_id
        );
        self.messages.reset();
        self.transcript_generation = self.transcript_generation.wrapping_add(1);
        self.transcript_needs_resync = false;
        self.step_overrides.clear();
        self.invalidate_rendered_lines();
        self.current_run = None;
        self.run_started_at = None;
        self.current_run_uses_agent_trace = false;
        self.dialog = None;
        self.palette = None;
        self.approval = None;
        self.focus = Focus::Input;
        self.scroll_to_bottom();
        self.add_system_message(summary);
        self.replaying_trace = true;
        for trace in &replay.traces {
            let changes = self.messages.apply_replay(&trace.event);
            self.observe_transcript_changes(&changes);
        }
        let changes = self.messages.finish_replay();
        self.observe_transcript_changes(&changes);
        self.replaying_trace = false;
        if replay.traces.is_empty() {
            self.add_system_message("Replay has no structured trace events.".to_owned());
        }
        self.current_run_trace_summary_applied = false;
    }
}
