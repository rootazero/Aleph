//! A run's end, on either leg: reconciling from the record, settling
//! orphans, hoisting the answer out of the last step, and the trailers —
//! one derivation for both.

use aleph_protocol::{RunSummary, ToolResult};

use super::tool_rows::finish_row;
use super::{Change, Transcript};
use crate::transcript::{
    summarize_turn, worked_for, RowBody, RowStatus, StepStatus, ToolRow, TranscriptEntry,
};

impl Transcript {
    /// A new run was accepted while the previous one never reported its end
    /// (a dropped terminal frame, a reconnect). How it ended is unknown: its
    /// rows stop spinning and its open step closes `Pending`, never `Live`
    /// forever and never a fabricated `Settled`.
    pub(super) fn abandon_open_run(&mut self, now_ms: u64) -> Vec<Change> {
        if self.run.is_none() {
            return Vec::new();
        }
        let mut changes = self.settle_orphans();
        changes.extend(self.close_open_step(StepStatus::Pending, Some(now_ms)));
        self.run = None;
        changes
    }

    pub(super) fn complete_run(
        &mut self,
        summary: &RunSummary,
        total_ms: u64,
        now_ms: Option<u64>,
    ) -> Vec<Change> {
        // 1. The authoritative terminal record fills what the stream dropped.
        let mut changes = self.reconcile_from_record(summary, now_ms);
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

    /// Settle every row `RunSummary.tool_summaries` names from that record,
    /// reconstructing (header-only) the rows whose frames never arrived.
    fn reconcile_from_record(&mut self, summary: &RunSummary, now_ms: Option<u64>) -> Vec<Change> {
        let mut changes = Vec::new();
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
        changes
    }

    pub(super) fn fail_run(&mut self, error: &str, now_ms: Option<u64>) -> Vec<Change> {
        let mut changes = self.settle_orphans();
        changes.extend(self.hoist_open_step_text(now_ms));
        changes.push(self.push_notice(format!("Error: {error}")));
        self.run = None;
        changes
    }

    /// Every row of THIS run's steps, in entry order: the trailer's input on
    /// both legs.
    pub(super) fn run_rows(&self) -> Vec<ToolRow> {
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
    pub(super) fn push_trailers(&mut self, rows: &[ToolRow], total_ms: Option<u64>) -> Vec<Change> {
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
    pub(super) fn settle_orphans(&mut self) -> Vec<Change> {
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
    pub(super) fn hoist_open_step_text(&mut self, now_ms: Option<u64>) -> Vec<Change> {
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
