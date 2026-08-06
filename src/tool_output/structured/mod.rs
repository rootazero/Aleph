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
//! `ToolResultPruningStage` as a smarter alternative to the first-line
//! placeholder: prose that doesn't match a recognized type returns `None`, and
//! the caller keeps its existing behaviour.
//!
//! # Who decides what
//!
//! - **What counts as signal** is the individual reducer's, and only that.
//! - **How small is small enough** is the *caller's*, expressed as a token
//!   budget and handed to [`reduce_within`]. Every reducer's size knob then
//!   comes from the resulting [`Profile`] instead of a module-private constant,
//!   so a tool that declares a 6 000-token budget gets a reduction sized for
//!   6 000 tokens rather than one sized for nothing in particular and then cut
//!   by a blind truncator downstream.
//! - **Whether the reduction was worth emitting** is neither: it is measured
//!   once, in bytes, by [`Reduction::is_meaningful_shrink`].

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

    /// Minimum line count this reducer's *own shape* requires before it is
    /// worth consulting.
    ///
    /// This used to be one global gate applied ahead of classification, and that
    /// shape mismatch made the JSON reducer structurally unreachable for the
    /// single most common large tool result there is. `Value::to_string()`,
    /// `curl`, every `--format json` flag and every MCP text result emit
    /// **compact JSON on one line** — one line is under any line-count floor, so
    /// classification returned `None` before `looks_like_json` was ever asked,
    /// and a 300 KB API response fell through to a head/tail byte slice that
    /// cuts JSON into invalid syntax mid-structure. A line floor is a
    /// precondition of the *line-oriented* reducers; JSON's floor is
    /// [`MIN_INPUT_BYTES`], which applies to every kind anyway.
    const fn min_lines(self) -> usize {
        match self {
            Self::Json => 1,
            Self::Log | Self::Search | Self::Diff => MIN_LINES,
        }
    }
}

/// What a [`Reduction`]'s kept/total tally counts.
///
/// The unit is not incidental and must not be inferred at render time. The
/// line-oriented reducers select whole lines out of the input's own lines, so
/// "kept 43/812 lines" states the same sequence twice. The JSON reducer
/// re-serializes a value tree, so its output's line count bears no relation to
/// the input's — a dense single-line blob re-renders as dozens of lines and the
/// header read `kept 43/1 lines`, claiming to have kept more than existed.
/// Carrying the unit in the value means [`Reduction::render`] cannot mismatch
/// it, and a new reducer has to state its unit to compile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tally {
    /// Whole lines selected out of the input's lines (log / search / diff).
    Lines { kept: usize, total: usize },
    /// Characters, for a reducer that re-serializes rather than selects (json).
    Chars { kept: usize, total: usize },
}

impl Tally {
    /// The honest header phrasing for this unit.
    fn describe(self) -> String {
        match self {
            Self::Lines { kept, total } => format!("kept {kept}/{total} lines"),
            Self::Chars { kept, total } => format!("reduced {total}→{kept} chars"),
        }
    }

    /// Kept count, in this tally's own unit.
    ///
    /// Test-only. Production reads the tally through [`Tally::describe`] (the
    /// header) and never needs the raw number; a `pub` accessor with no
    /// production caller is a second way to ask a question nobody is asking
    /// (R10). Assertions do need it, so it lives here rather than being deleted.
    #[cfg(test)]
    #[must_use]
    pub const fn kept(self) -> usize {
        match self {
            Self::Lines { kept, .. } | Self::Chars { kept, .. } => kept,
        }
    }

    /// Total count, in this tally's own unit. Test-only, see [`Tally::kept`].
    #[cfg(test)]
    #[must_use]
    pub const fn total(self) -> usize {
        match self {
            Self::Lines { total, .. } | Self::Chars { total, .. } => total,
        }
    }
}

/// Outcome of a structured reduction: the kept body plus a [`Tally`], so the
/// caller can emit an honest header telling the model the result was compacted
/// (and roughly how much was dropped).
pub struct Reduction {
    pub kind: ContentKind,
    /// The reduced body (signal-preserving), without the header line.
    pub body: String,
    /// Kept/total tally for `body`, carrying its own unit.
    pub tally: Tally,
}

impl Reduction {
    /// Render the full replacement text: an honest header line + the reduced
    /// body. The header doubles as a signal to the model that this result is
    /// partial, so it can re-run the tool if it needs the dropped detail.
    #[must_use]
    pub fn render(&self) -> String {
        format!(
            "[compacted {}: {}]\n{}",
            self.kind.label(),
            self.tally.describe(),
            self.body
        )
    }

    /// Whether this reduction is worth emitting: the body must be at most
    /// [`MAX_KEPT_BYTES_X10`] tenths of `input`'s bytes.
    ///
    /// Measured in **bytes**, deliberately. Every reducer's own guard counted
    /// lines (or, for JSON, allowed any strict improvement at all), and line
    /// count is exactly the unit that doesn't bound context: one kept 200 KB
    /// line is a 94 % "line reduction" and a 1 % token reduction.
    fn is_meaningful_shrink(&self, input: &str) -> bool {
        self.body.len() * 10 <= input.len() * MAX_KEPT_BYTES_X10
    }
}

/// Every size knob the reducers read, derived once from the caller's budget.
///
/// Before this existed each reducer hard-coded its caps, so a reduction was
/// sized for nothing in particular: the diff reducer could emit 240 lines of up
/// to 500 chars each — 120 KB — into a context slot the caller had already
/// measured at 6 000 tokens, and `apply_result_budget` then handed that
/// carefully signal-selected body to a *blind* head/tail truncator. The
/// component that knows which lines matter has to be the one that decides how
/// many of them fit.
///
/// All fields are character or item counts, never bytes: the reducers clamp by
/// characters (P7 — a byte cap slices multi-byte characters), and the
/// budget→characters conversion is the project's single source, reached through
/// [`super::scale_to_budget`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profile {
    /// Chars kept per rendered line by the line-oriented reducers.
    pub line_chars: usize,
    /// Loud lines the log reducer may keep.
    pub log_signal: usize,
    /// Lines the diff reducer may keep.
    pub diff_lines: usize,
    /// Match lines the search reducer may keep overall.
    pub search_total: usize,
    /// Match lines the search reducer may keep per file.
    pub search_per_file: usize,
    /// Distinct files the search reducer opens a group for.
    pub search_files: usize,
    /// Chars kept from an oversized JSON string leaf.
    pub json_string_chars: usize,
    /// Elements kept from an oversized JSON array.
    pub json_array_elems: usize,
    /// Keys kept from an oversized JSON object.
    pub json_object_keys: usize,
    /// Records kept from a JSONL / ndjson stream.
    pub jsonl_records: usize,
}

impl Profile {
    /// The caps the module shipped with, and the ceiling every derived profile
    /// is clamped to. They are not merely a default: `log_signal = 24` also
    /// encodes "a digest orients, it does not reproduce the log", so a caller
    /// with an enormous budget still gets a digest.
    pub const DEFAULT: Self = Self {
        line_chars: 500,
        log_signal: 24,
        diff_lines: 240,
        search_total: 60,
        search_per_file: 5,
        search_files: 20,
        json_string_chars: 200,
        json_array_elems: 8,
        json_object_keys: 48,
        jsonl_records: 12,
    };

    /// Floors: below these a reduction stops being a digest and becomes a
    /// rumour. A caller whose budget cannot pay for even this gets a reduction
    /// slightly over its budget rather than a body with no `+`/`-` line in it —
    /// and [`Reduction::is_meaningful_shrink`] plus the caller's own token guard
    /// still decide whether to take it.
    const FLOOR: Self = Self {
        line_chars: 80,
        log_signal: 4,
        diff_lines: 12,
        search_total: 8,
        search_per_file: 2,
        search_files: 3,
        json_string_chars: 40,
        json_array_elems: 2,
        json_object_keys: 8,
        jsonl_records: 2,
    };

    /// Scale every cap linearly with `budget_tokens`, clamped to
    /// `[FLOOR, DEFAULT]`.
    ///
    /// Scaling is [`super::scale_to_budget`], shared with the tier-2 digest cap,
    /// so at the default result budget every knob equals [`Self::DEFAULT`] and
    /// the reduction is byte-for-byte what it has always been. Only a tool
    /// declaring a *smaller* budget (or the stale pass, which asks for an
    /// aggressive one) sees tighter caps — and it sees them inside the reducer
    /// rather than as a blind cut afterwards.
    #[must_use]
    pub fn for_token_budget(budget_tokens: usize) -> Self {
        let d = Self::DEFAULT;
        let f = Self::FLOOR;
        let scaled = |default, floor| super::scale_to_budget(default, floor, budget_tokens);
        Self {
            line_chars: scaled(d.line_chars, f.line_chars),
            log_signal: scaled(d.log_signal, f.log_signal),
            diff_lines: scaled(d.diff_lines, f.diff_lines),
            search_total: scaled(d.search_total, f.search_total),
            search_per_file: scaled(d.search_per_file, f.search_per_file),
            search_files: scaled(d.search_files, f.search_files),
            json_string_chars: scaled(d.json_string_chars, f.json_string_chars),
            json_array_elems: scaled(d.json_array_elems, f.json_array_elems),
            json_object_keys: scaled(d.json_object_keys, f.json_object_keys),
            jsonl_records: scaled(d.jsonl_records, f.jsonl_records),
        }
    }
}

/// Classify then reduce a tool-result body with the default profile.
///
/// Test-only, for the same reason as [`classify`]: both production callers know
/// their budget and pass it, so a budget-less entry point is a second way to ask
/// the question — and the one that answers it with caps sized for a budget the
/// caller does not have. Kept because the reducers' own tests are about *what is
/// signal*, which the default profile expresses most plainly.
#[cfg(test)]
#[must_use]
pub fn reduce(text: &str) -> Option<Reduction> {
    reduce_within(text, None)
}

/// Classify then reduce a tool-result body, sized for `budget_tokens`.
///
/// Tries each candidate type in most-specific-first order and returns the first
/// reducer that produces a **meaningful shrink**. Falling through matters: the
/// cheap `looks_like_*` gates are heuristics, and a gate that fires on content
/// its reducer then declines used to end the attempt outright. `rg --json` is
/// the case that made this concrete — ndjson satisfies
/// [`json::looks_like_json`], then `serde_json::from_str` rejects the
/// multi-document body, and the search/log reducers were never consulted at all.
///
/// Returns `None` when no candidate produces a worthwhile reduction — the
/// caller then keeps its own fallback (a first-line placeholder for the stale
/// pass, head/tail truncation at ingress), which is safe for prose.
#[must_use]
pub fn reduce_within(text: &str, budget_tokens: Option<usize>) -> Option<Reduction> {
    // One size floor for every kind. The line floor that used to stand here is a
    // property of the *line-oriented* reducers and now lives on each kind (see
    // [`ContentKind::min_lines`]); what is universally true is only that below
    // some size a header costs more than the lines it drops.
    if text.len() < MIN_INPUT_BYTES {
        return None;
    }
    let profile = budget_tokens.map_or(Profile::DEFAULT, Profile::for_token_budget);
    let lines: Vec<&str> = text.lines().collect();
    for kind in candidates(&lines) {
        if lines.len() < kind.min_lines() {
            continue;
        }
        // The line reducers take the already-collected `lines` — each used to
        // re-collect `text.lines()` on its own, paying the split once per
        // candidate kind. `reduce_json` keeps the whole text: it parses rather
        // than selects lines, so it never collected them.
        let reduced = match kind {
            ContentKind::Diff => diff::reduce_diff(&lines, &profile),
            ContentKind::Search => search::reduce_search(&lines, &profile),
            ContentKind::Json => json::reduce_json(text, &profile),
            ContentKind::Log => log::reduce_log(&lines, &profile),
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
    candidates(&lines)
        .into_iter()
        .find(|kind| lines.len() >= kind.min_lines())
}

/// Below this line count, the line-oriented reducers have nothing to select
/// that the caller's first-line placeholder doesn't already handle.
const MIN_LINES: usize = 8;

/// Below this many bytes, no reduction is worth its header line — the one size
/// precondition that holds for every content type.
const MIN_INPUT_BYTES: usize = 512;

/// A reduction must leave at most this fraction (in tenths) of the input's
/// bytes to be worth emitting. A hair under the input is not a reduction — it
/// is a header plus a rounding error, and it costs the caller the chance to
/// apply a real one.
const MAX_KEPT_BYTES_X10: usize = 9;

/// Clamp one line to `max_chars` characters, char-boundary safe (P7 — never
/// slice a multi-byte character). Returns [`Cow::Borrowed`] for the
/// overwhelmingly common short line, so clamping allocates nothing.
///
/// Without this cap, all of the "kept N/M lines" arithmetic is measured in the
/// one unit that doesn't matter: a `rg` hit inside a minified bundle is a single
/// 200 KB line, so "kept 5/40 lines" could still be a megabyte of context. pi
/// clamps grep match lines at 500 chars for exactly this reason
/// (`GREP_MAX_LINE_LENGTH`), and `file_ops/text.rs::clamp_line` is the same idea
/// on the read path.
fn clamp_line(line: &str, max_chars: usize) -> Cow<'_, str> {
    // `chars().count()` is O(n); skip it entirely when the byte length already
    // proves the line is short (a char is at least one byte).
    if line.len() <= max_chars {
        return Cow::Borrowed(line);
    }
    // One pass for the head, one for the tail. The obvious version called
    // `chars().count()` over the *whole* line to compute the dropped count after
    // having already walked its head — two and a half traversals of a line whose
    // entire problem is that it is 200 KB long.
    let Some((split, _)) = line.char_indices().nth(max_chars) else {
        // Multi-byte content: more bytes than `max_chars` but fewer characters.
        return Cow::Borrowed(line);
    };
    let dropped = line[split..].chars().count();
    Cow::Owned(format!(
        "{}… (+{dropped} chars, line truncated)",
        &line[..split]
    ))
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
pub(super) fn render_selected(
    lines: &[&str],
    kept: &[usize],
    total: usize,
    profile: &Profile,
) -> String {
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
        out.push_str(&clamp_line(lines[idx], profile.line_chars));
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
///
/// `pub(crate)` because [`distill`](crate::tool_output::distill) had the same
/// per-line lowercase copy on the same hot path and now shares this.
pub(crate) fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
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
        assert!(
            reduce(tiny).is_none(),
            "under MIN_INPUT_BYTES → no reduction"
        );
    }

    /// A compact JSON document is one line by construction, and a head/tail cut
    /// through it produces invalid syntax. The line floor is about line texture,
    /// so it must not apply to the one reducer that parses instead of selecting.
    #[test]
    fn a_single_line_json_document_is_still_reduced() {
        let payload = "y".repeat(4000);
        let one_line = format!(
            "{{\"status\":\"error\",\"message\":\"connection refused\",\"body\":\"{payload}\"}}"
        );
        assert_eq!(one_line.lines().count(), 1, "precondition: one line");

        let r = reduce(&one_line).expect("a compact JSON document must still reduce");
        assert_eq!(r.kind, ContentKind::Json);
        let parsed: serde_json::Value =
            serde_json::from_str(&r.body).expect("the reduction must still be valid JSON");
        assert_eq!(
            parsed["status"], "error",
            "the salient scalars must survive"
        );
        assert_eq!(
            parsed["message"], "connection refused",
            "the salient scalars must survive"
        );
    }

    /// …but the line-oriented kinds keep their floor, or a three-line snippet
    /// gets a header that costs more than the lines it drops.
    #[test]
    fn the_line_floor_still_applies_to_the_line_oriented_kinds() {
        let short_log =
            "$ make\nerror: one\nerror: two\nerror: three\nBuild finished with 3 errors\n";
        assert!(short_log.lines().count() < MIN_LINES);
        assert!(reduce(short_log).is_none());
    }

    #[test]
    fn render_selected_marks_gaps() {
        let lines = vec!["a", "b", "c", "d", "e"];
        // Keep 0 and 4 — three lines (1,2,3) omitted between them.
        let body = render_selected(&lines, &[0, 4], lines.len(), &Profile::DEFAULT);
        assert_eq!(body, "a\n… (3 lines omitted) …\ne");
    }

    #[test]
    fn render_selected_contiguous_has_no_marker() {
        let lines = vec!["a", "b", "c"];
        let body = render_selected(&lines, &[0, 1, 2], lines.len(), &Profile::DEFAULT);
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

    /// A line whose byte length exceeds the cap but whose *character* count does
    /// not must survive verbatim — the byte-length fast path is an optimisation,
    /// not the predicate.
    #[test]
    fn clamp_line_does_not_cut_multibyte_lines_that_fit_in_chars() {
        let cjk = "上".repeat(100); // 300 bytes, 100 chars
        assert!(cjk.len() > 200);
        assert!(matches!(clamp_line(&cjk, 200), Cow::Borrowed(_)));
        // …and one that genuinely overflows is cut on a character boundary.
        let long = "上".repeat(400);
        let clamped = clamp_line(&long, 200);
        assert!(clamped.starts_with(&"上".repeat(200)));
        assert!(clamped.contains("+200 chars"), "got: {clamped}");
    }

    /// The reference budget reproduces the shipped caps exactly, so the
    /// overwhelmingly common tool call is byte-for-byte unaffected by profiles
    /// existing at all.
    #[test]
    fn the_default_budget_reproduces_the_default_profile() {
        assert_eq!(
            Profile::for_token_budget(
                crate::tools::result_processing::DEFAULT_RESULT_BUDGET_TOKENS
            ),
            Profile::DEFAULT,
        );
        // A larger budget must not *raise* the caps: `log_signal` also encodes
        // "a digest orients, it does not reproduce the log".
        assert_eq!(Profile::for_token_budget(1_000_000), Profile::DEFAULT);
    }

    /// A smaller budget tightens every knob and never drops below the floor — a
    /// reduction with no signal left in it is worse than no reduction.
    #[test]
    fn a_smaller_budget_tightens_every_knob_but_never_past_the_floor() {
        let small = Profile::for_token_budget(600);
        let d = Profile::DEFAULT;
        let f = Profile::FLOOR;
        for (label, got, def, floor) in [
            ("line_chars", small.line_chars, d.line_chars, f.line_chars),
            ("log_signal", small.log_signal, d.log_signal, f.log_signal),
            ("diff_lines", small.diff_lines, d.diff_lines, f.diff_lines),
            (
                "search_total",
                small.search_total,
                d.search_total,
                f.search_total,
            ),
            (
                "search_per_file",
                small.search_per_file,
                d.search_per_file,
                f.search_per_file,
            ),
            (
                "search_files",
                small.search_files,
                d.search_files,
                f.search_files,
            ),
            (
                "json_string_chars",
                small.json_string_chars,
                d.json_string_chars,
                f.json_string_chars,
            ),
            (
                "json_array_elems",
                small.json_array_elems,
                d.json_array_elems,
                f.json_array_elems,
            ),
            (
                "json_object_keys",
                small.json_object_keys,
                d.json_object_keys,
                f.json_object_keys,
            ),
            (
                "jsonl_records",
                small.jsonl_records,
                d.jsonl_records,
                f.jsonl_records,
            ),
        ] {
            assert!(got < def, "{label}: a 600-token budget must tighten {def}");
            assert!(got >= floor, "{label}: {got} fell below the floor {floor}");
        }
    }

    /// The headline connection: compact single-line JSON — `curl`, every
    /// `--format json` flag, every flattened tool envelope — used to be
    /// structurally unreachable, because the line floor was applied to *all*
    /// kinds and the one content type whose canonical wire form has no newline
    /// therefore never reached its own reducer.
    #[test]
    fn compact_single_line_json_is_reachable() {
        let payload = "y".repeat(4000);
        let one_line =
            format!(r#"{{"status":"error","code":503,"detail":"{payload}","retryable":true}}"#);
        assert_eq!(one_line.lines().count(), 1, "precondition: one line");

        assert_eq!(classify(&one_line), Some(ContentKind::Json));
        let r = reduce(&one_line).expect("a 4 KB single-line API response must reduce");
        assert_eq!(r.kind, ContentKind::Json);
        // The salient short scalars survive; the bulk leaf does not.
        assert!(r.body.contains("\"status\""), "got: {}", r.body);
        assert!(r.body.contains("503"));
        assert!(!r.body.contains(&payload));
        assert!(r.body.len() < one_line.len() / 2);
    }
}
