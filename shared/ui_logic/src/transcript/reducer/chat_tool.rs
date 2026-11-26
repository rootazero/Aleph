//! V1 uses authorized display data, not the legacy tool input or run summary.

use aleph_protocol::{
    chat_transcript::{ChatToolChange, ChatToolDelta},
    ToolResult,
};

use super::{Change, StepStatus, Transcript, TranscriptEntry};
use crate::transcript::{CallSummary, RowBody, RowStatus, ToolRow};

impl Transcript {
    /// Fold a display-only tool event into the currently attached run. Only
    /// the first sighting is an ordering boundary; later events patch that row.
    pub fn apply_chat_tool(
        &mut self,
        run_id: &str,
        delta: &ChatToolDelta,
        now_ms: u64,
    ) -> Vec<Change> {
        let Some(run) = self.run.as_ref().filter(|run| run.run_id == run_id) else {
            return Vec::new();
        };
        let id = format!("tool/{run_id}/{}", delta.tool_call_id);
        let existing = self.entries[run.first_entry..]
            .iter_mut()
            .find_map(|entry| match entry {
                TranscriptEntry::Tool(row) if row.id == id => Some(row),
                _ => None,
            });
        if let Some(row) = existing {
            return if finish(row, &delta.change, now_ms) {
                vec![Change::Updated(id)]
            } else {
                Vec::new()
            };
        }
        let summary = match &delta.change {
            ChatToolChange::Started {
                display_name,
                args_text,
            } => CallSummary {
                display_name: display_name.clone(),
                args_text: args_text.clone(),
            },
            ChatToolChange::Working => return Vec::new(),
            ChatToolChange::Finished { .. } => CallSummary {
                display_name: "Tool".into(),
                args_text: String::new(),
            },
        };
        let mut row = ToolRow {
            id,
            tool: String::new(),
            summary,
            status: RowStatus::Pending,
            body: RowBody::None,
            expanded: false,
            started_ms: None,
            ended_ms: None,
        };
        if !finish(&mut row, &delta.change, now_ms) {
            row.start(now_ms);
        }
        let mut changes = self.close_open_step(StepStatus::Settled, Some(now_ms));
        changes.push(Change::Inserted(row.id.clone()));
        self.entries.push(TranscriptEntry::Tool(row));
        changes
    }

    /// Stop provisional rows without ingesting undisclosed terminal-summary
    /// fields. The Panel then reconciles from authorized committed history.
    pub fn seal_chat_run(&mut self, run_id: &str, now_ms: u64) -> Vec<Change> {
        if self.run.as_ref().is_some_and(|run| run.run_id == run_id) {
            self.abandon_open_run(now_ms)
        } else {
            Vec::new()
        }
    }
}

fn finish(row: &mut ToolRow, change: &ChatToolChange, now_ms: u64) -> bool {
    let ChatToolChange::Finished {
        success,
        duration_ms,
        presentation,
        ..
    } = change
    else {
        return false;
    };
    if row.is_terminal() {
        return false;
    }
    row.finish(
        &ToolResult {
            success: *success,
            presentation: presentation.clone(),
            error: None,
            output: None,
        },
        *duration_ms,
        now_ms,
    );
    true
}
