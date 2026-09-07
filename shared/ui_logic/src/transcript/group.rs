//! Merge consecutive read-only tool rows into one `Explored N calls` group.
//!
//! Rules (pi-cc-extensions `grouping.ts`, adjusted for Aleph names): only
//! read-only rows group; Edit/Write/Bash never do and always break a run;
//! up to `MAX_GAP_TEXT` empty/whitespace assistant texts between two
//! read-only rows are tolerated; a group of one dissolves back to a row.

use super::view_model::{ToolGroup, TranscriptEntry};

/// Empty assistant texts tolerated inside a run before it breaks.
pub const MAX_GAP_TEXT: usize = 3;
/// Rows needed for a group to exist.
pub const MIN_GROUP: usize = 2;

fn is_blank_text(e: &TranscriptEntry) -> bool {
    matches!(e, TranscriptEntry::AssistantText { markdown, .. } if markdown.trim().is_empty())
}

#[must_use]
pub fn group_entries(entries: Vec<TranscriptEntry>) -> Vec<TranscriptEntry> {
    let mut out: Vec<TranscriptEntry> = Vec::with_capacity(entries.len());
    let mut run: Vec<super::view_model::ToolRow> = Vec::new();
    let mut gap: Vec<TranscriptEntry> = Vec::new();

    let flush = |run: &mut Vec<super::view_model::ToolRow>, gap: &mut Vec<TranscriptEntry>, out: &mut Vec<TranscriptEntry>| {
        if run.len() >= MIN_GROUP {
            out.push(TranscriptEntry::ToolGroup(ToolGroup { rows: std::mem::take(run), expanded: false }));
        } else {
            for r in run.drain(..) {
                out.push(TranscriptEntry::Tool(r));
            }
        }
        out.append(gap);
    };

    for e in entries {
        match e {
            TranscriptEntry::Tool(row) if row.is_read_only() => {
                // Gap texts between two read-only rows are swallowed into the
                // group's position (they were blank anyway).
                gap.clear();
                run.push(row);
            }
            other if !run.is_empty() && is_blank_text(&other) && gap.len() < MAX_GAP_TEXT => {
                gap.push(other);
            }
            other => {
                flush(&mut run, &mut gap, &mut out);
                out.push(other);
            }
        }
    }
    flush(&mut run, &mut gap, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::super::view_model::{RowStatus, ToolRow};
    use super::*;
    use serde_json::json;

    fn read(id: &str) -> TranscriptEntry {
        let mut r = ToolRow::new(id, "file_read", &json!({"path": format!("{id}.rs")}));
        r.status = RowStatus::Ok { duration_ms: 100 };
        TranscriptEntry::Tool(r)
    }
    fn edit(id: &str) -> TranscriptEntry {
        TranscriptEntry::Tool(ToolRow::new(id, "file_edit", &json!({"file_path": "x.rs"})))
    }
    fn text(s: &str) -> TranscriptEntry {
        TranscriptEntry::AssistantText { id: "t".into(), markdown: s.into(), streaming: false }
    }

    #[test]
    fn consecutive_reads_group_and_edits_break_the_run() {
        let out = group_entries(vec![read("a"), read("b"), edit("c"), read("d")]);
        assert!(matches!(&out[0], TranscriptEntry::ToolGroup(g) if g.rows.len() == 2));
        assert!(matches!(&out[1], TranscriptEntry::Tool(r) if r.tool == "file_edit"));
        // A lone read after the break dissolves back to a plain row.
        assert!(matches!(&out[2], TranscriptEntry::Tool(r) if r.id == "d"));
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn blank_texts_inside_a_run_are_tolerated_but_real_text_breaks_it() {
        let out = group_entries(vec![read("a"), text("  "), read("b"), text("Found it."), read("c")]);
        assert!(matches!(&out[0], TranscriptEntry::ToolGroup(g) if g.rows.len() == 2));
        assert!(matches!(&out[1], TranscriptEntry::AssistantText { markdown, .. } if markdown == "Found it."));
        assert!(matches!(&out[2], TranscriptEntry::Tool(_)));
    }

    #[test]
    fn a_single_read_never_becomes_a_group() {
        let out = group_entries(vec![read("a"), text("x")]);
        assert!(matches!(&out[0], TranscriptEntry::Tool(_)));
    }

    #[test]
    fn group_headline_counts_calls_and_sums_duration() {
        let out = group_entries(vec![read("a"), read("b")]);
        let TranscriptEntry::ToolGroup(g) = &out[0] else { panic!("group") };
        assert_eq!(g.headline(), "Explored 2 calls · 0.2s");
    }
}
