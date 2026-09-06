//! `Ran 3 commands, read 2 files, edited 1 file · 42s` — stored as DATA on
//! the transcript, formatted at paint time (pi-cc-extensions agent-summary).

use super::affordance::fmt_duration_ms;
use super::view_model::{RowStatus, ToolRow, TurnSummaryEntry};

pub const MIN_TOOLS_FOR_SUMMARY: usize = 2;

#[must_use]
pub fn summarize_turn(rows: &[ToolRow]) -> Option<TurnSummaryEntry> {
    if rows.len() < MIN_TOOLS_FOR_SUMMARY {
        return None;
    }
    let mut e = TurnSummaryEntry { commands: 0, reads: 0, edits: 0, writes: 0, others: 0, failed: 0, duration_ms: 0 };
    let mut read_paths = std::collections::HashSet::new();
    let mut edit_paths = std::collections::HashSet::new();
    let mut write_paths = std::collections::HashSet::new();
    for r in rows {
        match r.summary.display_name.as_str() {
            "Bash" => e.commands += 1,
            "Read" => {
                read_paths.insert(r.summary.args_text.clone());
            }
            "Edit" | "Patch" => {
                edit_paths.insert(r.summary.args_text.clone());
            }
            "Write" => {
                write_paths.insert(r.summary.args_text.clone());
            }
            _ => e.others += 1,
        }
        if let RowStatus::Err { .. } = r.status {
            e.failed += 1;
        }
        // Only a terminal (Ok/Err) row's elapsed time is billed — matching
        // `ToolGroup::headline`'s existing rule. A `Pending` row is an
        // outcome we explicitly refuse to vouch for (see
        // `ToolRow::settle_resumed`), even when a resume/replay caller left
        // a stale `ended_ms` on it; counting that time would report
        // duration for something we just said we cannot claim.
        if r.is_terminal() {
            if let (Some(s), Some(t)) = (r.started_ms, r.ended_ms) {
                e.duration_ms += t.saturating_sub(s);
            }
        }
    }
    e.reads = read_paths.len() as u32;
    e.edits = edit_paths.len() as u32;
    e.writes = write_paths.len() as u32;
    Some(e)
}

fn part(n: u32, verb: &str, noun: &str) -> Option<String> {
    (n > 0).then(|| format!("{verb} {n} {noun}{}", if n == 1 { "" } else { "s" }))
}

/// Empty string when nothing was counted (the renderer drops it).
#[must_use]
pub fn turn_summary_text(e: &TurnSummaryEntry) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.extend(part(e.commands, "ran", "command"));
    parts.extend(part(e.reads, "read", "file"));
    parts.extend(part(e.edits, "edited", "file"));
    parts.extend(part(e.writes, "wrote", "file"));
    if e.others > 0 {
        parts.push(format!("{} other tool{}", e.others, if e.others == 1 { "" } else { "s" }));
    }
    if e.failed > 0 {
        parts.push(format!("{} failed", e.failed));
    }
    if parts.is_empty() {
        return String::new();
    }
    let mut s = parts.join(", ");
    if let Some(f) = s.chars().next() {
        if f.is_ascii_lowercase() {
            s.replace_range(..1, &f.to_ascii_uppercase().to_string());
        }
    }
    if e.duration_ms > 0 {
        s.push_str(&format!(" · {}", fmt_duration_ms(e.duration_ms)));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn row(tool: &str, args: serde_json::Value, ok: bool) -> ToolRow {
        let mut r = ToolRow::new("id", tool, &args);
        r.started_ms = Some(0);
        r.ended_ms = Some(14_000);
        r.status = if ok { RowStatus::Ok { duration_ms: 14_000 } } else { RowStatus::Err { duration_ms: 14_000, message: "x".into() } };
        r
    }
    #[test]
    fn formats_in_fixed_order_with_plurals_and_a_capital() {
        // NOTE: the brief's draft used the tool name "shell_exec" here, but
        // Task 4 (summarize.rs::DISPLAY_NAMES, see its doc comment) already
        // settled the registered shell-tool name as "bash" — "shell_exec"
        // humanises to "Shell Exec" and falls into the `_ => e.others += 1`
        // arm instead of `"Bash" => e.commands += 1`, which would silently
        // turn this into "3 other tools" and fail the assertion below.
        // Using the real registered name ("bash") is the verbatim-correct
        // fix, not a judgment call; see task-8-report.md for the RED run
        // that demonstrated the brief's literal fixture failing.
        let rows = vec![
            row("bash", json!({"command": "ls"}), true),
            row("bash", json!({"command": "pwd"}), true),
            row("bash", json!({"command": "id"}), false),
            row("file_read", json!({"path": "a.rs"}), true),
            row("file_read", json!({"path": "a.rs"}), true), // same file counts once
            row("file_read", json!({"path": "b.rs"}), true),
            row("file_edit", json!({"file_path": "c.rs"}), true),
            row("file_write", json!({"file_path": "d.rs"}), true),
        ];
        let e = summarize_turn(&rows).unwrap();
        assert_eq!(turn_summary_text(&e), "Ran 3 commands, read 2 files, edited 1 file, wrote 1 file, 1 failed · 1m 52s");
    }
    #[test]
    fn one_tool_is_below_the_gate() {
        assert!(summarize_turn(&[row("file_read", json!({"path": "a"}), true)]).is_none());
    }
    #[test]
    fn a_pending_row_with_a_stale_ended_ms_contributes_zero_duration() {
        // `settle_resumed` deliberately leaves `ended_ms` set on a row it
        // settles to `Pending` (an outcome it refuses to vouch for) — see
        // view_model.rs's own `a_running_row_with_ended_ms_set_still_settles_to_pending_not_a_fabricated_ok`.
        // Billing wall-clock time for that row would report a duration for
        // something we just said we cannot claim. Must match
        // `ToolGroup::headline`'s existing rule: only terminal (Ok/Err) rows
        // contribute duration — one derivation for "how long did this take",
        // shared by both call sites.
        let mut pending = ToolRow::new("id", "file_read", &json!({"path": "a.rs"}));
        pending.started_ms = Some(0);
        pending.ended_ms = Some(9_000);
        pending.status = RowStatus::Pending;
        let terminal = row("file_read", json!({"path": "b.rs"}), true); // 14_000ms, Ok
        let e = summarize_turn(&[pending, terminal]).unwrap();
        assert_eq!(e.duration_ms, 14_000, "the Pending row's stale ended_ms must not be billed");
    }
}
