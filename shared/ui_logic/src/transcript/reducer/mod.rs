//! The fold. Live frames (`apply_live`) and replay rows (`apply_replay`,
//! then `finish_replay`) become the same `Vec<TranscriptEntry>` through one
//! internal `apply_trace` — the leg-independent core every
//! `AgentTraceEvent` goes through, whichever leg delivered it. That is the
//! "two legs, one derivation" rule of TRANSCRIPT_RENDERING §2, made
//! structural; G2 (`the_live_leg_and_the_replay_leg_fold_to_the_same_entries`)
//! pins it modulo clocks.
//!
//! Pure over its inputs: the wall clock and the run's frames are arguments;
//! the entry-id counter is internal and deterministic. It is written to
//! replace the TUI's `app/trace.rs` projection and the Panel's `begin_step`
//! / `set_step_text` family (Phases T and P); the rules below are theirs,
//! ported. Nothing renders it yet.
//!
//! What it does NOT decide: cost, context gauge, plan snapshots, halt
//! wording (locale) — the client reads those from the same frame.
//!
//! Split by responsibility, not by leg (the legs share almost everything):
//! `step_ops` owns the open step, `tool_rows` the rows both tool faces
//! settle, `run_end` what a run's end does on either leg.

mod run_end;
mod step_ops;
mod tool_rows;

use aleph_protocol::trace_presentation::{
    present_agent_trace_event_with_preset, AgentTracePresentationPreset,
};
use aleph_protocol::{AgentTraceEvent, AgentTraceSessionOutcome, StreamEvent};

use super::step::{NoteKind, StepStatus};
use super::view_model::{TranscriptEntry, TuiAttachment};
use step_ops::RecordCursor;
pub use tool_rows::trace_result_to_wire;

/// What a fold step changed, by entry id — enough for a keyed re-render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Inserted(String),
    Updated(String),
    Removed(String),
    /// The fold cannot vouch for what it holds. Raised when
    /// `RunSummary.loops` counted more iterations than this transcript saw
    /// `TurnStarted` frames for (something was dropped on the way), or when a
    /// thinking record names an iteration other than the open step's. The
    /// entries are NOT patched — no step is minted to hold the frame and
    /// none is renumbered.
    ///
    /// The client's move: discard the whole transcript and rebuild it from
    /// the session's replay — the user messages from the session log, then
    /// each run's `trace.by_runs` rows through `apply_replay` and
    /// `finish_replay` (spec §5.1, §9). A resync raised mid-run keeps what
    /// is shown, marked stale, until the run ends. Phase T implements that
    /// client side; this type offers no replace-run surface. The other
    /// producers are fail-closed branches no input reaches today.
    NeedsResync,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct RunState {
    run_id: String,
    /// Index into `entries` where this run's entries begin. A transcript
    /// holds a session's runs; tool lookup, orphan settling and the run-end
    /// trailers see only this run's entries. Nothing before it is ever
    /// removed while the run is open.
    first_entry: usize,
    /// Index into `entries` of the `Live` step, if any.
    open_step: Option<usize>,
    /// Deltas vs records for the open step's thinking and text.
    thinking: RecordCursor,
    text: RecordCursor,
    /// `TurnStarted` frames seen — compared with `RunSummary.loops`.
    steps_seen: u32,
    /// Whether any step of THIS run holds a `TextEmitted` record. Run-scoped
    /// (never cleared by `reset_step_state`): it gates the final-text fill,
    /// which must not repeat an answer an earlier step already shows
    /// (ruling T910-N1b).
    text_recorded: bool,
    /// Replay leg only: whether a `SessionCompleted` row arrived, and its
    /// wall clock. `finish_replay` reads both; the live leg ends on
    /// `RunComplete` instead.
    session_completed: bool,
    session_duration_ms: Option<u64>,
}

impl RunState {
    fn starting_at(first_entry: usize, run_id: String) -> Self {
        Self {
            run_id,
            first_entry,
            ..Self::default()
        }
    }

    /// The per-step bookkeeping, cleared whenever the open step changes.
    fn reset_step_state(&mut self) {
        self.thinking = RecordCursor::default();
        self.text = RecordCursor::default();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Transcript {
    entries: Vec<TranscriptEntry>,
    next_id: u64,
    run: Option<RunState>,
}

impl Transcript {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn entries(&self) -> &[TranscriptEntry] {
        &self.entries
    }

    fn next_id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}-{}", self.next_id)
    }

    pub fn push_user(
        &mut self,
        text: &str,
        at_ms: Option<u64>,
        attachments: Vec<TuiAttachment>,
    ) -> Change {
        let id = self.next_id("user");
        self.entries.push(TranscriptEntry::UserText {
            id: id.clone(),
            text: text.to_string(),
            at_ms,
            attachments,
        });
        Change::Inserted(id)
    }

    // ---- live leg --------------------------------------------------------

    pub fn apply_live(&mut self, ev: &StreamEvent, now_ms: u64) -> Vec<Change> {
        if let StreamEvent::RunAccepted { run_id, .. } = ev {
            let changes = self.abandon_open_run(now_ms);
            self.run = Some(RunState::starting_at(self.entries.len(), run_id.clone()));
            return changes;
        }
        // Frames of a run this transcript was not told about (another run,
        // or a client that attached after `RunAccepted`) are not folded:
        // attaching mid-run is a `trace.by_runs` pull, not a live fold.
        let mine = self.run.as_ref().is_some_and(|r| r.run_id == ev.run_id());
        if !mine {
            return Vec::new();
        }
        let now = Some(now_ms);
        match ev {
            StreamEvent::Reasoning { content, .. }
            | StreamEvent::ReasoningBlock { content, .. } => self.append_thinking(content, now),
            StreamEvent::ResponseChunk { content, .. } => self.append_text(content, now),
            StreamEvent::ToolStart {
                tool_id,
                tool_name,
                params,
                ..
            } => self.start_tool(tool_id, tool_name, params, now),
            StreamEvent::ToolUpdate {
                tool_id, progress, ..
            } => self.update_tool(tool_id, progress),
            StreamEvent::ToolEnd {
                tool_id,
                result,
                duration_ms,
                ..
            } => self.finish_tool(tool_id, None, result, *duration_ms, now),
            StreamEvent::AgentTrace { event, .. } => self.apply_trace(event, now),
            StreamEvent::RunComplete {
                summary,
                total_duration_ms,
                ..
            } => self.complete_run(summary, *total_duration_ms, now),
            StreamEvent::RunError { error, .. } => self.fail_run(error, now),
            StreamEvent::RunAccepted { .. }
            | StreamEvent::RunQueued { .. }
            | StreamEvent::AskUser { .. }
            | StreamEvent::ClarificationEnded { .. }
            | StreamEvent::UncertaintySignal { .. }
            | StreamEvent::RunRetrying { .. }
            | StreamEvent::ModelResolved { .. }
            | StreamEvent::ContextGauge { .. }
            | StreamEvent::SessionUserMessage { .. } => Vec::new(),
        }
    }

    // ---- replay leg ------------------------------------------------------

    /// One `trace.by_runs` row. The same `apply_trace` the live leg uses;
    /// the only difference is the absent clock. Call `finish_replay` after
    /// the run's last row.
    pub fn apply_replay(&mut self, ev: &AgentTraceEvent) -> Vec<Change> {
        let first_entry = self.entries.len();
        let run = self
            .run
            .get_or_insert_with(|| RunState::starting_at(first_entry, String::new()));
        if let AgentTraceEvent::SessionCompleted { duration_ms, .. } = ev {
            // Remembered for the trailer; `finish_replay` reads it back
            // after the last row.
            run.session_completed = true;
            run.session_duration_ms = *duration_ms;
        }
        self.apply_trace(ev, None)
    }

    /// After the last row: what `complete_run` does for a live run, with
    /// what a log can honestly say. No `tool_summaries` exist on this leg —
    /// the run's rows ARE the record — and a run whose log has no
    /// `SessionCompleted` ends `Pending` (unknown), with no answer hoisted.
    pub fn finish_replay(&mut self) -> Vec<Change> {
        let (completed, duration_ms) = self.run.as_ref().map_or((false, None), |r| {
            (r.session_completed, r.session_duration_ms)
        });
        let mut changes = self.settle_orphans();
        if completed {
            changes.extend(self.hoist_open_step_text(None));
            let rows = self.run_rows();
            changes.extend(self.push_trailers(&rows, duration_ms));
        } else {
            changes.extend(self.close_open_step(StepStatus::Pending, None));
        }
        self.run = None;
        changes
    }

    // ---- the shared leg --------------------------------------------------

    /// Every `AgentTraceEvent` ends here, whichever leg delivered it.
    /// `now_ms` is `None` when the caller has no clock: rows then carry
    /// none, only the durations the wire recorded.
    fn apply_trace(&mut self, ev: &AgentTraceEvent, now_ms: Option<u64>) -> Vec<Change> {
        if self.run.is_none() {
            // Every caller opens a run first; an event outside one has no
            // step it could belong to.
            return vec![Change::NeedsResync];
        }
        match ev {
            AgentTraceEvent::TurnStarted { iteration } => {
                self.start_turn(u32::try_from(*iteration).ok(), now_ms)
            }
            AgentTraceEvent::ReasoningEmitted { iteration, text } => {
                self.record_thinking(*iteration, text, now_ms)
            }
            AgentTraceEvent::TextEmitted { text, .. } => self.record_text(text, now_ms),
            AgentTraceEvent::ToolCallStarted { call, .. } => {
                self.start_tool(&call.tool_id, &call.tool_name, &call.input, now_ms)
            }
            AgentTraceEvent::ToolCallCompleted { call, result, .. } => {
                let wire = trace_result_to_wire(result, call.presentation.as_ref());
                self.finish_tool(
                    &call.tool_id,
                    Some(&call.tool_name),
                    &wire,
                    call.duration_ms,
                    now_ms,
                )
            }
            AgentTraceEvent::ToolSummary { .. } => self.note(ev, NoteKind::ToolSummary, now_ms),
            AgentTraceEvent::VerifierVeto { .. } => self.note(ev, NoteKind::VerifierVeto, now_ms),
            AgentTraceEvent::ReactiveCompactionAttempted { .. } => {
                self.note(ev, NoteKind::ReactiveCompaction, now_ms)
            }
            AgentTraceEvent::MoaAdvisor { .. } => self.note(ev, NoteKind::MoaAdvisor, now_ms),
            AgentTraceEvent::MoaAggregating { .. } => {
                self.note(ev, NoteKind::MoaAggregating, now_ms)
            }
            AgentTraceEvent::MoaAdvisorSpend { .. } => {
                self.note(ev, NoteKind::MoaAdvisorSpend, now_ms)
            }
            // Run-scoped, never evidence of an iteration: a subagent's
            // metering still reports into the parent's chain, so this may
            // come from a child (see `MULTI_AGENT_SYSTEM.md`).
            AgentTraceEvent::CacheHealthDegraded { .. } => match presentation_text(ev) {
                Some(text) => vec![self.push_notice(text)],
                None => Vec::new(),
            },
            // Reached on the replay leg only: the live sink never publishes
            // this event (`is_step_event`). It fills only for an outcome whose
            // LIVE run end is established to carry the same final text
            // (ruling T910-NEW1), traced per variant by reading the server:
            //
            // - `Completed` / `HitLimit`: `src/harness/agent.rs` breaks
            //   `Ok(..)` and emits this event on the `Ok` arm;
            //   `src/orchestrator/harness_bridge/runner_impl.rs` then
            //   broadcasts `Complete` (`on_complete_with_outcome`) with the
            //   run-bounded final-text scan, and
            //   `src/gateway/execution_engine/helpers.rs` forwards it as the
            //   `RunComplete` whose `final_response` the live leg fills
            //   from. FILLS.
            // - `Failed` (any `HarnessError` but `Cancelled`, agent.rs `Err`
            //   arm): runner_impl broadcasts `Complete` on both arms, but
            //   helpers.rs then sends one of three things: the held real
            //   `RunComplete`; nothing (`may_retry` + transient: superseded
            //   by the retry); or the synthetic `pre_outcome_summary`
            //   `RunComplete` whose `final_response` is the error receipt.
            //   `execution_engine/execute.rs` follows with `RunError`. The
            //   live final text is not established. EXCLUDED.
            // - `Cancelled` (`HarnessError::Cancelled`): the helpers.rs drain
            //   returns before `Complete` when it sees the cancel token first,
            //   so the live terminal is the real frame or the synthetic
            //   receipt by race. Not established. EXCLUDED.
            AgentTraceEvent::SessionCompleted {
                outcome,
                final_text,
                ..
            } => match outcome {
                AgentTraceSessionOutcome::Completed | AgentTraceSessionOutcome::HitLimit => {
                    self.record_final_text(final_text.as_deref(), now_ms)
                }
                AgentTraceSessionOutcome::Failed | AgentTraceSessionOutcome::Cancelled => {
                    Vec::new()
                }
            },
            // `ProviderUsage` too may come from a child's metering: accounting,
            // not a step.
            AgentTraceEvent::TurnStateEntered { .. }
            | AgentTraceEvent::TurnCompleted { .. }
            | AgentTraceEvent::WorktreeCreated { .. }
            | AgentTraceEvent::WorktreeCleanedUp { .. }
            | AgentTraceEvent::McpScopeAttached { .. }
            | AgentTraceEvent::McpScopeCleaned { .. }
            | AgentTraceEvent::ProviderUsage { .. }
            | AgentTraceEvent::MoaTurnTrace { .. } => Vec::new(),
        }
    }
}

/// One derivation of the loop's narration text, shared with the TUI's
/// debug entries: the protocol's own presenter.
fn presentation_text(ev: &AgentTraceEvent) -> Option<String> {
    present_agent_trace_event_with_preset(ev, AgentTracePresentationPreset::TuiDebug)
        .map(|p| p.content)
}

#[cfg(test)]
mod tests;
