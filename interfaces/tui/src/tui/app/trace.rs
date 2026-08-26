//! `AgentTrace` projection: maps `AgentTraceEvent`s (live and replayed) onto
//! chat/tool/reasoning state.
//!
//! Pulled out of [`mod`] to keep it under the 1 kLOC soft cap, as a third
//! `impl AppState` block sibling to [`super::events`] (the `StreamEvent`
//! projection) — the two projection paths now sit side by side.

use std::time::Duration;

use aleph_protocol::{
    present_agent_trace_event_with_preset, summarize_tool_input, AgentTraceEvent,
    AgentTracePresentation, AgentTracePresentationPreset, AgentTraceReplay, AgentTraceTextKind,
    AgentTraceToolResult,
};

use super::{Action, AppState, ChatMessage, Focus, ToolExecution, ToolStatus};

impl AppState {
    pub(super) fn append_reasoning_entry(&mut self, content: String) {
        self.ensure_assistant_message();
        if let ChatMessage::Assistant { reasoning, .. } = self.current_assistant_mut() {
            match reasoning {
                Some(existing) if !existing.is_empty() => {
                    existing.push('\n');
                    existing.push_str(&content);
                }
                Some(existing) => existing.push_str(&content),
                None => *reasoning = Some(content),
            }
        }
    }

    pub(super) fn append_assistant_content(&mut self, content: &str) {
        if content.is_empty() {
            return;
        }

        // The one place both carriers of assistant text converge, which is why
        // the "did this run say anything" flag is set here rather than at each
        // caller — see `run_rendered_assistant_text`.
        self.run_rendered_assistant_text = true;
        self.ensure_assistant_message();
        if let ChatMessage::Assistant {
            content: msg_content,
            ..
        } = self.current_assistant_mut()
        {
            msg_content.push_str(content);
        }
    }

    pub(super) fn start_tool_execution(
        &mut self,
        tool_id: String,
        tool_name: String,
        params: String,
    ) {
        self.ensure_assistant_message();
        if let Some(tool) = self.find_tool_mut(&tool_id) {
            tool.name = tool_name;
            tool.params = params;
            tool.status = ToolStatus::Running;
            tool.duration = None;
            tool.progress = None;
            tool.error = None;
            return;
        }

        if let ChatMessage::Assistant { tools, .. } = self.current_assistant_mut() {
            tools.push(ToolExecution {
                id: tool_id,
                name: tool_name,
                params,
                status: ToolStatus::Running,
                duration: None,
                progress: None,
                error: None,
            });
        }
    }

    pub(super) fn finish_tool_execution(
        &mut self,
        tool_id: &str,
        result: &AgentTraceToolResult,
        duration_ms: u64,
    ) {
        if let Some(tool) = self.find_tool_mut(tool_id) {
            tool.status = if result.is_success() {
                ToolStatus::Success
            } else {
                ToolStatus::Failed
            };
            tool.duration = Some(Duration::from_millis(duration_ms));
            tool.error = result.error_text().map(ToOwned::to_owned);
            tool.progress = None;
        }
    }

    /// End-of-stream reconciliation against the run's authoritative terminal
    /// record.
    ///
    /// The live tool rows are built from `agent_trace`, which the protocol
    /// itself documents as a *deliberately lossy* mirror (bounded mpsc +
    /// `try_send`, drop when full) — and says, in the same doc, that
    /// `RunSummary.tool_summaries` exists precisely so consumers can reconcile
    /// against it at `run_complete`. The TUI is the second consumer of that
    /// mirror and had never been wired to the invariant, so one dropped frame
    /// on a tool-heavy run left a row spinning ⟳ forever with no repair path.
    ///
    /// Rows absent from the live stream are reconstructed, not skipped: the
    /// dropped frame may have been the *start*.
    pub(super) fn reconcile_tools_from_summary(
        &mut self,
        summaries: &[aleph_protocol::events::ToolSummaryItem],
        errors: &[aleph_protocol::events::ToolErrorItem],
    ) {
        if summaries.is_empty() {
            return;
        }
        self.ensure_assistant_message();
        for item in summaries {
            let error_text = errors
                .iter()
                .find(|e| e.tool_id == item.tool_id)
                .map(|e| e.error.clone());
            let status = if item.success {
                ToolStatus::Success
            } else {
                ToolStatus::Failed
            };
            let duration = Some(Duration::from_millis(item.duration_ms));
            if let Some(tool) = self.find_tool_mut(&item.tool_id) {
                tool.status = status;
                tool.duration = duration;
                tool.progress = None;
                if tool.error.is_none() {
                    tool.error = error_text;
                }
                continue;
            }
            if let ChatMessage::Assistant { tools, .. } = self.current_assistant_mut() {
                tools.push(ToolExecution {
                    id: item.tool_id.clone(),
                    name: item.tool_name.clone(),
                    params: String::new(),
                    status,
                    duration,
                    progress: None,
                    error: error_text,
                });
            }
        }
    }

    /// Move every row still `Running` at run end to `Unknown`.
    ///
    /// Reached both after [`Self::reconcile_tools_from_summary`] (a row the
    /// authoritative record does not mention either) and on `RunError`, which
    /// carries no summary at all. Never guesses `Success`: the run is over, so
    /// "still running" is the one thing the row cannot be.
    pub(super) fn settle_orphan_tools(&mut self) {
        for msg in &mut self.messages {
            if let ChatMessage::Assistant { tools, .. } = msg {
                for tool in tools.iter_mut() {
                    if tool.status == ToolStatus::Running {
                        tool.status = ToolStatus::Unknown;
                        tool.progress = None;
                    }
                }
            }
        }
    }

    pub(super) fn mark_current_assistant_complete(&mut self) {
        if let Some(ChatMessage::Assistant { is_streaming, .. }) = self
            .messages
            .iter_mut()
            .rev()
            .find(|m| matches!(m, ChatMessage::Assistant { .. }))
        {
            *is_streaming = false;
        }
    }

    fn update_total_tokens_from_trace(&mut self, total_tokens: usize) {
        let bounded = u64::try_from(total_tokens).unwrap_or(u64::MAX);
        self.total_tokens = self.total_tokens.saturating_add(bounded);
    }

    fn default_trace_presentation(event: &AgentTraceEvent) -> Option<AgentTracePresentation> {
        present_agent_trace_event_with_preset(event, AgentTracePresentationPreset::TuiDebug)
    }

    fn append_trace_debug_entry(
        &mut self,
        event: &AgentTraceEvent,
        presentation: &AgentTracePresentation,
    ) {
        match event {
            // TextEmitted carries the model's verbatim output. Feed the raw
            // text into the user-facing message — the debug presentation would
            // prefix it with "[Final text] iter N:" decoration meant only for
            // a trace/debug panel, not the primary chat content.
            AgentTraceEvent::TextEmitted { stream, text, .. } => match stream {
                AgentTraceTextKind::Intermediate => self.append_reasoning_entry(text.clone()),
                // Append only what streaming has NOT already delivered for this
                // turn. `text` is the turn's full text and the `ResponseChunk`
                // deltas are its prefix, so on a streamed turn this is empty and
                // on a non-streamed turn it is the whole thing. `.get()` (never
                // `&text[..]`) keeps this UTF-8 safe if the counter is ever out
                // of step: an out-of-range or non-boundary index yields None and
                // we append nothing rather than panicking mid-render.
                AgentTraceTextKind::Final => {
                    let fresh = text.get(self.turn_streamed_len..).unwrap_or("");
                    self.append_assistant_content(fresh);
                }
            },
            // ToolSummary carries an agent-authored summary sentence — use it
            // verbatim instead of the "Tool summary: " decorated form.
            AgentTraceEvent::ToolSummary { summary, .. } => {
                self.append_reasoning_entry(summary.clone());
            }
            AgentTraceEvent::TurnStarted { .. }
            | AgentTraceEvent::TurnStateEntered { .. }
            | AgentTraceEvent::TurnCompleted { .. }
            | AgentTraceEvent::SessionCompleted { .. }
            // Goal-loop watchdog veto: surface the interception reason (the
            // presentation renders "checklist incomplete — …") so the user
            // sees why the run was forced to continue.
            | AgentTraceEvent::VerifierVeto { .. }
            // Reactive compaction: a long run that overflowed context, compacted
            // history, and retried. Surface the outcome ("reactive compaction
            // rescued/exhausted") so the run does not look frozen while it
            // self-heals — mirrors the Panel's compaction notice.
            | AgentTraceEvent::ReactiveCompactionAttempted { .. }
            // Cache watchdog alarm: the domain's only automated signal that a
            // stable prefix is churning — must reach the user, not just the
            // log. The presentation carries streak/read/write/attribution.
            | AgentTraceEvent::CacheHealthDegraded { .. } => {
                self.append_reasoning_entry(presentation.content.clone());
            }
            // Tool-call lifecycle is rendered by ToolStart/ToolEnd gateway events;
            // observability passthrough variants have no TUI rendering.
            // (ProviderUsage feeds the status-bar cache stat in the state
            // match below — it has no presentation, so it never reaches this
            // debug-entry dispatch anyway.)
            AgentTraceEvent::ToolCallStarted { .. }
            | AgentTraceEvent::ToolCallCompleted { .. }
            | AgentTraceEvent::WorktreeCreated { .. }
            | AgentTraceEvent::WorktreeCleanedUp { .. }
            | AgentTraceEvent::McpScopeAttached { .. }
            | AgentTraceEvent::McpScopeCleaned { .. }
            | AgentTraceEvent::ProviderUsage { .. }
            // MoaTurnTrace is persisted-only (no live wire, no TUI replay).
            | AgentTraceEvent::MoaTurnTrace { .. } => {}
            // MoA fan-out moments render as reasoning entries — presentation
            // already carries the error/cached/billed forms (round-2 W2).
            AgentTraceEvent::MoaAdvisor { .. }
            | AgentTraceEvent::MoaAggregating { .. }
            | AgentTraceEvent::MoaAdvisorSpend { .. } => {
                self.append_reasoning_entry(presentation.content.clone());
            }
        }
    }

    pub(super) fn apply_agent_trace_event(&mut self, event: &AgentTraceEvent) -> Action {
        // New turn ⇒ a new `TextEmitted{Final}` payload is coming, so the
        // streamed-prefix watermark restarts. Must run before the debug entry
        // below, which is where the previous turn's final text was appended.
        if matches!(event, AgentTraceEvent::TurnStarted { .. }) {
            self.turn_streamed_len = 0;
        }
        let presentation = Self::default_trace_presentation(event);
        if let Some(presentation) = &presentation {
            self.append_trace_debug_entry(event, presentation);
        }

        match event {
            AgentTraceEvent::TextEmitted { .. } => Action::ScrollToBottomIfAutoScroll,
            AgentTraceEvent::ToolCallStarted { call, .. } => {
                self.start_tool_execution(
                    call.tool_id.clone(),
                    call.tool_name.clone(),
                    summarize_tool_input(
                        &call.input,
                        AgentTracePresentationPreset::TuiDebug.options(),
                    ),
                );
                Action::ScrollToBottomIfAutoScroll
            }
            AgentTraceEvent::ToolCallCompleted { call, result, .. } => {
                self.finish_tool_execution(&call.tool_id, result, call.duration_ms);
                Action::ScrollToBottomIfAutoScroll
            }
            AgentTraceEvent::ToolSummary { summary, .. } => {
                let _ = summary;
                Action::ScrollToBottomIfAutoScroll
            }
            AgentTraceEvent::SessionCompleted {
                total_tokens,
                final_text,
                ..
            } => {
                if let Some(text) = final_text {
                    let needs_final_text = !matches!(
                        self.messages.iter().rev().find_map(|msg| match msg {
                            ChatMessage::Assistant { content, .. } => Some(!content.is_empty()),
                            _ => None,
                        }),
                        Some(true)
                    );

                    if needs_final_text {
                        self.append_assistant_content(text);
                    }
                }

                if !self.current_run_trace_summary_applied {
                    self.update_total_tokens_from_trace(*total_tokens);
                    self.current_run_trace_summary_applied = true;
                }
                self.current_run = None;
                self.run_started_at = None;
                self.dismiss_pending_approval();
                self.current_run_uses_agent_trace = false;
                self.mark_current_assistant_complete();
                Action::ScrollToBottomIfAutoScroll
            }
            // Live per-call cache telemetry → status-bar cache stat. Only
            // calls that actually report cache activity update it, so
            // providers without prompt caching never surface a misleading 0%.
            // The percentage comes from the single canonical formula
            // (`aleph_protocol::cache_hit_ratio`, read / (input + read)) — the
            // same one the core rollup and Panel Usage use. This arm used to
            // recompute with `input + creation + read` as the denominator, so
            // the status bar read systematically lower than the Panel for the
            // very same call.
            AgentTraceEvent::ProviderUsage {
                agent_id,
                input_tokens,
                cache_read_tokens,
                cache_creation_tokens,
                ..
            } => {
                // The first reporting agent in a session is its root: an agent
                // must take a turn before it can delegate, so nothing else can
                // report first.
                if self.cache_root_agent.is_none() {
                    self.cache_root_agent = Some(agent_id.clone());
                }
                let read = u64::from(cache_read_tokens.unwrap_or(0));
                let creation = u64::from(cache_creation_tokens.unwrap_or(0));
                if read > 0 || creation > 0 {
                    // `unwrap_or(0.0)` covers "creation reported, read not": a
                    // pure cold write is a 0% call, not an unknown one.
                    let ratio = aleph_protocol::cache_hit_ratio(
                        u64::from(*input_tokens),
                        cache_read_tokens.map(u64::from),
                    )
                    .unwrap_or(0.0);
                    self.cache_stat = Some((ratio * 100.0).round() as u64);
                    // Label the reading whenever it is not the root agent's —
                    // sub-agents and MoA advisors share this stream, and their
                    // cold starts would otherwise read as the root agent's
                    // prefix breaking.
                    self.cache_stat_agent = match self.cache_root_agent.as_deref() {
                        Some(root) if root == agent_id => None,
                        _ => Some(agent_id.clone()),
                    };
                }
                Action::None
            }
            _ => Action::None,
        }
    }

    pub fn load_trace_replay(&mut self, replay: &AgentTraceReplay) {
        let summary = format!(
            "Loaded replay {} from session {} [{}] via {}.",
            replay.task.task_id, replay.task.session_id, replay.task.status, replay.task.agent_id
        );

        self.messages.clear();
        // The cache is keyed by positional index into `messages`, which is
        // about to be repopulated from scratch — a stale entry whose (kind,
        // len, width) happens to match new content at the same index must
        // not survive.
        self.chat_line_cache = crate::tui::widgets::chat_area::LineCache::default();
        self.current_run = Some(replay.task.task_id.clone());
        // Replay is not a live run — keep the working indicator off even though
        // current_run is briefly Some for projection bookkeeping.
        self.run_started_at = None;
        self.current_run_uses_agent_trace = true;
        self.dialog = None;
        self.palette = None;
        self.approval = None;
        self.focus = Focus::Input;
        self.scroll_to_bottom();
        self.add_system_message(summary);

        for trace in &replay.traces {
            let _ = self.apply_agent_trace_event(&trace.event);
        }

        if replay.traces.is_empty() {
            self.add_system_message("Replay has no structured trace events.".to_string());
        }

        self.current_run = None;
        self.current_run_uses_agent_trace = false;
        self.current_run_trace_summary_applied = false;
        self.mark_current_assistant_complete();
    }
}
