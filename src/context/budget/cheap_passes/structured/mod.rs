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
}

impl ContentKind {
    /// Short label used in the honest "this was compacted" header.
    const fn label(self) -> &'static str {
        match self {
            Self::Log => "log",
            Self::Search => "search",
            Self::Diff => "diff",
        }
    }
}

/// Outcome of a structured reduction: the kept body plus a kept/total line
/// tally, so the caller can emit an honest header telling the model the result
/// was compacted (and roughly how much was dropped).
pub struct Reduction {
    pub kind: ContentKind,
    /// The reduced body (signal-preserving), without the header line.
    pub body: String,
    /// Lines kept in `body` (excludes the omission markers).
    pub kept_lines: usize,
    /// Lines in the original input.
    pub total_lines: usize,
}

impl Reduction {
    /// Render the full replacement text: an honest header line + the reduced
    /// body. The header doubles as a signal to the model that this result is
    /// partial, so it can re-run the tool if it needs the dropped detail.
    #[must_use]
    pub fn render(&self) -> String {
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
        ContentKind::Log => log::reduce_log(text),
    }
}

/// Cheap whole-text classification. Checks most-specific types first (diff has
/// unmistakable markers; search has a rigid `path:line:` shape; log is the
/// broad fallback gated on clear command/build/test signals so ordinary prose
/// is never misclassified).
#[must_use]
pub fn classify(text: &str) -> Option<ContentKind> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() < MIN_LINES {
        return None;
    }
    if diff::looks_like_diff(&lines) {
        return Some(ContentKind::Diff);
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
    const NEEDLES: [&str; 11] = [
        "error", "warning", "failed", "failure", "panic", "exception", "traceback", "fatal",
        "assert", " e/", "✗",
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
    fn diff_wins_over_log_classification() {
        // A diff that also contains the word "error" must classify as Diff, not
        // Log — most-specific-first ordering.
        let d = "diff --git a/x.rs b/x.rs\n@@ -1,3 +1,3 @@\n-let x = error();\n+let x = ok();\n context\n context2\n context3\n context4\n";
        assert_eq!(classify(d), Some(ContentKind::Diff));
    }
}
