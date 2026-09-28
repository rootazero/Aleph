//! Tool rows: both faces of a call (`tool_start` / `tool_end` and the
//! trace's `tool_call_started` / `tool_call_completed`) settle one row by id,
//! within the current run.

use aleph_protocol::{AgentTraceToolResult, ToolResult};

use super::{Change, Transcript};
use crate::transcript::{RowBody, RowStatus, ToolRow, TranscriptEntry};

impl Transcript {
    // ---- tool rows (scan over THIS run's steps: a provider may repeat an
    // ---- id across runs, never within one) --------------------------------

    pub(super) fn run_first_entry(&self) -> usize {
        self.run.as_ref().map_or(0, |r| r.first_entry)
    }

    pub(super) fn find_tool(&mut self, tool_id: &str) -> Option<(String, &mut ToolRow)> {
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

    pub(super) fn start_tool(
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

    pub(super) fn update_tool(&mut self, tool_id: &str, progress: &str) -> Vec<Change> {
        match self.find_tool(tool_id) {
            Some((step_id, row)) if matches!(row.status, RowStatus::Running { .. }) => {
                row.body = RowBody::Text(progress.to_string());
                vec![Change::Updated(step_id)]
            }
            _ => Vec::new(),
        }
    }

    pub(super) fn finish_tool(
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
pub(super) fn finish_row(
    row: &mut ToolRow,
    result: &ToolResult,
    duration_ms: u64,
    now_ms: Option<u64>,
) {
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
