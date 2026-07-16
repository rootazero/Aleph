//! Content-type-aware structured reduction of stale tool results.
//!
//! Headroom's core insight: compression should route by *content type*, not
//! treat every blob the same. A grep result, a test log, and a git diff each
//! have a distinct "signal" — matched lines, error/summary lines, `+`/`-`
//! change lines — that a blunt first-line truncation throws away. This module
//! classifies a tool-result body with cheap deterministic heuristics and
//! applies a type-specific reducer that keeps the signal at a fraction of the
//! tokens.
//!
//! Everything here is deterministic line processing — no LLM, no tree-sitter,
//! no regex engine, no new dependency (R3 core minimalism). It plugs into
//! [`ToolResultPruningStage`](super::tool_result_pruning::ToolResultPruningStage)
//! as a smarter alternative to the first-line placeholder: prose that doesn't
//! match a recognized type returns `None`, and the caller keeps its existing
//! behaviour.

mod diff;
mod json;
mod log;
mod search;

/// Recognized structured content types worth a tailored reducer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentKind {
    /// Command / build / test output (cargo, pytest, npm, make, generic logs).
    Log,
    /// grep / ripgrep style `path:line:content` search results.
    Search,
    /// Unified diff (`git diff` / `diff -u`).
    Diff,
    /// JSON document / array (API response, config dump, structured output).
    Json,
}

impl ContentKind {
    /// Short label used in the honest "this was compacted" header.
    const fn label(self) -> &'static str {
        match self {
            Self::Log => "log",
            Self::Search => "search",
            Self::Diff => "diff",
            Self::Json => "json",
        }
    }
}

/// Outcome of a structured reduction: the kept body plus a kept/total tally,
/// so the caller can emit an honest header telling the model the result was
/// compacted (and roughly how much was dropped).
pub struct Reduction {
    pub kind: ContentKind,
    /// The reduced body (signal-preserving), without the header line.
    pub body: String,
    /// Kept tally for `body`. Unit depends on the reducer: lines for the
    /// line-oriented kinds (log / search / diff, excluding omission markers),
    /// chars for [`ContentKind::Json`] — its body is re-pretty-printed, so a
    /// line tally would be dishonest ("kept 43/1 lines" for a dense blob).
    pub kept_lines: usize,
    /// Tally for the original input, in the same unit as `kept_lines`.
    pub total_lines: usize,
}

impl Reduction {
    /// Render the full replacement text: an honest header line + the reduced
    /// body. The header doubles as a signal to the model that this result is
    /// partial, so it can re-run the tool if it needs the dropped detail.
    #[must_use]
    pub fn render(&self) -> String {
        // JSON is tallied in chars (see `kept_lines`) — render the matching
        // unit so the header counts what the reducer actually measured.
        if matches!(self.kind, ContentKind::Json) {
            return format!(
                "[compacted {}: reduced {}→{} chars]\n{}",
                self.kind.label(),
                self.total_lines,
                self.kept_lines,
                self.body
            );
        }
        format!(
            "[compacted {}: kept {}/{} lines]\n{}",
            self.kind.label(),
            self.kept_lines,
            self.total_lines,
            self.body
        )
    }
}

/// Classify then reduce a tool-result body.
///
/// Returns `None` when the content isn't a recognized structured type (the
/// caller falls back to first-line truncation, which is safe for prose), or
/// when the matched reducer decides the content is all signal and not worth
/// reducing.
#[must_use]
pub fn reduce(text: &str) -> Option<Reduction> {
    match classify(text)? {
        ContentKind::Diff => diff::reduce_diff(text),
        ContentKind::Search => search::reduce_search(text),
        ContentKind::Json => json::reduce_json(text),
        ContentKind::Log => log::reduce_log(text),
    }
}

/// Cheap whole-text classification. Checks most-specific types first (diff has
/// unmistakable markers; JSON is a brace/bracket-delimited document; search has
/// a rigid `path:line:` shape; log is the broad fallback gated on clear
/// command/build/test signals so ordinary prose is never misclassified).
#[must_use]
pub fn classify(text: &str) -> Option<ContentKind> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() < MIN_LINES {
        return None;
    }
    if diff::looks_like_diff(&lines) {
        return Some(ContentKind::Diff);
    }
    // JSON is brace/bracket-delimited — an unmistakable whole-document shape
    // that never collides with the `path:line:` search texture below (a JSON
    // `"key": value` line has no `:<digits>:` line-number marker).
    if json::looks_like_json(&lines) {
        return Some(ContentKind::Json);
    }
    if search::looks_like_search(&lines) {
        return Some(ContentKind::Search);
    }
    if log::looks_like_log(&lines) {
        return Some(ContentKind::Log);
    }
    None
}

/// Below this line count, structured reduction isn't worth the header cost;
/// the caller's first-line placeholder already handles tiny results.
const MIN_LINES: usize = 8;

/// Render a subset of `lines` identified by the sorted, deduped `kept` indices,
/// inserting a `… (N lines omitted) …` marker between non-contiguous runs so
/// the model can see where detail was dropped. `kept` must be ascending and in
/// bounds. Shared by the log and diff reducers (search renders per file).
pub(super) fn render_selected(lines: &[&str], kept: &[usize]) -> String {
    let mut out = String::new();
    let mut prev: Option<usize> = None;
    for &idx in kept {
        if let Some(p) = prev {
            let gap = idx.saturating_sub(p + 1);
            if gap > 0 {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&format!("… ({gap} lines omitted) …"));
            }
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(lines[idx]);
        prev = Some(idx);
    }
    out
}

/// Lower-cased substring test for error/failure signals, shared by the log and
/// search reducers so the two stay consistent about what counts as "loud".
pub(super) fn is_error_signal(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    // Android logcat error lines ("E/Tag: message") put the marker at the
    // start of the (possibly indented) line — as a bare substring needle it
    // would false-positive on any path containing "e/", so it gets a trimmed
    // prefix check instead.
    if l.trim_start().starts_with("e/") {
        return true;
    }
    const NEEDLES: [&str; 10] = [
        "error",
        "warning",
        "failed",
        "failure",
        "panic",
        "exception",
        "traceback",
        "fatal",
        "assert",
        "✗",
    ];
    NEEDLES.iter().any(|n| l.contains(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prose_is_not_classified() {
        let prose = "This is an ordinary paragraph of prose.\n".repeat(20);
        assert_eq!(classify(&prose), None, "prose must not match any reducer");
        assert!(reduce(&prose).is_none());
    }

    #[test]
    fn tiny_input_is_not_reduced() {
        let tiny = "error: boom\nwarning: x\n";
        assert_eq!(classify(tiny), None, "under MIN_LINES → no reduction");
    }

    #[test]
    fn render_selected_marks_gaps() {
        let lines = vec!["a", "b", "c", "d", "e"];
        // Keep 0 and 4 — three lines (1,2,3) omitted between them.
        let body = render_selected(&lines, &[0, 4]);
        assert_eq!(body, "a\n… (3 lines omitted) …\ne");
    }

    #[test]
    fn render_selected_contiguous_has_no_marker() {
        let lines = vec!["a", "b", "c"];
        let body = render_selected(&lines, &[0, 1, 2]);
        assert_eq!(body, "a\nb\nc");
    }

    #[test]
    fn logcat_error_lines_are_error_signals() {
        // Brief-format logcat error lines start at column 0 — the old " e/"
        // needle (leading space) could never match them.
        assert!(is_error_signal("E/ActivityManager: ANR in com.example"));
        assert!(is_error_signal("  E/Tag: indented variant"));
        // …while a mid-line "e/" (e.g. a path segment) must not be loud.
        assert!(!is_error_signal("copied assets to build e/output dir"));
    }

    #[test]
    fn diff_wins_over_log_classification() {
        // A diff that also contains the word "error" must classify as Diff, not
        // Log — most-specific-first ordering.
        let d = "diff --git a/x.rs b/x.rs\n@@ -1,3 +1,3 @@\n-let x = error();\n+let x = ok();\n context\n context2\n context3\n context4\n";
        assert_eq!(classify(d), Some(ContentKind::Diff));
    }
}
