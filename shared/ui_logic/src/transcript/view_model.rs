//! The transcript as data. Entries are chronological; a tool row is its own
//! entry (Claude Code interleaves tools with text), so a surface that used
//! to draw "all tools above the turn's text" now just paints the list.

use aleph_protocol::file_change::{FileChange, Presentation};
use aleph_protocol::ToolResult;

use super::summarize::{summarize, CallSummary};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowStatus {
    /// Requested, not yet started (or restored from a log with no start
    /// event and no result — NOT spinning, see `settle_resumed`).
    Pending,
    Running { since_ms: u64 },
    Ok { duration_ms: u64 },
    Err { duration_ms: u64, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowBody {
    None,
    Text(String),
    FileChanges(Vec<FileChange>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRow {
    pub id: String,
    pub tool: String,
    pub summary: CallSummary,
    pub status: RowStatus,
    pub body: RowBody,
    pub expanded: bool,
    pub started_ms: Option<u64>,
    pub ended_ms: Option<u64>,
}

/// Display names whose calls are read-only (they group; Edit/Write/Bash never do).
///
/// `"Files"` (`file_ops`) is deliberately absent: `file_ops` multiplexes
/// read-only actions (list/search) with destructive ones (delete/move) —
/// see `src/security/dangerous_tools.rs`'s registration of `file_ops` as
/// dangerous, precisely because the danger lives in an argument, not the
/// tool name. `ToolRow` carries only the rendered `CallSummary`, not the
/// structured args, so `is_read_only` cannot tell a `list` call from a
/// `delete` call here — grouping a delete as "explored" would lie, while
/// failing to group a genuinely read-only call merely under-groups. Fail
/// closed: leave it out.
pub const READ_ONLY_DISPLAY_NAMES: &[&str] = &["Read", "Grep", "Find", "Fetch", "Search", "Memory", "Context", "Tools"];

impl ToolRow {
    #[must_use]
    pub fn new(id: impl Into<String>, tool: impl Into<String>, args: &serde_json::Value) -> Self {
        let tool = tool.into();
        Self {
            summary: summarize(&tool, args),
            id: id.into(),
            tool,
            status: RowStatus::Pending,
            body: RowBody::None,
            expanded: false,
            started_ms: None,
            ended_ms: None,
        }
    }

    pub fn start(&mut self, now_ms: u64) {
        self.started_ms = Some(now_ms);
        self.status = RowStatus::Running { since_ms: now_ms };
    }

    /// Apply a wire `ToolResult`. A presentation wins over text for the body.
    pub fn finish(&mut self, result: &ToolResult, duration_ms: u64, now_ms: u64) {
        self.ended_ms = Some(now_ms);
        self.body = match &result.presentation {
            Some(Presentation::FileChanges { changes }) => RowBody::FileChanges(changes.clone()),
            None => match result.output.as_deref().filter(|s| !s.is_empty()) {
                Some(text) => RowBody::Text(text.to_string()),
                None => RowBody::None,
            },
        };
        self.status = if result.success {
            RowStatus::Ok { duration_ms }
        } else {
            RowStatus::Err {
                duration_ms,
                message: result.error.clone().unwrap_or_else(|| "failed".into()),
            }
        };
    }

    /// A row restored from a log arrives without a start event. `Running`
    /// never survives resume: it always settles to `Pending` — "unknown",
    /// never a fabricated success — even when `ended_ms` happens to be set
    /// (a resume/replay caller can set that field directly without calling
    /// `finish`, and this method must not read that as a completed call it
    /// never actually observed). Never a spinner that turns forever (pi
    /// `resolveToolVisualState`).
    pub fn settle_resumed(&mut self) {
        if matches!(self.status, RowStatus::Running { .. }) {
            self.status = RowStatus::Pending;
        }
    }

    #[must_use]
    pub fn is_read_only(&self) -> bool {
        READ_ONLY_DISPLAY_NAMES.contains(&self.summary.display_name.as_str())
    }

    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self.status, RowStatus::Ok { .. } | RowStatus::Err { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolGroup {
    pub rows: Vec<ToolRow>,
    pub expanded: bool,
}

impl ToolGroup {
    /// `Explored 4 calls · 0.8s` — `n` is the row count (not distinct file
    /// paths) and the duration is the sum of each row's `Ok`/`Err`
    /// duration; the ` · duration` suffix is omitted while that sum is
    /// still zero (no row has finished yet).
    #[must_use]
    pub fn headline(&self) -> String {
        let n = self.rows.len();
        let total_ms: u64 = self.rows.iter().map(|r| match &r.status {
            RowStatus::Ok { duration_ms } | RowStatus::Err { duration_ms, .. } => *duration_ms,
            _ => 0,
        }).sum();
        let noun = if n == 1 { "call" } else { "calls" };
        let dur = if total_ms > 0 { format!(" · {}", super::affordance::fmt_duration_ms(total_ms)) } else { String::new() };
        format!("Explored {n} {noun}{dur}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnSummaryEntry {
    pub commands: u32,
    pub reads: u32,
    pub edits: u32,
    pub writes: u32,
    pub others: u32,
    pub failed: u32,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptEntry {
    UserText { id: String, text: String },
    AssistantText { id: String, markdown: String, streaming: bool },
    Reasoning { id: String, text: String, collapsed: bool },
    Tool(ToolRow),
    ToolGroup(ToolGroup),
    TurnSummary(TurnSummaryEntry),
    SystemNotice { id: String, text: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finish_prefers_a_presentation_body_over_text() {
        let mut r = ToolRow::new("c1", "file_edit", &json!({"file_path": "a.rs"}));
        r.start(1000);
        let res = ToolResult::success("Replaced 1 occurrence").with_presentation(Some(
            Presentation::FileChanges { changes: vec![FileChange::unavailable("a.rs", aleph_protocol::file_change::FileChangeKind::Modified, aleph_protocol::file_change::Unavailable::TooLarge)] },
        ));
        r.finish(&res, 42, 1042);
        assert!(matches!(r.body, RowBody::FileChanges(ref c) if c.len() == 1));
        assert_eq!(r.status, RowStatus::Ok { duration_ms: 42 });
    }

    #[test]
    fn a_resumed_row_without_a_result_is_pending_not_spinning() {
        let mut r = ToolRow::new("c1", "grep", &json!({"pattern": "x"}));
        r.status = RowStatus::Running { since_ms: 5 }; // what a naive replay would set
        r.settle_resumed();
        assert_eq!(r.status, RowStatus::Pending);
    }

    #[test]
    fn a_running_row_with_ended_ms_set_still_settles_to_pending_not_a_fabricated_ok() {
        // A resume/replay caller can set `ended_ms` directly (the field is
        // `pub`) without ever calling `finish` with a real `ToolResult`.
        // `settle_resumed` must not read that as "it must have succeeded".
        let mut r = ToolRow::new("c1", "grep", &json!({"pattern": "x"}));
        r.status = RowStatus::Running { since_ms: 5 };
        r.ended_ms = Some(9);
        r.settle_resumed();
        assert_eq!(r.status, RowStatus::Pending);
    }

    #[test]
    fn a_file_ops_row_is_not_read_only() {
        // file_ops multiplexes read-only (list/search) and destructive
        // (delete/move) actions behind one tool name; ToolRow only carries
        // the rendered display name, not the args, so it cannot tell them
        // apart here. Grouping a delete as "explored" would lie — fail
        // closed by never treating file_ops as read-only.
        let r = ToolRow::new("c1", "file_ops", &json!({"action": "delete", "path": "a.rs"}));
        assert_eq!(r.summary.display_name, "Files");
        assert!(!r.is_read_only());
    }

    #[test]
    fn an_error_result_carries_its_message() {
        let mut r = ToolRow::new("c1", "bash", &json!({"command": "false"}));
        r.finish(&ToolResult::error("exit 1"), 7, 7);
        assert!(matches!(r.status, RowStatus::Err { ref message, .. } if message == "exit 1"));
        assert!(r.is_terminal());
    }
}
