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

use std::borrow::Cow;

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

    /// Whether this reduction is worth emitting: the body must be at most
    /// [`MAX_KEPT_BYTES_X10`] tenths of `input`'s bytes.
    ///
    /// Measured in **bytes**, deliberately. Every reducer's own guard counts
    /// lines (or, for JSON, allowed any strict improvement at all), and line
    /// count is exactly the unit that doesn't bound context: one kept 200 KB
    /// line is a 94 % "line reduction" and a 1 % token reduction.
    fn is_meaningful_shrink(&self, input: &str) -> bool {
        self.body.len() * 10 <= input.len() * MAX_KEPT_BYTES_X10
    }
}

/// Classify then reduce a tool-result body.
///
/// Tries each candidate type in most-specific-first order and returns the first
/// reducer that produces a **meaningful shrink**. Falling through matters: the
/// cheap `looks_like_*` gates are heuristics, and a gate that fires on content
/// its reducer then declines used to end the attempt outright. `rg --json` is
/// the case that made this concrete — ndjson satisfies
/// [`json::looks_like_json`] (first line `{`, last line `}`), then
/// `serde_json::from_str` rejects the multi-document body, and the search/log
/// reducers were never consulted at all.
///
/// Returns `None` when no candidate produces a worthwhile reduction — the
/// caller then keeps its own fallback (a first-line placeholder for the stale
/// pass, head/tail truncation at ingress), which is safe for prose.
#[must_use]
pub fn reduce(text: &str) -> Option<Reduction> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() < MIN_LINES {
        return None;
    }
    for kind in candidates(&lines) {
        let reduced = match kind {
            ContentKind::Diff => diff::reduce_diff(text),
            ContentKind::Search => search::reduce_search(text),
            ContentKind::Json => json::reduce_json(text),
            ContentKind::Log => log::reduce_log(text),
        };
        // Central size guard — the single place that decides whether a
        // reduction was worth it. Each reducer only has to decide *what* is
        // signal; whether the result is actually smaller is measured once,
        // here, in bytes. Before this existed, `reduce_diff` could return a
        // body *larger* than its input (its only check compared line counts)
        // and `reduce_json` reported a 91-byte saving on a 91 KB document as a
        // success, so `Some(_)` carried no size guarantee for the caller at all.
        if let Some(reduction) = reduced.filter(|r| r.is_meaningful_shrink(text)) {
            return Some(reduction);
        }
    }
    None
}

/// Candidate content types for `lines`, most specific first. Empty when nothing
/// matches. Ordering rationale:
///
/// - diff has unmistakable structural markers, so it wins outright;
/// - JSON is a brace/bracket-delimited whole document;
/// - search has a rigid `path:line:` shape;
/// - log is the broad fallback, gated on clear command/build/test signals so
///   ordinary prose is never routed here.
fn candidates(lines: &[&str]) -> Vec<ContentKind> {
    let mut out = Vec::new();
    if diff::looks_like_diff(lines) {
        // A diff is the one shape where falling through is *worse* than doing
        // nothing: the log reducer keeps "loud" lines and drops the rest, and in a
        // diff that means deleting every `-` line and every `@@` header — it
        // returns something that still looks like a diff but describes a different
        // change. So a diff is offered to its own reducer only; if that declines,
        // the caller's head/tail truncation is the safe fallback.
        return vec![ContentKind::Diff];
    }
    if json::looks_like_json(lines) {
        out.push(ContentKind::Json);
    }
    if search::looks_like_search(lines) {
        out.push(ContentKind::Search);
    }
    if log::looks_like_log(lines) {
        out.push(ContentKind::Log);
    }
    out
}

/// The single best-guess content type, or `None`.
///
/// Test-only introspection over [`candidates`]. Production went through this
/// function until `reduce` started walking the whole candidate list, and leaving
/// it as a `pub` fn with zero production callers would just be a second way to
/// ask the question — one that answers "Json" for the ndjson case `reduce`
/// deliberately no longer stops at.
#[cfg(test)]
#[must_use]
pub fn classify(text: &str) -> Option<ContentKind> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() < MIN_LINES {
        return None;
    }
    candidates(&lines).first().copied()
}

/// Below this line count, structured reduction isn't worth the header cost;
/// the caller's first-line placeholder already handles tiny results.
const MIN_LINES: usize = 8;

/// A reduction must leave at most this fraction (in tenths) of the input's
/// bytes to be worth emitting. A hair under the input is not a reduction — it
/// is a header plus a rounding error, and it costs the caller the chance to
/// apply a real one.
const MAX_KEPT_BYTES_X10: usize = 9;

/// Char cap applied to every line a line-oriented reducer keeps.
///
/// Without it, all of the "kept N/M lines" arithmetic is measured in the one
/// unit that doesn't matter: a `rg` hit inside a minified bundle is a single
/// 200 KB line, so "kept 5/40 lines" could still be a megabyte of context. pi
/// clamps grep match lines at 500 chars for exactly this reason
/// (`GREP_MAX_LINE_LENGTH`), and `file_ops/text.rs::clamp_line` is the same idea
/// on the read path.
const MAX_LINE_CHARS: usize = 500;

/// Clamp one line to [`MAX_LINE_CHARS`] characters, char-boundary safe (P7 —
/// never slice a multi-byte character). Returns [`Cow::Borrowed`] for the
/// overwhelmingly common short line, so clamping allocates nothing.
fn clamp_line(line: &str) -> Cow<'_, str> {
    // `chars().count()` is O(n); skip it entirely when the byte length already
    // proves the line is short (a char is at least one byte).
    if line.len() <= MAX_LINE_CHARS {
        return Cow::Borrowed(line);
    }
    let mut kept = String::with_capacity(MAX_LINE_CHARS + 16);
    for (seen, c) in line.chars().enumerate() {
        if seen == MAX_LINE_CHARS {
            let dropped = line.chars().count() - MAX_LINE_CHARS;
            kept.push_str(&format!("… (+{dropped} chars, line truncated)"));
            return Cow::Owned(kept);
        }
        kept.push(c);
    }
    Cow::Owned(kept)
}

/// Render a subset of `lines` identified by the sorted, deduped `kept` indices.
///
/// A `… (N lines omitted) …` marker is emitted for **every** gap — including a
/// leading gap before the first kept index and a trailing gap after the last.
/// The trailing marker is why `total` is a parameter: without it a reducer that
/// dropped everything after line 240 rendered a body that simply *stopped*, and
/// a model reading a 26-file diff truncated to its first 3 files had no way to
/// know 23 files were missing.
///
/// `kept` must be ascending and in bounds. Shared by the log and diff reducers
/// (search renders per file).
pub(super) fn render_selected(lines: &[&str], kept: &[usize], total: usize) -> String {
    let mut out = String::new();
    let push_gap = |out: &mut String, gap: usize| {
        if gap == 0 {
            return;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("… ({gap} lines omitted) …"));
    };

    let mut prev: Option<usize> = None;
    for &idx in kept {
        match prev {
            Some(p) => push_gap(&mut out, idx.saturating_sub(p + 1)),
            // Leading gap: detail dropped before the first kept line.
            None => push_gap(&mut out, idx),
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&clamp_line(lines[idx]));
        prev = Some(idx);
    }
    if let Some(last) = prev {
        push_gap(&mut out, total.saturating_sub(last + 1));
    }
    out
}

/// Case-insensitive ASCII substring test that allocates nothing.
///
/// The obvious `line.to_ascii_lowercase().contains(n)` allocated a full copy of
/// every line, in both the `looks_like_*` pass and the reduce pass — four
/// whole-line allocations per line, which for the 200 KB minified line above is
/// 800 KB of copying to decide what to throw away. `needle` must already be
/// lowercase.
fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    let (h, n) = (haystack.as_bytes(), needle.as_bytes());
    if n.is_empty() || h.len() < n.len() {
        return n.is_empty();
    }
    // `eq_ignore_ascii_case` on bytes folds only ASCII and compares the rest
    // exactly, so a non-ASCII needle (`✗`) still matches byte-for-byte.
    h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
}

/// Case-insensitive substring test for error/failure signals, shared by the log
/// and search reducers so the two stay consistent about what counts as "loud".
pub(super) fn is_error_signal(line: &str) -> bool {
    // Android logcat error lines ("E/Tag: message") put the marker at the
    // start of the (possibly indented) line — as a bare substring needle it
    // would false-positive on any path containing "e/", so it gets a trimmed
    // prefix check instead.
    let trimmed = line.trim_start();
    if trimmed.len() >= 2 && trimmed.as_bytes()[1] == b'/' && trimmed.as_bytes()[0] | 0x20 == b'e' {
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
    NEEDLES.iter().any(|n| contains_ignore_ascii_case(line, n))
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
        let body = render_selected(&lines, &[0, 4], lines.len());
        assert_eq!(body, "a\n… (3 lines omitted) …\ne");
    }

    #[test]
    fn render_selected_contiguous_has_no_marker() {
        let lines = vec!["a", "b", "c"];
        let body = render_selected(&lines, &[0, 1, 2], lines.len());
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
