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

use aleph_protocol::trace_presentation::{
    present_agent_trace_event_with_preset, AgentTracePresentationPreset,
};
use aleph_protocol::{AgentTraceEvent, AgentTraceToolResult, RunSummary, StreamEvent, ToolResult};

use super::affordance::worked_for;
use super::step::{Note, NoteKind, StepEntry, StepStatus, ThinkingBlock};
use super::turn_summary::summarize_turn;
use super::view_model::{RowBody, RowStatus, ToolRow, TranscriptEntry, TuiAttachment};

/// Between two records of one field on one iteration (the verifier-halt
/// salvage path re-runs Think on the same iteration and records again), and
/// before a delta segment that follows a record.
const RECORD_JOIN: &str = "\n\n";

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
    /// none is renumbered. The client's move is to rebuild through the
    /// replay leg: a fresh `Transcript`, each `trace.by_runs` row through
    /// `apply_replay`, then `finish_replay` per run (spec §5.1, §9). The
    /// other producers are fail-closed branches no input reaches today.
    NeedsResync,
}

/// One field of the open step (its thinking, or its text) fed by two
/// sources: streamed deltas, and the authoritative records the trace later
/// writes for them (`ReasoningEmitted`, `TextEmitted`). A record replaces
/// every delta streamed since the previous record and is appended after any
/// earlier record of the same step — so the live leg converges to exactly
/// what the replay leg (records only) shows, whatever the deltas were.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct RecordCursor {
    /// Bytes at the head of the field that came from records.
    record_len: usize,
    /// Whether deltas were appended since the last record.
    streamed: bool,
}

impl RecordCursor {
    /// What a delta is prefixed with: a delta segment that follows a record
    /// starts a new paragraph, joined the way the record covering it will be.
    const fn delta_prefix(self) -> &'static str {
        if self.record_len > 0 && !self.streamed {
            RECORD_JOIN
        } else {
            ""
        }
    }

    /// `current`'s records followed by `record`, the deltas after them
    /// dropped. `None` when `record_len` is not a boundary of `current`:
    /// something else rewrote the field and the earlier records are unknown.
    fn fold(self, current: &str, record: &str) -> Option<String> {
        let prior = current.get(..self.record_len)?;
        Some(if prior.is_empty() {
            record.to_string()
        } else {
            format!("{prior}{RECORD_JOIN}{record}")
        })
    }
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
    /// Whether any assistant text was rendered this run (gates the
    /// `final_response` fallback at `RunComplete`).
    text_rendered: bool,
    /// `TurnStarted` frames seen — compared with `RunSummary.loops`.
    steps_seen: u32,
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

    /// A new run was accepted while the previous one never reported its end
    /// (a dropped terminal frame, a reconnect). How it ended is unknown: its
    /// rows stop spinning and its open step closes `Pending`, never `Live`
    /// forever and never a fabricated `Settled`.
    fn abandon_open_run(&mut self, now_ms: u64) -> Vec<Change> {
        if self.run.is_none() {
            return Vec::new();
        }
        let mut changes = self.settle_orphans();
        changes.extend(self.close_open_step(StepStatus::Pending, Some(now_ms)));
        self.run = None;
        changes
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
            AgentTraceEvent::SessionCompleted { final_text, .. } => {
                let has_text = self
                    .open_step_ref()
                    .is_some_and(|s| s.text.as_deref().is_some_and(|t| !t.trim().is_empty()));
                match final_text.as_deref().filter(|t| !t.trim().is_empty()) {
                    Some(t) if !has_text => self.append_text(t, now_ms),
                    _ => Vec::new(),
                }
            }
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

    // ---- step bookkeeping ------------------------------------------------

    fn open_step_ref(&self) -> Option<&StepEntry> {
        let idx = self.run.as_ref()?.open_step?;
        match self.entries.get(idx) {
            Some(TranscriptEntry::Step(s)) => Some(s),
            _ => None,
        }
    }

    /// The ONE way a frame reaches the open step for writing (PF-C5/C10).
    /// `None` = no run, no open step, or a recorded index that no longer
    /// names a step.
    fn open_step_mut(&mut self) -> Option<&mut StepEntry> {
        let idx = self.run.as_ref()?.open_step?;
        match self.entries.get_mut(idx) {
            Some(TranscriptEntry::Step(s)) => Some(s),
            _ => None,
        }
    }

    /// Push a new `Live` step, make it the run's open step and return its
    /// index. `None` when there is no run to own it — nothing is pushed.
    fn open_step(&mut self, iteration: Option<u32>, now_ms: Option<u64>) -> Option<usize> {
        self.run.as_ref()?;
        let id = self.next_id("step");
        self.entries.push(TranscriptEntry::Step(StepEntry {
            id,
            iteration,
            thinking: None,
            text: None,
            tools: Vec::new(),
            notes: Vec::new(),
            status: StepStatus::Live,
            started_ms: now_ms,
            ended_ms: None,
        }));
        let idx = self.entries.len() - 1;
        let run = self.run.as_mut()?;
        run.open_step = Some(idx);
        run.reset_step_state();
        Some(idx)
    }

    /// `Inserted` for the step `open_step` just opened at `idx`.
    fn inserted_step(&self, idx: Option<usize>) -> Change {
        match idx.and_then(|i| self.entries.get(i)) {
            Some(TranscriptEntry::Step(s)) => Change::Inserted(s.id.clone()),
            _ => Change::NeedsResync,
        }
    }

    /// Open an UNNUMBERED step when the run has none — a frame that arrives
    /// before any `TurnStarted` is real and must land somewhere, but never
    /// on a step number the server did not say. A run that already records
    /// an open step is left alone: whether that index still reaches a step
    /// is `open_step_mut`'s question.
    fn ensure_open_step(&mut self, now_ms: Option<u64>) -> Vec<Change> {
        match self.run.as_ref().map(|r| r.open_step) {
            Some(Some(_)) => Vec::new(),
            Some(None) => {
                let idx = self.open_step(None, now_ms);
                vec![self.inserted_step(idx)]
            }
            None => vec![Change::NeedsResync],
        }
    }

    /// Apply `edit` to the open step (opening an unnumbered one if the run
    /// has none). `edit` returns whether it changed the step. A miss — no
    /// step reachable, or an edit that could not apply — is `NeedsResync`,
    /// never a silently minted step and never an `Updated` for a step that
    /// did not change. The flag says whether the edit applied.
    fn edit_open_step(
        &mut self,
        now_ms: Option<u64>,
        edit: impl FnOnce(&mut StepEntry) -> bool,
    ) -> (Vec<Change>, bool) {
        let mut changes = self.ensure_open_step(now_ms);
        let updated = self
            .open_step_mut()
            .and_then(|step| edit(step).then(|| step.id.clone()));
        match updated {
            Some(id) => {
                changes.push(Change::Updated(id));
                (changes, true)
            }
            None => {
                if !changes.contains(&Change::NeedsResync) {
                    changes.push(Change::NeedsResync);
                }
                (changes, false)
            }
        }
    }

    fn start_turn(&mut self, iteration: Option<u32>, now_ms: Option<u64>) -> Vec<Change> {
        let closing = self.run.as_ref().and_then(|r| r.open_step);
        let mut changes = self.close_open_step(StepStatus::Settled, now_ms);
        if let Some(idx) = closing {
            // An iteration that produced nothing is not a step (spec §9) —
            // dropped here, where it is provably over.
            changes.extend(self.drop_step_if_empty(idx));
        }
        let idx = self.open_step(iteration, now_ms);
        changes.push(self.inserted_step(idx));
        if let Some(r) = self.run.as_mut() {
            r.steps_seen += 1;
        }
        changes
    }

    fn close_open_step(&mut self, status: StepStatus, now_ms: Option<u64>) -> Vec<Change> {
        if self.run.as_ref().and_then(|r| r.open_step).is_none() {
            return Vec::new();
        }
        let changes = match self.open_step_mut() {
            Some(step) => {
                step.status = status;
                step.ended_ms = now_ms;
                if let Some(t) = step.thinking.as_mut() {
                    t.streaming = false;
                }
                vec![Change::Updated(step.id.clone())]
            }
            None => vec![Change::NeedsResync],
        };
        if let Some(r) = self.run.as_mut() {
            r.open_step = None;
            r.reset_step_state();
        }
        changes
    }

    /// Remove the step at `idx` when it holds nothing. Only called on a step
    /// that is already closed, so no index in `RunState` points at it.
    fn drop_step_if_empty(&mut self, idx: usize) -> Vec<Change> {
        match self.entries.get(idx) {
            Some(TranscriptEntry::Step(s)) if s.is_empty() => {
                let id = s.id.clone();
                self.entries.remove(idx);
                vec![Change::Removed(id)]
            }
            _ => Vec::new(),
        }
    }

    /// A `Reasoning` / `ReasoningBlock` delta.
    fn append_thinking(&mut self, s: &str, now_ms: Option<u64>) -> Vec<Change> {
        if s.is_empty() {
            return Vec::new();
        }
        let prefix = self.run.as_ref().map_or("", |r| r.thinking.delta_prefix());
        let (changes, applied) = self.edit_open_step(now_ms, |step| {
            let block = step.thinking.get_or_insert_with(|| ThinkingBlock {
                text: String::new(),
                streaming: true,
            });
            block.text.push_str(prefix);
            block.text.push_str(s);
            block.streaming = true;
            true
        });
        if applied {
            if let Some(r) = self.run.as_mut() {
                r.thinking.streamed = true;
            }
        }
        changes
    }

    /// A `ReasoningEmitted` record (`RecordCursor::fold`). It never assumes
    /// it precedes the iteration's text (production emits
    /// `TextEmitted{Final}` first). A record for an iteration other than a
    /// numbered open step's cannot be placed: `NeedsResync`, nothing moves.
    fn record_thinking(
        &mut self,
        iteration: usize,
        text: &str,
        now_ms: Option<u64>,
    ) -> Vec<Change> {
        if text.is_empty() {
            return Vec::new();
        }
        let wanted = u32::try_from(iteration).ok();
        if let Some(step) = self.open_step_ref() {
            if step.iteration.is_some() && step.iteration != wanted {
                return vec![Change::NeedsResync];
            }
        }
        let cursor = self.run.as_ref().map(|r| r.thinking).unwrap_or_default();
        let mut kept_len = 0;
        let (changes, applied) = self.edit_open_step(now_ms, |step| {
            let current = step.thinking.as_ref().map_or("", |b| b.text.as_str());
            let Some(joined) = cursor.fold(current, text) else {
                return false;
            };
            kept_len = joined.len();
            step.thinking = Some(ThinkingBlock {
                text: joined,
                streaming: false,
            });
            true
        });
        if applied {
            if let Some(r) = self.run.as_mut() {
                r.thinking = RecordCursor {
                    record_len: kept_len,
                    streamed: false,
                };
            }
        }
        changes
    }

    /// A `ResponseChunk` delta, or text the record supplies when the stream
    /// rendered none (`SessionCompleted.final_text`, `final_response`).
    fn append_text(&mut self, s: &str, now_ms: Option<u64>) -> Vec<Change> {
        if s.is_empty() {
            return Vec::new();
        }
        let prefix = self.run.as_ref().map_or("", |r| r.text.delta_prefix());
        let (changes, applied) = self.edit_open_step(now_ms, |step| {
            let text = step.text.get_or_insert_with(String::new);
            text.push_str(prefix);
            text.push_str(s);
            true
        });
        if applied {
            if let Some(r) = self.run.as_mut() {
                r.text_rendered = true;
                r.text.streamed = true;
            }
        }
        changes
    }

    /// A `TextEmitted` record: authoritative for the text exactly as
    /// `ReasoningEmitted` is for thinking (spec §5.2) — it replaces whatever
    /// was streamed since the previous record, so a grace turn's record
    /// after a halted Think's partial text, or a rescued call re-streamed
    /// from the start, both converge to what replay shows.
    fn record_text(&mut self, text: &str, now_ms: Option<u64>) -> Vec<Change> {
        if text.is_empty() {
            return Vec::new();
        }
        let cursor = self.run.as_ref().map(|r| r.text).unwrap_or_default();
        let mut kept_len = 0;
        let (changes, applied) = self.edit_open_step(now_ms, |step| {
            let Some(joined) = cursor.fold(step.text.as_deref().unwrap_or(""), text) else {
                return false;
            };
            kept_len = joined.len();
            step.text = Some(joined);
            true
        });
        if applied {
            if let Some(r) = self.run.as_mut() {
                r.text_rendered = true;
                r.text = RecordCursor {
                    record_len: kept_len,
                    streamed: false,
                };
            }
        }
        changes
    }

    fn note(&mut self, ev: &AgentTraceEvent, kind: NoteKind, now_ms: Option<u64>) -> Vec<Change> {
        // The presenter's `None` means "no visual representation" (its own
        // contract), not "unknown": no note, nothing to resync.
        let Some(text) = presentation_text(ev) else {
            return Vec::new();
        };
        self.edit_open_step(now_ms, |step| {
            step.notes.push(Note { kind, text });
            true
        })
        .0
    }

    fn push_notice(&mut self, text: String) -> Change {
        let id = self.next_id("notice");
        self.entries.push(TranscriptEntry::SystemNotice {
            id: id.clone(),
            text,
        });
        Change::Inserted(id)
    }

    // ---- tool rows (scan over THIS run's steps: a provider may repeat an
    // ---- id across runs, never within one) --------------------------------

    fn run_first_entry(&self) -> usize {
        self.run.as_ref().map_or(0, |r| r.first_entry)
    }

    fn find_tool(&mut self, tool_id: &str) -> Option<(String, &mut ToolRow)> {
        let first = self.run_first_entry();
        self.entries
            .get_mut(first..)?
            .iter_mut()
            .rev()
            .find_map(|e| match e {
                TranscriptEntry::Step(s) => {
                    let id = s.id.clone();
                    s.find_tool_mut(tool_id).map(|r| (id, r))
                }
                _ => None,
            })
    }

    fn start_tool(
        &mut self,
        tool_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
        now_ms: Option<u64>,
    ) -> Vec<Change> {
        if let Some((step_id, row)) = self.find_tool(tool_id) {
            // The two faces (`tool_start` and `agent_trace.tool_call_started`)
            // both announce the same call; the second sighting must not reset
            // a running or finished row.
            if row.status == RowStatus::Pending {
                start_row(row, now_ms);
                return vec![Change::Updated(step_id)];
            }
            return Vec::new();
        }
        let mut row = ToolRow::new(tool_id, tool_name, args);
        start_row(&mut row, now_ms);
        self.edit_open_step(now_ms, |step| {
            step.tools.push(row);
            true
        })
        .0
    }

    fn update_tool(&mut self, tool_id: &str, progress: &str) -> Vec<Change> {
        match self.find_tool(tool_id) {
            Some((step_id, row)) if matches!(row.status, RowStatus::Running { .. }) => {
                row.body = RowBody::Text(progress.to_string());
                vec![Change::Updated(step_id)]
            }
            _ => Vec::new(),
        }
    }

    fn finish_tool(
        &mut self,
        tool_id: &str,
        tool_name: Option<&str>,
        result: &ToolResult,
        duration_ms: u64,
        now_ms: Option<u64>,
    ) -> Vec<Change> {
        if let Some((step_id, row)) = self.find_tool(tool_id) {
            finish_row(row, result, duration_ms, now_ms);
            return vec![Change::Updated(step_id)];
        }
        // A result whose start was dropped: the trace face names the tool
        // and can reconstruct the row; the `tool_end` face cannot and leaves
        // it to `RunComplete`'s authoritative list.
        let Some(name) = tool_name else {
            return Vec::new();
        };
        let mut row = ToolRow::new(tool_id, name, &serde_json::Value::Null);
        finish_row(&mut row, result, duration_ms, now_ms);
        self.edit_open_step(now_ms, |step| {
            step.tools.push(row);
            true
        })
        .0
    }

    // ---- run end ---------------------------------------------------------

    fn complete_run(
        &mut self,
        summary: &RunSummary,
        total_ms: u64,
        now_ms: Option<u64>,
    ) -> Vec<Change> {
        let mut changes = Vec::new();
        // 1. The authoritative terminal record fills what the stream dropped.
        for item in &summary.tool_summaries {
            let error = summary
                .errors
                .iter()
                .find(|e| e.tool_id == item.tool_id)
                .map(|e| e.error.clone());
            let wire = ToolResult {
                success: item.success,
                output: None,
                error,
                presentation: None,
            };
            // `map` ends the mutable borrow before the `None` arm needs
            // `self` again (an `if let … else` here is E0499).
            let existing = self.find_tool(&item.tool_id).map(|(step_id, row)| {
                // Keep whatever body the live stream delivered; the record
                // carries none.
                let body = row.body.clone();
                finish_row(row, &wire, item.duration_ms, now_ms);
                if matches!(row.body, RowBody::None) {
                    row.body = body;
                }
                step_id
            });
            match existing {
                Some(step_id) => changes.push(Change::Updated(step_id)),
                None => changes.extend(self.finish_tool(
                    &item.tool_id,
                    Some(&item.tool_name),
                    &wire,
                    item.duration_ms,
                    now_ms,
                )),
            }
        }
        // 2. Nothing spins after the run ended.
        changes.extend(self.settle_orphans());
        // 3. A run that streamed no text still has an answer in the record.
        let rendered = self.run.as_ref().is_some_and(|r| r.text_rendered);
        if !rendered {
            if let Some(t) = summary.final_response.as_deref() {
                changes.extend(self.append_text(t.trim_end(), now_ms));
            }
        }
        // 4. The last step's text IS the answer: hoist it out (ruling R5).
        changes.extend(self.hoist_open_step_text(now_ms));
        // 5. The trailers, over every row of this run — the same input the
        //    replay leg uses (ruling T910-SUM); `tool_summaries` only
        //    reconciled the rows above.
        let rows = self.run_rows();
        changes.extend(self.push_trailers(&rows, Some(total_ms)));
        // 6. Effect reached? Fewer turn boundaries than the loop counted
        //    means frames were lost — say so, patch nothing. `<`, not `≠`:
        //    the loops/TurnStarted relation on the grace turn is unmeasured
        //    (PF-D8), and a guard that misreports costs more than one that
        //    under-reports.
        let steps_seen = self.run.as_ref().map_or(0, |r| r.steps_seen);
        if summary.loops > 0 && steps_seen < summary.loops {
            changes.push(Change::NeedsResync);
        }
        self.run = None;
        changes
    }

    fn fail_run(&mut self, error: &str, now_ms: Option<u64>) -> Vec<Change> {
        let mut changes = self.settle_orphans();
        changes.extend(self.hoist_open_step_text(now_ms));
        changes.push(self.push_notice(format!("Error: {error}")));
        self.run = None;
        changes
    }

    /// Every row of THIS run's steps, in entry order: the trailer's input on
    /// both legs.
    fn run_rows(&self) -> Vec<ToolRow> {
        self.entries
            .get(self.run_first_entry()..)
            .unwrap_or_default()
            .iter()
            .filter_map(|e| match e {
                TranscriptEntry::Step(s) => Some(s.tools.iter()),
                _ => None,
            })
            .flatten()
            .cloned()
            .collect()
    }

    /// The run's trailers, one derivation for both legs: the turn summary
    /// over `rows`, and the worked-for notice with the iterations this
    /// transcript saw when the run's wall clock is known.
    fn push_trailers(&mut self, rows: &[ToolRow], total_ms: Option<u64>) -> Vec<Change> {
        let mut changes = Vec::new();
        if let Some(entry) = summarize_turn(rows) {
            // `TurnSummaryEntry` carries no id; the change names a minted one
            // (PF-C9) so it never collides with another entry's.
            let id = self.next_id("summary");
            self.entries.push(TranscriptEntry::TurnSummary(entry));
            changes.push(Change::Inserted(id));
        }
        if let Some(ms) = total_ms {
            let steps_seen = self.run.as_ref().map_or(0, |r| r.steps_seen);
            changes.push(self.push_notice(format!("{} · {} steps", worked_for(ms), steps_seen)));
        }
        changes
    }

    /// Nothing spins after its run ended: THIS run's `Running` rows settle
    /// to `Pending`.
    fn settle_orphans(&mut self) -> Vec<Change> {
        let first = self.run_first_entry();
        let mut changes = Vec::new();
        for e in self.entries.get_mut(first..).unwrap_or_default() {
            if let TranscriptEntry::Step(s) = e {
                let mut touched = false;
                for r in &mut s.tools {
                    if matches!(r.status, RowStatus::Running { .. }) {
                        r.settle_resumed();
                        touched = true;
                    }
                }
                if touched {
                    changes.push(Change::Updated(s.id.clone()));
                }
            }
        }
        changes
    }

    /// Close the open step, move its text out as the run's `AssistantText`,
    /// and remove the step if that was all it held.
    fn hoist_open_step_text(&mut self, now_ms: Option<u64>) -> Vec<Change> {
        // Captured BEFORE `close_open_step` clears it.
        let Some(idx) = self.run.as_ref().and_then(|r| r.open_step) else {
            return Vec::new();
        };
        let mut changes = self.close_open_step(StepStatus::Settled, now_ms);
        let (text, step_id, empty) = match self.entries.get_mut(idx) {
            Some(TranscriptEntry::Step(s)) => {
                let text = s.text.take().filter(|t| !t.trim().is_empty());
                (text, s.id.clone(), s.is_empty())
            }
            _ => {
                if !changes.contains(&Change::NeedsResync) {
                    changes.push(Change::NeedsResync);
                }
                return changes;
            }
        };
        if empty {
            self.entries.remove(idx);
            changes.push(Change::Removed(step_id));
        }
        if let Some(markdown) = text {
            let id = self.next_id("assistant");
            self.entries.push(TranscriptEntry::AssistantText {
                id: id.clone(),
                markdown,
                streaming: false,
            });
            changes.push(Change::Inserted(id));
        }
        changes
    }
}

/// Start a row. Without a clock the row carries none (`since_ms: 0` is the
/// placeholder `Running` needs; it never survives the run's end).
fn start_row(row: &mut ToolRow, now_ms: Option<u64>) {
    match now_ms {
        Some(now) => row.start(now),
        None => row.status = RowStatus::Running { since_ms: 0 },
    }
}

/// Settle a row from a wire result. Without a clock no clock is written:
/// `RowStatus` still carries the recorded duration.
fn finish_row(row: &mut ToolRow, result: &ToolResult, duration_ms: u64, now_ms: Option<u64>) {
    let ended = row.ended_ms;
    row.finish(result, duration_ms, now_ms.unwrap_or(0));
    if now_ms.is_none() {
        row.ended_ms = ended;
    }
}

/// One trace tool-end, as the wire result shape a row settles from.
///
/// The trace event splits what the wire keeps together: the outcome is in
/// `AgentTraceToolResult`, the `presentation` side-channel is on
/// `AgentTraceToolCallEnd` beside it. Joining them here — rather than at each
/// call site — is what stops the diff from being dropped on this path while
/// the live `tool_end` path carries it (判据 §9: one verb, two faces, one
/// derivation). The TUI's `app/trace.rs` imports this one; there is no
/// second copy.
#[must_use]
pub fn trace_result_to_wire(
    result: &AgentTraceToolResult,
    presentation: Option<&aleph_protocol::file_change::Presentation>,
) -> ToolResult {
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
    ToolResult {
        success,
        output,
        error,
        presentation: presentation.cloned(),
    }
}

/// One derivation of the loop's narration text, shared with the TUI's
/// debug entries: the protocol's own presenter.
fn presentation_text(ev: &AgentTraceEvent) -> Option<String> {
    present_agent_trace_event_with_preset(ev, AgentTracePresentationPreset::TuiDebug)
        .map(|p| p.content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{step_headline, step_tally};
    use aleph_protocol::events::ToolSummaryItem;
    use aleph_protocol::{
        AgentTraceEvent, AgentTraceTextKind, AgentTraceToolCallEnd, AgentTraceToolCallStart,
        AgentTraceToolResult, RunSummary, StreamEvent, ToolResult,
    };
    use serde_json::json;

    const RUN: &str = "run-1";

    fn accepted() -> StreamEvent {
        StreamEvent::RunAccepted {
            run_id: RUN.into(),
            session_key: "s".into(),
            accepted_at: "t".into(),
        }
    }
    fn reasoning(s: &str) -> StreamEvent {
        StreamEvent::Reasoning {
            run_id: RUN.into(),
            seq: 0,
            content: s.into(),
            is_complete: false,
        }
    }
    fn chunk(s: &str) -> StreamEvent {
        StreamEvent::ResponseChunk {
            run_id: RUN.into(),
            seq: 0,
            content: s.into(),
            chunk_index: 0,
            is_final: false,
            is_intermediate: false,
        }
    }
    fn trace(ev: AgentTraceEvent) -> StreamEvent {
        StreamEvent::AgentTrace {
            run_id: RUN.into(),
            seq: 0,
            event: ev,
        }
    }
    fn turn(i: usize) -> StreamEvent {
        trace(AgentTraceEvent::TurnStarted { iteration: i })
    }
    fn thought(i: usize, text: &str) -> StreamEvent {
        trace(AgentTraceEvent::ReasoningEmitted {
            iteration: i,
            text: text.into(),
        })
    }
    fn tool_start(id: &str, name: &str) -> StreamEvent {
        StreamEvent::ToolStart {
            run_id: RUN.into(),
            seq: 0,
            tool_name: name.into(),
            tool_id: id.into(),
            params: json!({"command": "ls"}),
        }
    }
    fn tool_end(id: &str, ms: u64) -> StreamEvent {
        StreamEvent::ToolEnd {
            run_id: RUN.into(),
            seq: 0,
            tool_id: id.into(),
            result: ToolResult::success("ok"),
            duration_ms: ms,
        }
    }
    fn complete(loops: u32, summaries: Vec<ToolSummaryItem>) -> StreamEvent {
        StreamEvent::RunComplete {
            run_id: RUN.into(),
            seq: 0,
            summary: RunSummary {
                loops,
                tool_summaries: summaries,
                ..Default::default()
            },
            total_duration_ms: 32_000,
        }
    }
    fn item(id: &str, name: &str, ms: u64, ok: bool) -> ToolSummaryItem {
        ToolSummaryItem {
            tool_id: id.into(),
            tool_name: name.into(),
            emoji: String::new(),
            duration_ms: ms,
            success: ok,
        }
    }
    fn steps(t: &Transcript) -> Vec<&StepEntry> {
        t.entries()
            .iter()
            .filter_map(|e| match e {
                TranscriptEntry::Step(s) => Some(s),
                _ => None,
            })
            .collect()
    }
    fn finals(t: &Transcript) -> Vec<String> {
        t.entries()
            .iter()
            .filter_map(|e| match e {
                TranscriptEntry::AssistantText { markdown, .. } => Some(markdown.clone()),
                _ => None,
            })
            .collect()
    }
    fn drive(t: &mut Transcript, frames: &[StreamEvent]) -> Vec<Change> {
        let mut out = Vec::new();
        for (i, f) in frames.iter().enumerate() {
            out.extend(t.apply_live(f, 1_000 + i as u64));
        }
        out
    }

    #[test]
    fn a_frame_before_any_turn_started_opens_an_unnumbered_step_that_is_never_renumbered() {
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[accepted(), reasoning("early"), turn(1), chunk("hi")],
        );
        let s = steps(&t);
        assert_eq!(s.len(), 2, "{:?}", t.entries());
        assert_eq!(s[0].iteration, None);
        assert_eq!(
            s[0].thinking.as_ref().map(|b| b.text.as_str()),
            Some("early")
        );
        assert_eq!(s[1].iteration, Some(1));
        assert_eq!(s[1].text.as_deref(), Some("hi"));
    }

    #[test]
    fn turn_started_closes_the_previous_step_and_opens_the_next() {
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[accepted(), turn(1), chunk("a"), turn(2), chunk("b")],
        );
        let s = steps(&t);
        assert_eq!(s[0].status, StepStatus::Settled);
        assert!(s[0].ended_ms.is_some());
        assert_eq!(s[1].status, StepStatus::Live);
        assert_eq!(
            (s[0].text.as_deref(), s[1].text.as_deref()),
            (Some("a"), Some("b"))
        );
    }

    #[test]
    fn streamed_thinking_is_replaced_by_the_authoritative_record() {
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                reasoning("Weigh"),
                reasoning("ing…"),
                trace(AgentTraceEvent::ReasoningEmitted {
                    iteration: 1,
                    text: "Weighing the two readings.".into(),
                }),
            ],
        );
        let b = steps(&t)[0].thinking.clone().unwrap();
        assert_eq!(b.text, "Weighing the two readings.");
        assert!(!b.streaming);
    }

    /// The verifier-halt salvage path re-runs Think on the SAME iteration and
    /// emits a second record. Records accumulate in arrival order; only the
    /// streamed deltas each record covers are replaced.
    #[test]
    fn two_reasoning_records_on_one_iteration_are_kept_in_arrival_order() {
        let mut t = Transcript::new();
        let changes = drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                reasoning("Fir"),
                thought(1, "First pass."),
                reasoning("Sec"),
                thought(1, "Second pass."),
            ],
        );
        assert!(!changes.contains(&Change::NeedsResync), "{changes:?}");
        let b = steps(&t)[0].thinking.clone().unwrap();
        assert_eq!(b.text, "First pass.\n\nSecond pass.");
        assert!(!b.streaming);
    }

    /// Thinking + tool calls and no text: the record still lands on the step,
    /// and the step survives the run's end (it holds thinking and a tool).
    #[test]
    fn a_tool_only_step_gets_its_thinking() {
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                tool_start("a", "bash"),
                tool_end("a", 4),
                thought(1, "List the directory first."),
                complete(1, vec![item("a", "bash", 4, true)]),
            ],
        );
        let s = steps(&t);
        assert_eq!(s.len(), 1, "{:?}", t.entries());
        assert_eq!(
            s[0].thinking.as_ref().map(|b| b.text.as_str()),
            Some("List the directory first.")
        );
        assert_eq!(s[0].text, None);
        assert_eq!(s[0].tools.len(), 1);
        assert!(finals(&t).is_empty(), "{:?}", t.entries());
    }

    /// A record naming an iteration other than the open step's cannot be
    /// placed: "I don't know", never a guess (判据 §8).
    #[test]
    fn a_reasoning_record_for_another_iteration_asks_for_a_resync_and_changes_nothing() {
        let mut t = Transcript::new();
        drive(&mut t, &[accepted(), turn(2)]);
        let before = t.entries().to_vec();
        let changes = t.apply_live(&thought(1, "stray"), 2_000);
        assert_eq!(changes, vec![Change::NeedsResync]);
        assert_eq!(t.entries(), before.as_slice());
    }

    #[test]
    fn final_text_from_the_trace_is_deduplicated_against_the_streamed_chunks() {
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                chunk("Hel"),
                chunk("lo"),
                trace(AgentTraceEvent::TextEmitted {
                    iteration: 1,
                    stream: AgentTraceTextKind::Final,
                    text: "Hello world".into(),
                }),
            ],
        );
        assert_eq!(steps(&t)[0].text.as_deref(), Some("Hello world"));
    }

    #[test]
    fn parallel_tool_calls_in_one_iteration_share_the_step_and_settle_independently() {
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                tool_start("a", "bash"),
                tool_start("b", "bash"),
                // the trace face repeats the start: must not reset the row
                trace(AgentTraceEvent::ToolCallStarted {
                    iteration: 1,
                    call: AgentTraceToolCallStart {
                        tool_id: "a".into(),
                        tool_name: "bash".into(),
                        input: json!({"command": "ls"}),
                    },
                }),
                tool_end("b", 7),
            ],
        );
        let s = steps(&t);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].tools.len(), 2);
        assert!(matches!(s[0].tools[0].status, RowStatus::Running { .. }));
        assert_eq!(s[0].tools[1].status, RowStatus::Ok { duration_ms: 7 });
    }

    #[test]
    fn a_tool_end_for_an_unknown_id_is_a_no_op_until_the_summary_names_it() {
        let mut t = Transcript::new();
        drive(&mut t, &[accepted(), turn(1), tool_end("ghost", 3)]);
        assert!(steps(&t)[0].tools.is_empty());
        drive(&mut t, &[complete(1, vec![item("ghost", "bash", 3, true)])]);
        let s = steps(&t);
        assert_eq!(
            s[0].tools.len(),
            1,
            "the authoritative record reconstructs it"
        );
        assert_eq!(s[0].tools[0].status, RowStatus::Ok { duration_ms: 3 });
        assert_eq!(
            s[0].tools[0].body,
            RowBody::None,
            "header-only: it knows the call, not its output"
        );
    }

    #[test]
    fn run_complete_hoists_the_final_text_and_keeps_a_thinking_only_step() {
        // Step 1: thinking only (a veto forced a continue). Step 2: the answer.
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                reasoning("first try"),
                trace(AgentTraceEvent::VerifierVeto {
                    iteration: 1,
                    reason: "- [ ] tests".into(),
                }),
                turn(2),
                reasoning("second"),
                chunk("Done."),
                complete(2, vec![]),
            ],
        );
        let s = steps(&t);
        assert_eq!(s.len(), 2, "{:?}", t.entries());
        assert_eq!(s[0].notes.len(), 1);
        assert_eq!(s[0].notes[0].kind, NoteKind::VerifierVeto);
        assert_eq!(s[1].text, None, "hoisted out of the step");
        assert_eq!(
            s[1].thinking.as_ref().map(|b| b.text.as_str()),
            Some("second")
        );
        assert_eq!(finals(&t), vec!["Done.".to_string()]);
        let hoisted_after_step = t
            .entries()
            .iter()
            .position(|e| matches!(e, TranscriptEntry::AssistantText { .. }));
        let last_step = t
            .entries()
            .iter()
            .rposition(|e| matches!(e, TranscriptEntry::Step(_)));
        assert!(
            hoisted_after_step > last_step,
            "the answer follows the steps"
        );
    }

    #[test]
    fn run_complete_removes_a_step_that_held_only_the_hoisted_text() {
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                chunk("Just an answer."),
                complete(1, vec![]),
            ],
        );
        assert!(steps(&t).is_empty(), "{:?}", t.entries());
        assert_eq!(finals(&t), vec!["Just an answer.".to_string()]);
    }

    #[test]
    fn run_complete_drops_a_step_that_held_nothing() {
        // The fourth hoist cell: an iteration that produced no thinking, no
        // text, no tool and no note (e.g. an empty retried response). It is
        // not rendered as "Step N: (nothing)".
        // Step 1 is closed empty by `TurnStarted 2` and dropped there; step 2
        // holds only the answer, which the hoist takes, so it is dropped too.
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                turn(2),
                chunk("Answer."),
                complete(2, vec![]),
            ],
        );
        assert!(steps(&t).is_empty(), "{:?}", t.entries());
        assert_eq!(finals(&t), vec!["Answer.".to_string()]);
        assert!(
            t.entries().iter().any(
                |e| matches!(e, TranscriptEntry::SystemNotice { text, .. } if text.ends_with("· 2 steps"))
            ),
            "the trailer still counts the iterations the loop ran: {:?}",
            t.entries()
        );
    }

    /// PF-D9 hoist cell: the OPEN step at `RunComplete` (not one a
    /// `TurnStarted` closed) holds only thinking and the record carries an
    /// empty answer. The step stays; no empty answer is invented.
    #[test]
    fn run_complete_keeps_a_thinking_only_open_step_when_final_response_is_empty() {
        let mut t = Transcript::new();
        drive(&mut t, &[accepted(), turn(1), reasoning("only a thought")]);
        let ev = StreamEvent::RunComplete {
            run_id: RUN.into(),
            seq: 0,
            summary: RunSummary {
                loops: 1,
                final_response: Some(String::new()),
                ..Default::default()
            },
            total_duration_ms: 10,
        };
        t.apply_live(&ev, 5_000);
        let s = steps(&t);
        assert_eq!(s.len(), 1, "{:?}", t.entries());
        assert_eq!(s[0].status, StepStatus::Settled);
        assert_eq!(
            s[0].thinking.as_ref().map(|b| b.text.as_str()),
            Some("only a thought")
        );
        assert_eq!(s[0].text, None);
        assert!(
            finals(&t).is_empty(),
            "no answer was recorded, none is invented"
        );
    }

    /// PF-D9 hoist cell: the OPEN step at `RunComplete` holds nothing at
    /// all. It is removed by the hoist, not left as "Step 1: (nothing)".
    #[test]
    fn run_complete_drops_an_open_step_that_held_nothing() {
        let mut t = Transcript::new();
        drive(&mut t, &[accepted(), turn(1)]);
        let changes = t.apply_live(&complete(1, vec![]), 5_000);
        assert!(steps(&t).is_empty(), "{:?}", t.entries());
        assert!(finals(&t).is_empty());
        assert!(
            changes.iter().any(|c| matches!(c, Change::Removed(_))),
            "the removal is reported so a keyed surface drops the row: {changes:?}"
        );
    }

    #[test]
    fn run_complete_with_thinking_and_text_keeps_the_thinking_row_beside_the_answer() {
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                reasoning("why"),
                chunk("Answer."),
                complete(1, vec![]),
            ],
        );
        let s = steps(&t);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].thinking.as_ref().map(|b| b.text.as_str()), Some("why"));
        assert_eq!(s[0].text, None);
        assert_eq!(finals(&t), vec!["Answer.".to_string()]);
    }

    #[test]
    fn run_complete_with_nothing_rendered_falls_back_to_final_response() {
        let mut t = Transcript::new();
        drive(&mut t, &[accepted(), turn(1)]);
        let ev = StreamEvent::RunComplete {
            run_id: RUN.into(),
            seq: 0,
            summary: RunSummary {
                loops: 1,
                final_response: Some("From the summary.  ".into()),
                ..Default::default()
            },
            total_duration_ms: 10,
        };
        t.apply_live(&ev, 5_000);
        assert_eq!(finals(&t), vec!["From the summary.".to_string()]);
    }

    #[test]
    fn run_complete_produces_the_turn_summary_and_the_worked_for_notice() {
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                tool_start("a", "bash"),
                tool_end("a", 100),
                turn(2),
                tool_start("b", "file_read"),
                tool_end("b", 50),
                chunk("Done."),
                complete(
                    2,
                    vec![
                        item("a", "bash", 100, true),
                        item("b", "file_read", 50, true),
                    ],
                ),
            ],
        );
        let summary = t.entries().iter().find_map(|e| match e {
            TranscriptEntry::TurnSummary(s) => Some(s.clone()),
            _ => None,
        });
        let summary = summary.expect("a turn summary");
        assert_eq!(
            (summary.commands, summary.reads, summary.duration_ms),
            (1, 1, 150)
        );
        let notice = t.entries().iter().rev().find_map(|e| match e {
            TranscriptEntry::SystemNotice { text, .. } => Some(text.clone()),
            _ => None,
        });
        assert_eq!(notice.as_deref(), Some("✻ Worked for 32s · 2 steps"));
    }

    type Skeleton = (String, Option<u32>, Option<String>, Vec<ToolRow>, Vec<Note>);

    /// What a step IS, minus what the run's end legitimately does to it:
    /// its status/clock (the run still completes) and its text (hoisted —
    /// asserted separately through `finals`).
    fn step_skeleton(t: &Transcript) -> Vec<Skeleton> {
        steps(t)
            .into_iter()
            .map(|s| {
                (
                    s.id.clone(),
                    s.iteration,
                    s.thinking.as_ref().map(|b| b.text.clone()),
                    s.tools.clone(),
                    s.notes.clone(),
                )
            })
            .collect()
    }

    #[test]
    fn fewer_turn_starts_than_summary_loops_reports_needs_resync_without_touching_entries() {
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[accepted(), turn(1), reasoning("weigh"), chunk("a")],
        );
        let before = step_skeleton(&t);
        assert_eq!(before.len(), 1, "fixture: one step that survives the hoist");
        let changes = t.apply_live(&complete(3, vec![]), 9_000);
        assert!(changes.contains(&Change::NeedsResync), "{changes:?}");
        // "Never patches" = no step is fabricated for the missing loops and
        // none is renumbered: same ids, iterations and contents.
        assert_eq!(step_skeleton(&t), before);
        // The run still completes normally (hoist etc.) — resync is the
        // client's next move, not a reason to leave the run half-open.
        assert_eq!(finals(&t), vec!["a".to_string()]);
    }

    #[test]
    fn a_run_with_zero_loops_never_asks_for_a_resync() {
        // simple.rs / slash paths: no trace, no iterations counted.
        let mut t = Transcript::new();
        drive(&mut t, &[accepted(), reasoning("x"), chunk("y")]);
        let changes = t.apply_live(&complete(0, vec![]), 9_000);
        assert!(!changes.contains(&Change::NeedsResync));
    }

    #[test]
    fn run_error_settles_rows_hoists_the_partial_answer_and_leaves_a_notice() {
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                tool_start("a", "bash"),
                chunk("half an ans"),
            ],
        );
        let ev = StreamEvent::RunError {
            run_id: RUN.into(),
            seq: 0,
            error: "provider down".into(),
            error_code: None,
        };
        t.apply_live(&ev, 5_000);
        assert_eq!(
            steps(&t)[0].tools[0].status,
            RowStatus::Pending,
            "never a spinner after the run ended"
        );
        assert_eq!(finals(&t), vec!["half an ans".to_string()]);
        let notice = t.entries().iter().rev().find_map(|e| match e {
            TranscriptEntry::SystemNotice { text, .. } => Some(text.clone()),
            _ => None,
        });
        assert_eq!(notice.as_deref(), Some("Error: provider down"));
    }

    #[test]
    fn a_tool_summary_becomes_a_note_on_the_open_step() {
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                trace(AgentTraceEvent::ToolSummary {
                    iteration: 1,
                    summary: "Read the failing test.".into(),
                }),
            ],
        );
        let n = &steps(&t)[0].notes[0];
        assert_eq!(n.kind, NoteKind::ToolSummary);
        assert!(n.text.contains("Read the failing test."), "{}", n.text);
    }

    #[test]
    fn cache_health_is_a_run_level_notice_not_a_step_note() {
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                trace(AgentTraceEvent::CacheHealthDegraded {
                    scope: "main".into(),
                    streak: 3,
                    reads: 0,
                    writes: 900,
                    prefix_changed: Some(true),
                }),
            ],
        );
        assert!(steps(&t)[0].notes.is_empty());
        assert!(t
            .entries()
            .iter()
            .any(|e| matches!(e, TranscriptEntry::SystemNotice { .. })));
    }

    #[test]
    fn a_frame_for_another_run_is_ignored() {
        let mut t = Transcript::new();
        drive(&mut t, &[accepted(), turn(1)]);
        let foreign = StreamEvent::ResponseChunk {
            run_id: "run-other".into(),
            seq: 0,
            content: "nope".into(),
            chunk_index: 0,
            is_final: false,
            is_intermediate: false,
        };
        assert!(t.apply_live(&foreign, 1).is_empty());
        assert_eq!(steps(&t)[0].text, None);
    }

    #[test]
    fn push_user_appends_a_user_row_with_its_time() {
        let mut t = Transcript::new();
        let c = t.push_user("hi", Some(42), vec![]);
        assert!(matches!(c, Change::Inserted(_)));
        assert!(matches!(
            &t.entries()[0],
            TranscriptEntry::UserText { text, at_ms: Some(42), .. } if text == "hi"
        ));
    }

    #[test]
    fn a_tool_call_completed_with_a_presentation_lands_it_on_the_row() {
        use aleph_protocol::file_change::{FileChange, FileChangeKind, Presentation, Unavailable};
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                trace(AgentTraceEvent::ToolCallStarted {
                    iteration: 1,
                    call: AgentTraceToolCallStart {
                        tool_id: "e".into(),
                        tool_name: "file_edit".into(),
                        input: json!({"file_path": "a.rs"}),
                    },
                }),
                trace(AgentTraceEvent::ToolCallCompleted {
                    iteration: 1,
                    call: AgentTraceToolCallEnd {
                        tool_id: "e".into(),
                        tool_name: "file_edit".into(),
                        input: json!({"file_path": "a.rs"}),
                        duration_ms: 9,
                        presentation: Some(Presentation::FileChanges {
                            changes: vec![FileChange::unavailable(
                                "a.rs",
                                FileChangeKind::Modified,
                                Unavailable::TooLarge,
                            )],
                        }),
                    },
                    result: AgentTraceToolResult::Success {
                        output: json!("ok"),
                    },
                }),
            ],
        );
        let row = &steps(&t)[0].tools[0];
        assert!(matches!(row.body, RowBody::FileChanges(ref c) if c.len() == 1));
        assert_eq!(row.status, RowStatus::Ok { duration_ms: 9 });
    }

    // ---- fix round 1 ----------------------------------------------------

    fn text_record(i: usize, text: &str) -> StreamEvent {
        trace(AgentTraceEvent::TextEmitted {
            iteration: i,
            stream: AgentTraceTextKind::Final,
            text: text.into(),
        })
    }

    /// The same frame, addressed to another run.
    fn in_run(run: &str, ev: StreamEvent) -> StreamEvent {
        let run_id = run.to_string();
        match ev {
            StreamEvent::RunAccepted {
                session_key,
                accepted_at,
                ..
            } => StreamEvent::RunAccepted {
                run_id,
                session_key,
                accepted_at,
            },
            StreamEvent::AgentTrace { seq, event, .. } => {
                StreamEvent::AgentTrace { run_id, seq, event }
            }
            StreamEvent::ToolStart {
                seq,
                tool_name,
                tool_id,
                params,
                ..
            } => StreamEvent::ToolStart {
                run_id,
                seq,
                tool_name,
                tool_id,
                params,
            },
            StreamEvent::ToolEnd {
                seq,
                tool_id,
                result,
                duration_ms,
                ..
            } => StreamEvent::ToolEnd {
                run_id,
                seq,
                tool_id,
                result,
                duration_ms,
            },
            StreamEvent::RunComplete {
                seq,
                summary,
                total_duration_ms,
                ..
            } => StreamEvent::RunComplete {
                run_id,
                seq,
                summary,
                total_duration_ms,
            },
            other => panic!("in_run: no arm for {other:?}"),
        }
    }

    /// F1(a): the boundary grace turn records a different text on the halted
    /// iteration after that Think streamed a partial one. The record is the
    /// text — it replaces the streamed segment instead of being dropped.
    #[test]
    fn a_text_record_replaces_a_halted_turns_partial_text() {
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                chunk("I think the bug is in the par"),
                chunk("The timezone handling was wrong."),
                text_record(1, "The timezone handling was wrong."),
            ],
        );
        assert_eq!(
            steps(&t)[0].text.as_deref(),
            Some("The timezone handling was wrong.")
        );
        t.apply_live(&complete(1, vec![]), 9_000);
        assert_eq!(
            finals(&t),
            vec!["The timezone handling was wrong.".to_string()]
        );
    }

    /// F1(b): deltas that ARE the record converge to it with no duplication.
    #[test]
    fn streamed_text_that_spells_the_record_converges_without_duplication() {
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                chunk("Hello "),
                chunk("world"),
                text_record(1, "Hello world"),
            ],
        );
        assert_eq!(steps(&t)[0].text.as_deref(), Some("Hello world"));
    }

    /// F1(c): a streaming call failed mid-stream and was rescued without
    /// streaming; the rescue's full text is delta'd again from the start.
    #[test]
    fn a_rescued_call_restreamed_from_the_start_converges_to_the_record() {
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                chunk("Fixed the ti"),
                chunk("Fixed the timezone."),
                text_record(1, "Fixed the timezone."),
            ],
        );
        assert_eq!(steps(&t)[0].text.as_deref(), Some("Fixed the timezone."));
    }

    /// Ruling T910-SUM: the trailer counts every row of the run on the live
    /// leg too — the record's `tool_summaries` only reconciles rows the
    /// client missed.
    #[test]
    fn the_turn_summary_counts_every_row_of_the_run_not_only_the_records_list() {
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                tool_start("a", "bash"),
                tool_end("a", 10),
                tool_start("b", "bash"),
                tool_end("b", 20),
                chunk("Done."),
                complete(1, vec![item("a", "bash", 10, true)]),
            ],
        );
        let summary = t.entries().iter().find_map(|e| match e {
            TranscriptEntry::TurnSummary(s) => Some(s.clone()),
            _ => None,
        });
        let summary = summary.expect("two rows cross the gate");
        assert_eq!((summary.commands, summary.duration_ms), (2, 30));
    }

    /// F2: thinking that was only ever streamed stops streaming when its step
    /// closes — at the next `TurnStarted` and at the run's end.
    #[test]
    fn a_delta_only_thinking_step_stops_streaming_when_it_closes() {
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                reasoning("first"),
                turn(2),
                reasoning("second"),
                complete(2, vec![]),
            ],
        );
        let s = steps(&t);
        assert_eq!(s.len(), 2, "{:?}", t.entries());
        assert!(s
            .iter()
            .all(|s| s.thinking.as_ref().is_some_and(|b| !b.streaming)));
    }

    /// F11: mid-stream, a delta segment after a record starts a new
    /// paragraph — the join the next record will use.
    #[test]
    fn a_delta_after_a_record_starts_a_new_paragraph_mid_stream() {
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[accepted(), turn(1), thought(1, "First."), reasoning("Sec")],
        );
        let b = steps(&t)[0].thinking.clone().unwrap();
        assert_eq!(b.text, "First.\n\nSec");
        assert!(b.streaming);
    }

    /// F4: a provider may repeat a tool id across runs. The next run gets its
    /// own row; the earlier run's finished row is not touched.
    #[test]
    fn a_tool_id_repeated_by_the_next_run_gets_its_own_row() {
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                tool_start("t1", "bash"),
                tool_end("t1", 5),
                complete(1, vec![item("t1", "bash", 5, true)]),
            ],
        );
        let second: Vec<StreamEvent> = [
            accepted(),
            turn(1),
            tool_start("t1", "bash"),
            tool_end("t1", 9),
            complete(1, vec![item("t1", "bash", 9, true)]),
        ]
        .into_iter()
        .map(|f| in_run("run-2", f))
        .collect();
        drive(&mut t, &second);
        let s = steps(&t);
        assert_eq!(s.len(), 2, "{:?}", t.entries());
        assert_eq!(s[0].tools.len(), 1);
        assert_eq!(s[0].tools[0].status, RowStatus::Ok { duration_ms: 5 });
        assert_eq!(s[1].tools.len(), 1);
        assert_eq!(s[1].tools[0].status, RowStatus::Ok { duration_ms: 9 });
    }

    /// F5: a run accepted while the previous one never reported its end.
    /// How that run ended is unknown: its step closes `Pending` and its rows
    /// stop spinning; the new run folds into a step of its own.
    #[test]
    fn a_run_accepted_over_an_open_run_closes_it_pending() {
        let mut t = Transcript::new();
        drive(
            &mut t,
            &[
                accepted(),
                turn(1),
                reasoning("thinking"),
                tool_start("a", "bash"),
            ],
        );
        let changes = t.apply_live(&in_run("run-2", accepted()), 5_000);
        assert!(!changes.is_empty(), "the old step's change is reported");
        t.apply_live(&in_run("run-2", turn(1)), 5_001);
        let s = steps(&t);
        assert_eq!(s.len(), 2, "{:?}", t.entries());
        assert_eq!(s[0].status, StepStatus::Pending);
        assert_eq!(s[0].tools[0].status, RowStatus::Pending);
        assert!(s[0].thinking.as_ref().is_some_and(|b| !b.streaming));
        assert_eq!(s[1].status, StepStatus::Live);
    }

    // ---- replay leg + G2 -------------------------------------------------

    /// A replay row is the same `AgentTraceEvent` a live `agent_trace`
    /// frame carries. The two legs share `apply_trace`; this fixture checks
    /// the parts that DIFFER around it: no `RunAccepted`, no deltas, no
    /// `RunComplete`, and `finish_replay` doing what `complete_run` does.
    ///
    /// `text_first` puts iteration 2's `TextEmitted{Final}` before its
    /// `ReasoningEmitted` — the order production emits them in; the fold
    /// must not depend on either. `salvage` adds the verifier-halt salvage
    /// path's second `ReasoningEmitted` on iteration 1. Iteration 1 reads
    /// two files, so the run crosses the turn-summary gate and both legs'
    /// trailer inputs are compared.
    fn full_run_trace_with(text_first: bool, salvage: bool) -> Vec<AgentTraceEvent> {
        let read = |id: &str, path: &str, ms: u64| {
            [
                AgentTraceEvent::ToolCallStarted {
                    iteration: 1,
                    call: AgentTraceToolCallStart {
                        tool_id: id.into(),
                        tool_name: "file_read".into(),
                        input: json!({ "path": path }),
                    },
                },
                AgentTraceEvent::ToolCallCompleted {
                    iteration: 1,
                    call: AgentTraceToolCallEnd {
                        tool_id: id.into(),
                        tool_name: "file_read".into(),
                        input: json!({ "path": path }),
                        duration_ms: ms,
                        presentation: None,
                    },
                    result: AgentTraceToolResult::Success {
                        output: json!("fn a() {}"),
                    },
                },
            ]
        };
        let mut rows = vec![
            AgentTraceEvent::TurnStarted { iteration: 1 },
            AgentTraceEvent::ReasoningEmitted {
                iteration: 1,
                text: "Look at the failing test first.".into(),
            },
        ];
        if salvage {
            rows.push(AgentTraceEvent::ReasoningEmitted {
                iteration: 1,
                text: "The veto says a box is unchecked.".into(),
            });
        }
        rows.extend(read("r1", "tests/a.rs", 12));
        rows.extend(read("r2", "src/b.rs", 8));
        rows.push(AgentTraceEvent::TurnStarted { iteration: 2 });
        let thinking = AgentTraceEvent::ReasoningEmitted {
            iteration: 2,
            text: "The timezone is the bug.".into(),
        };
        let text = AgentTraceEvent::TextEmitted {
            iteration: 2,
            stream: AgentTraceTextKind::Final,
            text: "Fixed the timezone handling.".into(),
        };
        if text_first {
            rows.extend([text, thinking]);
        } else {
            rows.extend([thinking, text]);
        }
        rows.push(AgentTraceEvent::SessionCompleted {
            outcome: aleph_protocol::AgentTraceSessionOutcome::Completed,
            iterations: 2,
            tool_calls_made: 1,
            total_tokens: 100,
            hit_limit: false,
            final_text: Some("Fixed the timezone handling.".into()),
            terminate_reason: None,
            duration_ms: Some(32_000),
            token_breakdown: None,
            tool_timeline: Vec::new(),
        });
        rows
    }

    fn full_run_trace() -> Vec<AgentTraceEvent> {
        full_run_trace_with(true, false)
    }

    /// How the last iteration's records reached the live leg.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Grace {
        /// A normal Think: each record's deltas, split in two, then it.
        No,
        /// The boundary grace turn (`fire_boundary_grace_turn`): no
        /// `TurnStarted` of its own, its text and thinking streamed as ONE
        /// delta each, then `TextEmitted{Final}` + `ReasoningEmitted` on the
        /// halted iteration. The halted Think streamed nothing.
        Clean,
        /// The same, after the halted Think had streamed partial thinking and
        /// partial text that no record covers and that are NOT prefixes of
        /// the grace turn's records.
        AfterPartial,
    }

    /// The live leg for the same run: the same trace frames interleaved
    /// with the deltas and the lifecycle frames a client actually receives.
    fn full_run_live(rows: Vec<AgentTraceEvent>, grace: Grace) -> Vec<StreamEvent> {
        let deltas = |text: &str| -> Vec<String> {
            if grace == Grace::No {
                let (a, b) = text.split_at(text.len() / 2);
                vec![a.to_string(), b.to_string()]
            } else {
                vec![text.to_string()]
            }
        };
        let mut out = vec![accepted()];
        for ev in rows {
            match &ev {
                AgentTraceEvent::TurnStarted { iteration: 2 } if grace == Grace::AfterPartial => {
                    out.push(trace(ev));
                    out.push(reasoning("Maybe the parser drops"));
                    out.push(chunk("I think the bug is in the par"));
                }
                AgentTraceEvent::ReasoningEmitted { text, .. } => {
                    // deltas first, then the authoritative record
                    out.extend(deltas(text).iter().map(|d| reasoning(d)));
                    out.push(trace(ev));
                }
                AgentTraceEvent::TextEmitted { text, .. } => {
                    out.extend(deltas(text).iter().map(|d| chunk(d)));
                    out.push(trace(ev));
                }
                AgentTraceEvent::ToolCallStarted { call, .. } => {
                    out.push(StreamEvent::ToolStart {
                        run_id: RUN.into(),
                        seq: 0,
                        tool_name: call.tool_name.clone(),
                        tool_id: call.tool_id.clone(),
                        params: call.input.clone(),
                    });
                    out.push(trace(ev));
                }
                AgentTraceEvent::ToolCallCompleted { call, result, .. } => {
                    out.push(StreamEvent::ToolEnd {
                        run_id: RUN.into(),
                        seq: 0,
                        tool_id: call.tool_id.clone(),
                        result: trace_result_to_wire(result, call.presentation.as_ref()),
                        duration_ms: call.duration_ms,
                    });
                    out.push(trace(ev));
                }
                _ => out.push(trace(ev)),
            }
        }
        out.push(StreamEvent::RunComplete {
            run_id: RUN.into(),
            seq: 0,
            summary: RunSummary {
                loops: 2,
                tool_summaries: vec![
                    item("r1", "file_read", 12, true),
                    item("r2", "file_read", 8, true),
                ],
                final_response: Some("Fixed the timezone handling.".into()),
                ..Default::default()
            },
            total_duration_ms: 32_000,
        });
        out
    }

    /// Clocks are the one thing the replay leg cannot know.
    fn strip_clocks(entries: &[TranscriptEntry]) -> Vec<TranscriptEntry> {
        entries
            .iter()
            .cloned()
            .map(|e| match e {
                TranscriptEntry::Step(mut s) => {
                    s.started_ms = None;
                    s.ended_ms = None;
                    for r in &mut s.tools {
                        r.started_ms = None;
                        r.ended_ms = None;
                        if let RowStatus::Running { since_ms } = &mut r.status {
                            *since_ms = 0;
                        }
                    }
                    TranscriptEntry::Step(s)
                }
                other => other,
            })
            .collect()
    }

    fn replay(rows: &[AgentTraceEvent]) -> Transcript {
        let mut t = Transcript::new();
        for ev in rows {
            t.apply_replay(ev);
        }
        t.finish_replay();
        t
    }

    /// G2. Two legs, one fold — in either within-turn order, across the
    /// salvage path's two records on one iteration, and across the boundary
    /// grace turn with and without a halted Think's partial stream.
    #[test]
    fn the_live_leg_and_the_replay_leg_fold_to_the_same_entries() {
        let shapes = [
            (true, false, Grace::No),
            (false, false, Grace::No),
            (true, true, Grace::No),
            (true, false, Grace::Clean),
            (true, false, Grace::AfterPartial),
        ];
        for (text_first, salvage, grace) in shapes {
            let rows = full_run_trace_with(text_first, salvage);
            let mut live = Transcript::new();
            let live_changes = drive(&mut live, &full_run_live(rows.clone(), grace));
            assert!(
                !live_changes.contains(&Change::NeedsResync),
                "{live_changes:?}"
            );
            let replay = replay(&rows);

            let (l, r) = (strip_clocks(live.entries()), strip_clocks(replay.entries()));
            assert_eq!(
                l, r,
                "text_first={text_first} salvage={salvage} grace={grace:?}\nLIVE:   {l:#?}\nREPLAY: {r:#?}"
            );
            assert!(
                l.iter()
                    .any(|e| matches!(e, TranscriptEntry::TurnSummary(_))),
                "the fixture must cross the turn-summary gate: {l:#?}"
            );
        }

        // And the shape is the one the surfaces will paint:
        let replay = replay(&full_run_trace());
        let s = steps(&replay);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].iteration, Some(1));
        assert_eq!(s[0].tools[0].status, RowStatus::Ok { duration_ms: 12 });
        assert_eq!(s[0].tools.len(), 2);
        assert_eq!(
            s[1].thinking.as_ref().map(|b| b.text.as_str()),
            Some("The timezone is the bug.")
        );
        assert_eq!(
            finals(&replay),
            vec!["Fixed the timezone handling.".to_string()]
        );
        assert_eq!(
            step_headline(s[0], 80).map(|h| h.text),
            Some("Look at the failing test first.".to_string())
        );
    }

    #[test]
    fn a_replayed_run_without_a_session_completed_row_is_pending_not_settled() {
        let mut t = Transcript::new();
        let rows = full_run_trace();
        // The first four rows: TurnStarted 1, ReasoningEmitted 1,
        // ToolCallStarted r1, ToolCallCompleted r1 — the log stops there.
        for ev in &rows[..4] {
            t.apply_replay(ev);
        }
        t.apply_replay(&AgentTraceEvent::TurnStarted { iteration: 2 });
        t.apply_replay(&AgentTraceEvent::ToolCallStarted {
            iteration: 2,
            call: AgentTraceToolCallStart {
                tool_id: "b".into(),
                tool_name: "bash".into(),
                input: json!({"command": "cargo test"}),
            },
        });
        t.finish_replay();
        let s = steps(&t);
        assert_eq!(
            s[0].status,
            StepStatus::Settled,
            "a step the next turn closed is settled"
        );
        assert_eq!(
            s[1].status,
            StepStatus::Pending,
            "the last step never ended: unknown"
        );
        assert_eq!(
            s[1].tools[0].status,
            RowStatus::Pending,
            "never a spinner from a log"
        );
        assert!(
            finals(&t).is_empty(),
            "no answer was recorded, none is invented"
        );
    }

    #[test]
    fn replay_rows_carry_no_clocks_but_the_recorded_durations() {
        let t = replay(&full_run_trace());
        let row = &steps(&t)[0].tools[0];
        assert_eq!((row.started_ms, row.ended_ms), (None, None));
        let summary = t.entries().iter().find_map(|e| match e {
            TranscriptEntry::TurnSummary(s) => Some(s.duration_ms),
            _ => None,
        });
        assert_eq!(summary, Some(20), "the recorded durations, 12 + 8");
        assert_eq!(step_tally(steps(&t)[0]).map(|s| s.duration_ms), Some(20));
    }

    /// One transcript holds a session's runs. The replay leg has no
    /// `tool_summaries` list to filter by, so "all rows" must mean THIS
    /// run's rows: a one-tool run after a two-tool run is still below the
    /// turn-summary gate.
    #[test]
    fn a_replayed_run_counts_only_its_own_rows_in_the_turn_summary() {
        let tool = |id: &str| {
            [
                AgentTraceEvent::ToolCallStarted {
                    iteration: 1,
                    call: AgentTraceToolCallStart {
                        tool_id: id.into(),
                        tool_name: "bash".into(),
                        input: json!({"command": "ls"}),
                    },
                },
                AgentTraceEvent::ToolCallCompleted {
                    iteration: 1,
                    call: AgentTraceToolCallEnd {
                        tool_id: id.into(),
                        tool_name: "bash".into(),
                        input: json!({"command": "ls"}),
                        duration_ms: 5,
                        presentation: None,
                    },
                    result: AgentTraceToolResult::Success {
                        output: json!("ok"),
                    },
                },
            ]
        };
        let done = AgentTraceEvent::SessionCompleted {
            outcome: aleph_protocol::AgentTraceSessionOutcome::Completed,
            iterations: 1,
            tool_calls_made: 1,
            total_tokens: 1,
            hit_limit: false,
            final_text: None,
            terminate_reason: None,
            duration_ms: Some(1_000),
            token_breakdown: None,
            tool_timeline: Vec::new(),
        };
        let mut t = Transcript::new();
        t.apply_replay(&AgentTraceEvent::TurnStarted { iteration: 1 });
        for ev in tool("a1").iter().chain(tool("a2").iter()) {
            t.apply_replay(ev);
        }
        t.apply_replay(&done);
        t.finish_replay();
        t.apply_replay(&AgentTraceEvent::TurnStarted { iteration: 1 });
        for ev in &tool("b1") {
            t.apply_replay(ev);
        }
        t.apply_replay(&done);
        t.finish_replay();
        let summaries = t
            .entries()
            .iter()
            .filter(|e| matches!(e, TranscriptEntry::TurnSummary(_)))
            .count();
        assert_eq!(
            summaries,
            1,
            "only the two-tool run earns one: {:?}",
            t.entries()
        );
    }
}
