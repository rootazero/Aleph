//! The open step: opening and closing it, and folding thinking, text and
//! notes into it — streamed deltas against the authoritative records.

use aleph_protocol::AgentTraceEvent;

use super::{presentation_text, Change, Transcript};
use crate::transcript::{Note, NoteKind, StepEntry, StepStatus, ThinkingBlock, TranscriptEntry};

/// Between two records of one field on one iteration (the verifier-halt
/// salvage path re-runs Think on the same iteration and records again), and
/// before a delta segment that follows a record.
const RECORD_JOIN: &str = "\n\n";

/// One field of the open step (its thinking, or its text) fed by two
/// sources: streamed deltas, and the authoritative records the trace later
/// writes for them (`ReasoningEmitted`, `TextEmitted`). A record replaces
/// every delta streamed since the previous record and is appended after any
/// earlier record of the same step — so the live leg converges to exactly
/// what the replay leg (records only) shows, whatever the deltas were.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct RecordCursor {
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

impl Transcript {
    pub(super) fn open_step_ref(&self) -> Option<&StepEntry> {
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
    pub(super) fn edit_open_step(
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

    pub(super) fn start_turn(
        &mut self,
        iteration: Option<u32>,
        now_ms: Option<u64>,
    ) -> Vec<Change> {
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

    pub(super) fn close_open_step(
        &mut self,
        status: StepStatus,
        now_ms: Option<u64>,
    ) -> Vec<Change> {
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
    pub(super) fn append_thinking(&mut self, s: &str, now_ms: Option<u64>) -> Vec<Change> {
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
    pub(super) fn record_thinking(
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
    pub(super) fn append_text(&mut self, s: &str, now_ms: Option<u64>) -> Vec<Change> {
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
    pub(super) fn record_text(&mut self, text: &str, now_ms: Option<u64>) -> Vec<Change> {
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

    pub(super) fn note(
        &mut self,
        ev: &AgentTraceEvent,
        kind: NoteKind,
        now_ms: Option<u64>,
    ) -> Vec<Change> {
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

    pub(super) fn push_notice(&mut self, text: String) -> Change {
        let id = self.next_id("notice");
        self.entries.push(TranscriptEntry::SystemNotice {
            id: id.clone(),
            text,
        });
        Change::Inserted(id)
    }
}
