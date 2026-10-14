use extism_pdk::*;
use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};

// ---------------------------------------------------------------------------
// Input / Output types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct DiffTextInput {
    old_text: String,
    new_text: String,
    context_lines: Option<usize>,
    format: Option<String>,
}

#[derive(Deserialize)]
struct DiffFilesInput {
    file_a: String,
    file_b: String,
    #[allow(dead_code)]
    context_lines: Option<usize>,
    #[allow(dead_code)]
    format: Option<String>,
}

#[derive(Deserialize)]
struct DiffSummaryInput {
    old_text: String,
    new_text: String,
}

#[derive(Serialize)]
struct DiffOutput {
    diff: String,
    format: String,
}

#[derive(Serialize)]
struct SummaryOutput {
    lines_added: usize,
    lines_removed: usize,
    lines_unchanged: usize,
    change_ratio: f64,
    total_lines: usize,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Count insertions, deletions, and unchanged lines from a `TextDiff`.
fn count_changes(diff: &TextDiff<'_, '_, '_, str>) -> (usize, usize, usize) {
    let mut added: usize = 0;
    let mut removed: usize = 0;
    let mut unchanged: usize = 0;

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => added += 1,
            ChangeTag::Delete => removed += 1,
            ChangeTag::Equal => unchanged += 1,
        }
    }

    (added, removed, unchanged)
}

/// Produce a unified diff string with the given number of context lines.
fn unified_diff(old: &str, new: &str, context_lines: usize) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut out = String::new();

    // Header
    out.push_str("--- old\n");
    out.push_str("+++ new\n");

    for hunk in diff.unified_diff().context_radius(context_lines).iter_hunks() {
        out.push_str(&hunk.to_string());
    }

    out
}

/// Produce an inline diff where each line is prefixed with a tag.
fn inline_diff(old: &str, new: &str) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut out = String::new();

    for change in diff.iter_all_changes() {
        let prefix = match change.tag() {
            ChangeTag::Insert => "[+] ",
            ChangeTag::Delete => "[-] ",
            ChangeTag::Equal => "[~] ",
        };
        out.push_str(prefix);
        out.push_str(change.value());
        // Ensure trailing newline
        if !change.value().ends_with('\n') {
            out.push('\n');
        }
    }

    out
}

/// Produce a stats-only output.
fn stats_diff(old: &str, new: &str) -> String {
    let diff = TextDiff::from_lines(old, new);
    let (added, removed, unchanged) = count_changes(&diff);
    format!(
        "Lines added:     {}\nLines removed:   {}\nLines unchanged: {}\n",
        added, removed, unchanged
    )
}

// ---------------------------------------------------------------------------
// Plugin exports
// ---------------------------------------------------------------------------

#[plugin_fn]
pub fn diff_text(Json(input): Json<DiffTextInput>) -> FnResult<Json<DiffOutput>> {
    let ctx = input.context_lines.unwrap_or(3);
    let fmt = input.format.as_deref().unwrap_or("unified");

    let diff_str = match fmt {
        "unified" => unified_diff(&input.old_text, &input.new_text, ctx),
        "inline" => inline_diff(&input.old_text, &input.new_text),
        "stats" => stats_diff(&input.old_text, &input.new_text),
        other => {
            return Err(Error::msg(format!(
                "Unknown format '{}'. Supported: unified, inline, stats",
                other
            )));
        }
    };

    Ok(Json(DiffOutput {
        diff: diff_str,
        format: fmt.to_string(),
    }))
}

#[plugin_fn]
pub fn diff_files(Json(input): Json<DiffFilesInput>) -> FnResult<Json<DiffOutput>> {
    Err(Error::msg(format!(
        "File reading requires host workspace capability. \
         Pass file contents via diff_text instead. \
         (requested: '{}' vs '{}')",
        input.file_a, input.file_b
    )))
}

#[plugin_fn]
pub fn diff_summary(Json(input): Json<DiffSummaryInput>) -> FnResult<Json<SummaryOutput>> {
    let diff = TextDiff::from_lines(input.old_text.as_str(), input.new_text.as_str());
    let (added, removed, unchanged) = count_changes(&diff);
    let total = added + removed + unchanged;
    let change_ratio = if total == 0 {
        0.0
    } else {
        ((added + removed) as f64 / total as f64) * 100.0
    };

    Ok(Json(SummaryOutput {
        lines_added: added,
        lines_removed: removed,
        lines_unchanged: unchanged,
        change_ratio,
        total_lines: total,
    }))
}
