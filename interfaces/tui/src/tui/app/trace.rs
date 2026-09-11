//! `AgentTrace` projection: maps `AgentTraceEvent`s (live and replayed) onto
//! chat/tool/reasoning state.
//!
//! Pulled out of [`mod`] to keep it under the 1 kLOC soft cap, as a third
//! `impl AppState` block sibling to [`super::events`] (the `StreamEvent`
//! projection) — the two projection paths now sit side by side.

use aleph_protocol::{
    present_agent_trace_event_with_preset, AgentTraceEvent, AgentTracePresentation,
    AgentTracePresentationPreset, AgentTraceReplay, AgentTraceTextKind, AgentTraceToolResult,
};

use super::{Action, AppState, Focus, RowBody, ToolRow, TranscriptEntry};

/// One trace tool-end, as the wire result shape the view model consumes.
///
/// The trace event splits what the wire keeps together: the outcome is in
/// `AgentTraceToolResult`, the `presentation` side-channel is on
/// `AgentTraceToolCallEnd` beside it. Joining them here — rather than at each
/// call site — is what stops the diff from being dropped on this path while
/// the live `tool_end` path carries it (判据 §9: one verb, two faces, one
/// derivation).
fn trace_result_to_wire(
    result: &AgentTraceToolResult,
    presentation: Option<&aleph_protocol::file_change::Presentation>,
) -> aleph_protocol::ToolResult {
    let (success, output, error) = match result {
        AgentTraceToolResult::Success { output } => (
            true,
            match output {
                serde_json::Value::String(s) => Some(s.clone()),
                serde_json::Value::Null => None,
                other => Some(other.to_string()),
            },
            None,
        ),
        AgentTraceToolResult::Error { error, .. } => (false, None, Some(error.clone())),
    };
    aleph_protocol::ToolResult {
        success,
        output,
        error,
        presentation: presentation.cloned(),
    }
}

impl AppState {
    /// Append to the trailing reasoning entry, or start one.
    ///
    /// Reasoning is its own chronological entry now rather than a field on the
    /// assistant message. That is what lets it sit where it happened — before
    /// the text it led to, and before any tool the model reached for while
    /// thinking — instead of always above the whole turn.
    pub(super) fn append_reasoning_entry(&mut self, content: String) {
        self.push_reasoning(&content, true);
    }

    /// A streaming reasoning chunk: joined with no separator, because the
    /// pieces are halves of one sentence.
    ///
    /// The block form above is the other carrier of the same text and takes a
    /// newline, because its pieces are whole thoughts. Two callers, one place
    /// that decides where reasoning lives.
    pub(super) fn append_reasoning_chunk(&mut self, content: &str) {
        self.push_reasoning(content, false);
    }

    fn push_reasoning(&mut self, content: &str, separate: bool) {
        if let Some(TranscriptEntry::Reasoning { text, .. }) = self.messages.last_mut() {
            if separate && !text.is_empty() {
                text.push('\n');
            }
            text.push_str(content);
            return;
        }
        let id = self.next_entry_id();
        self.messages.push(TranscriptEntry::Reasoning {
            id,
            text: content.to_string(),
            collapsed: true,
        });
    }

    pub(crate) fn append_assistant_content(&mut self, content: &str) {
        if content.is_empty() {
            return;
        }

        // The one place both carriers of assistant text converge, which is why
        // the "did this run say anything" flag is set here rather than at each
        // caller — see `run_rendered_assistant_text`.
        self.run_rendered_assistant_text = true;
        self.ensure_assistant_message();
        if let TranscriptEntry::AssistantText { markdown, .. } = self.current_assistant_mut() {
            markdown.push_str(content);
        }
    }

    /// Open (or re-open) a tool row.
    ///
    /// `args` is the RAW call input, not a pre-rendered string: the row's
    /// header comes from `transcript::summarize`, which needs the structure to
    /// produce `Read(src/lib.rs:1-120)` rather than a JSON dump. The trace
    /// debug view keeps its own `summarize_tool_input` — that one exists to
    /// show a call verbatim, which is a different question with a different
    /// right answer.
    pub(crate) fn start_tool_execution(
        &mut self,
        tool_id: String,
        tool_name: String,
        args: &serde_json::Value,
    ) {
        let now = self.now_ms();
        if let Some(row) = self.find_tool_mut(&tool_id) {
            *row = ToolRow::new(tool_id, tool_name, args);
            row.start(now);
            return;
        }
        let mut row = ToolRow::new(tool_id, tool_name, args);
        row.start(now);
        self.messages.push(TranscriptEntry::Tool(row));
    }

    /// Settle a tool row from the wire result.
    ///
    /// Takes `aleph_protocol::ToolResult` rather than the trace's own
    /// `AgentTraceToolResult` because that is the shape carrying
    /// `presentation` — the structured `FileChange` the server computed. The
    /// trace path has it too, on `AgentTraceToolCallEnd` beside the result
    /// rather than inside it, and converting there is what keeps this from
    /// being a function that silently cannot receive a diff.
    pub(super) fn finish_tool_execution(
        &mut self,
        tool_id: &str,
        result: &aleph_protocol::ToolResult,
        duration_ms: u64,
    ) {
        let now = self.now_ms();
        if let Some(row) = self.find_tool_mut(tool_id) {
            row.finish(result, duration_ms, now);
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
        let now = self.now_ms();
        for item in summaries {
            let error_text = errors
                .iter()
                .find(|e| e.tool_id == item.tool_id)
                .map(|e| e.error.clone());
            let wire = aleph_protocol::ToolResult {
                success: item.success,
                // The authoritative record carries no output, so a row it
                // RECONSTRUCTS has no body — and a row that already has one
                // keeps it, because `finish` only overwrites from the result
                // it is handed. Reconstruction below therefore lands on a
                // header-only row, which is honest: this path knows the call
                // happened and how it ended, not what it printed.
                output: None,
                error: error_text,
                presentation: None,
            };
            if let Some(row) = self.find_tool_mut(&item.tool_id) {
                // Keep whatever body the live stream did deliver.
                let body = row.body.clone();
                row.finish(&wire, item.duration_ms, now);
                if matches!(row.body, RowBody::None) {
                    row.body = body;
                }
                continue;
            }
            let mut row = ToolRow::new(
                item.tool_id.clone(),
                item.tool_name.clone(),
                &serde_json::Value::Null,
            );
            row.finish(&wire, item.duration_ms, now);
            self.messages.push(TranscriptEntry::Tool(row));
        }
    }

    /// Settle every row still `Running` at run end.
    ///
    /// Reached both after [`Self::reconcile_tools_from_summary`] (a row the
    /// authoritative record does not mention either) and on `RunError`, which
    /// carries no summary at all. `ToolRow::settle_resumed` is the shared rule
    /// and it moves `Running` to `Pending` — "unknown", never a fabricated
    /// success, and never a spinner that keeps turning after the run is over.
    pub(super) fn settle_orphan_tools(&mut self) {
        for entry in &mut self.messages {
            if let TranscriptEntry::Tool(row) = entry {
                row.settle_resumed();
            }
        }
    }

    pub(super) fn mark_current_assistant_complete(&mut self) {
        if let Some(TranscriptEntry::AssistantText { streaming, .. }) = self
            .messages
            .iter_mut()
            .rev()
            .find(|m| matches!(m, TranscriptEntry::AssistantText { .. }))
        {
            *streaming = false;
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
                    &call.input,
                );
                Action::ScrollToBottomIfAutoScroll
            }
            AgentTraceEvent::ToolCallCompleted { call, result, .. } => {
                // Live plan projection (`ScratchpadOutput.snapshot` rides the
                // tool result). `maybe_apply_plan_from_tool` no-ops during a
                // trace REPLAY — a historical run's checklist must not
                // overwrite the live conversation's.
                if let AgentTraceToolResult::Success { output } = result {
                    self.maybe_apply_plan_from_tool(&call.tool_name, output);
                }
                let wire = trace_result_to_wire(result, call.presentation.as_ref());
                self.finish_tool_execution(&call.tool_id, &wire, call.duration_ms);
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
                            TranscriptEntry::AssistantText { markdown, .. } =>
                                Some(!markdown.is_empty()),
                            _ => None,
                        }),
                        Some(true)
                    );

                    if needs_final_text {
                        self.append_assistant_content(text);
                    }
                }

                if !self.current_run_trace_summary_applied && !self.replaying_trace {
                    self.update_total_tokens_from_trace(*total_tokens);
                    self.current_run_trace_summary_applied = true;
                }
                self.current_run = None;
                self.run_started_at = None;
                // The replay must not tear down UI overlays the live run
                // owns — an open /btw overlay would be dissolved by its own
                // historical trace landing, which is confusing. The dismissal
                // runs only on the live path.
                if !self.replaying_trace {
                    self.dismiss_pending_approval();
                }
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

        // Replay must not move the live run's status-bar counters.
        // `apply_agent_trace_event` is shared with the live path, which uses
        // its `SessionCompleted` / `ProviderUsage` arms to bump `total_tokens`
        // and `cache_stat` — those side effects are correct for a finished
        // run that just ended, but the replay is showing a historical run
        // whose accounting has nothing to do with the conversation on
        // screen. Save/restore is the cheap fix; toggling a flag is the
        // localised one. We do both: the flag is the runtime guard (cheap,
        // branch), the save/restore is the safety net in case a future arm
        // touches another counter without remembering to check the flag.
        let saved_total = self.total_tokens;
        let saved_cache_stat = self.cache_stat;
        let saved_cache_agent = self.cache_stat_agent.clone();
        let saved_cache_root = self.cache_root_agent.clone();
        self.replaying_trace = true;
        for trace in &replay.traces {
            let _ = self.apply_agent_trace_event(&trace.event);
        }
        self.replaying_trace = false;
        self.total_tokens = saved_total;
        self.cache_stat = saved_cache_stat;
        self.cache_stat_agent = saved_cache_agent;
        self.cache_root_agent = saved_cache_root;

        if replay.traces.is_empty() {
            self.add_system_message("Replay has no structured trace events.".to_string());
        }

        self.current_run = None;
        self.current_run_uses_agent_trace = false;
        self.current_run_trace_summary_applied = false;
        self.mark_current_assistant_complete();
    }
}
