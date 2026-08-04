//! Command / build / test log reducer: keep the head (the command echo and
//! first lines), the tail (where test/build summaries land), and every loud
//! line (errors, warnings, failures) with a few lines of trailing context so a
//! stack trace following an error survives. The thousands of "compiling…" /
//! "ok" lines in between are the noise a stale log can shed.
//!
//! Classification is deliberately conservative-but-cheap: it fires only when
//! there is clear command-output texture (several loud lines or a test/build
//! summary), so ordinary prose is never routed here. Even a rare false positive
//! is benign — the reducer keeps strictly more than the first-line placeholder
//! it replaces.

use super::{
    contains_ignore_ascii_case, is_error_signal, render_selected, ContentKind, Profile, Reduction,
    Tally,
};

const KEEP_HEAD: usize = 2;
const KEEP_TAIL: usize = 3;
/// Lines kept after a loud line — captures the stack trace under an error.
///
/// Structural rather than a size knob: it is "how much of a diagnostic body
/// hangs off its first line", which does not change because the caller has a
/// smaller budget. [`Profile::log_signal`] is the knob that scales.
const ERROR_CONTEXT: usize = 3;

/// Digit-and-status-keyword needles for [`is_summary_line`], pre-lowercased so
/// they can be matched without allocating a lowercase copy of the line.
const SUMMARY_NEEDLES: [&str; 9] = [
    "passed",
    "failed",
    "tests",
    "test result",
    "errors",
    "warnings",
    "skipped",
    "assertions",
    "finished",
];

/// A test/build summary line: a digit alongside a status keyword. These usually
/// sit at the very end, but `keep_tail` may miss them in noisy output.
///
/// Allocation-free, like [`is_error_signal`]. The obvious
/// `line.to_ascii_lowercase()` was the exact copy-per-line that
/// [`contains_ignore_ascii_case`] exists to eliminate, and this predicate is the
/// hotter of the two: it runs on every line in `looks_like_log` *and* on every
/// line in each of `reduce_log`'s two `mark_signal` passes. A 200 KB minified
/// line inside a build log therefore cost 600 KB of copying just to decide it
/// was not a summary.
fn is_summary_line(line: &str) -> bool {
    line.bytes().any(|b| b.is_ascii_digit())
        && SUMMARY_NEEDLES
            .iter()
            .any(|k| contains_ignore_ascii_case(line, k))
}

fn is_loud(line: &str) -> bool {
    is_error_signal(line) || is_summary_line(line)
}

/// Cheap detector gated on clear command/build/test signals.
pub(super) fn looks_like_log(lines: &[&str]) -> bool {
    let mut signals = 0usize;
    let mut summary = false;
    for &l in lines {
        if is_summary_line(l) {
            summary = true;
        }
        if is_loud(l) {
            signals += 1;
        }
    }
    signals >= 3 || summary
}

/// First `n` non-empty line indices.
fn head_indices(lines: &[&str], n: usize) -> Vec<usize> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .take(n)
        .map(|(i, _)| i)
        .collect()
}

/// Last `n` non-empty line indices.
fn tail_indices(lines: &[&str], n: usize) -> Vec<usize> {
    let mut idx: Vec<usize> = lines
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, l)| !l.trim().is_empty())
        .take(n)
        .map(|(i, _)| i)
        .collect();
    idx.reverse();
    idx
}

/// Leading-whitespace width of a line, in bytes (tabs count as one).
fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// A diagnostic continuation line: a single-character marker followed by
/// whitespace.
///
/// Test runners and compilers all use this convention for the body of a
/// diagnostic — pytest's `E   AssertionError` / `>   assert got == …`, rustc's
/// `|` gutter, doctest's `|`. They sit at the *same* indent as the loud line they
/// belong to, so an indentation test alone cannot tell them from a sibling
/// `test x ... ok` (which starts with a multi-character word instead).
fn is_continuation(line: &str) -> bool {
    let mut chars = line.trim_start().chars();
    matches!(
        (chars.next(), chars.next()),
        (Some(marker), Some(space)) if !marker.is_alphanumeric() || space.is_whitespace()
    ) && !line.trim_start().is_empty()
}

/// Mark loud lines encountered along `order`, plus their trailing context, until
/// `budget` *newly* marked loud lines are found. Returns how many it spent.
///
/// Context extension stops at the first follower that is neither indented deeper
/// than the loud line nor loud itself. That predicate is what keeps the budget on
/// diagnostics: a compiler error's `--> src/main.rs:10:5` continuation and a
/// panic's stack frames are indented under it and get kept, whereas the line after
/// `test suite::case_7 ... FAILED` is `test suite::case_8 ... ok` at the same
/// indent — sibling noise that used to consume two thirds of everything kept.
///
/// The two escapes matter as much as the indentation rule. pytest writes its
/// assertion block at column 0 (`E   AssertionError: …`, `E     Differing
/// items:`), so an indentation-only rule cut the diff off after its first line —
/// keeping strictly less than the unconditional `i+1..=i+ERROR_CONTEXT` it
/// replaced, which is the one outcome this change was not allowed to have. Hence
/// [`is_continuation`] for a diagnostic body, and `is_loud` for a follower that
/// would have been kept on its own merits anyway.
fn mark_signal(
    lines: &[&str],
    keep: &mut [bool],
    order: impl Iterator<Item = usize>,
    budget: usize,
) -> usize {
    let total = lines.len();
    let mut spent = 0usize;
    let mut prev_loud: Option<&str> = None;
    for i in order {
        if spent >= budget {
            break;
        }
        if !is_loud(lines[i]) {
            prev_loud = None;
            continue;
        }
        // Collapse an immediately-repeated loud line (progress-bar warnings).
        if prev_loud == Some(lines[i]) {
            continue;
        }
        prev_loud = Some(lines[i]);
        // Only a newly-marked loud line costs budget; re-visiting one the other
        // pass already kept is free, so the two passes can't double-charge.
        if !keep[i] {
            keep[i] = true;
            spent += 1;
        }
        let base_indent = indent_of(lines[i]);
        for k in 1..=ERROR_CONTEXT {
            let Some(&follower) = lines.get(i + k) else {
                break;
            };
            if i + k >= total {
                break;
            }
            if !follower.trim().is_empty()
                && indent_of(follower) <= base_indent
                && !is_loud(follower)
                && !is_continuation(follower)
            {
                break;
            }
            keep[i + k] = true;
        }
    }
    spent
}

pub(super) fn reduce_log(text: &str, profile: &Profile) -> Option<Reduction> {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    let mut keep = vec![false; total];

    for i in head_indices(&lines, KEEP_HEAD) {
        keep[i] = true;
    }
    for i in tail_indices(&lines, KEEP_TAIL) {
        keep[i] = true;
    }

    // Loud lines + trailing context, with adjacent dedup so a burst of the same
    // warning doesn't eat the whole signal budget. Run backwards from the tail
    // first — half of the budget is reserved for the failure roster and summary
    // a test runner writes last, and spending it first means a long stream of
    // loud lines earlier in the log can no longer starve it.
    //
    // A single forward pass is the wrong shape for test-runner output, which is
    // where this reducer earns its keep. In `cargo test`, every
    // `test x ... FAILED` line is loud, so the first `log_signal` failures
    // exhausted the budget in the *roster* — and the `failures:` block, the
    // `panicked at …` lines and the ``assertion `left == right` failed`` diffs
    // all sit **after** that point, so they were dropped in their entirety.
    let tail_budget = profile.log_signal / 2;
    let spent_tail = mark_signal(&lines, &mut keep, (0..total).rev(), tail_budget);
    mark_signal(
        &lines,
        &mut keep,
        0..total,
        profile.log_signal.saturating_sub(spent_tail),
    );

    let selected: Vec<usize> = (0..total).filter(|&i| keep[i]).collect();
    // Collapse a burst of identical consecutive lines kept via error-context
    // (e.g. the same warning repeated dozens of times): keep only the first of
    // each original-consecutive identical run. `prev` tracks the previous kept
    // index regardless of whether it survived, so adjacency is judged on the
    // original line numbers.
    let mut kept: Vec<usize> = Vec::with_capacity(selected.len());
    let mut prev: Option<usize> = None;
    for &i in &selected {
        let is_burst_dup = matches!(prev, Some(p) if p + 1 == i && lines[p] == lines[i]);
        if !is_burst_dup {
            kept.push(i);
        }
        prev = Some(i);
    }
    if kept.len() >= total {
        return None; // nothing dropped
    }
    // Whether this is *smaller* is not decided here. A local kept/total **line**
    // ratio used to stand in this spot, and it was both redundant and wrong in
    // the same way the module doc calls out: it rejected a log whose 8 kept lines
    // dropped two 200 KB noise lines (a 96 % byte win read as "kept 80 % of the
    // lines"), while accepting reductions that shrank no bytes at all. The
    // central `Reduction::is_meaningful_shrink` measures bytes, once.
    let body = render_selected(&lines, &kept, total, profile);
    Some(Reduction {
        kind: ContentKind::Log,
        body,
        tally: Tally::Lines {
            kept: kept.len(),
            total,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reduce(text: &str) -> Option<Reduction> {
        reduce_log(text, &Profile::DEFAULT)
    }

    #[test]
    fn detects_test_log() {
        let mut s = String::from("$ cargo test\n");
        for i in 0..40 {
            s.push_str(&format!("   Compiling crate-{i}\n"));
        }
        s.push_str("test result: ok. 312 passed; 0 failed\n");
        let lines: Vec<&str> = s.lines().collect();
        assert!(looks_like_log(&lines));
    }

    #[test]
    fn keeps_errors_and_summary_drops_noise() {
        let mut s = String::from("$ cargo build\n");
        for i in 0..50 {
            s.push_str(&format!("   Compiling dep-{i} v1.0.0\n"));
        }
        s.push_str("error[E0382]: borrow of moved value: `x`\n");
        s.push_str("  --> src/main.rs:10:5\n");
        s.push_str("   |\n");
        s.push_str("10 |     use(x);\n");
        for i in 0..30 {
            s.push_str(&format!("   more noise {i}\n"));
        }
        s.push_str("error: could not compile `app` due to previous error\n");
        let r = reduce(&s).expect("should reduce");
        assert_eq!(r.kind, ContentKind::Log);
        assert!(
            r.body.contains("error[E0382]"),
            "first error must survive; got:\n{}",
            r.body
        );
        assert!(
            r.body.contains("src/main.rs:10:5"),
            "error context (stack/location) must survive; got:\n{}",
            r.body
        );
        assert!(
            r.body.contains("could not compile"),
            "tail/summary error must survive"
        );
        assert!(r.body.contains("lines omitted"), "noise must be dropped");
        assert!(r.tally.kept() < r.tally.total());
    }

    #[test]
    fn collapses_repeated_warning_burst() {
        let mut s = String::from("$ make\n");
        for _ in 0..40 {
            s.push_str("warning: unused variable: `y`\n");
        }
        s.push_str("Build finished with 40 warnings\n");
        let r = reduce(&s).expect("should reduce");
        // Adjacent dedup means we keep far fewer than 40 warning lines.
        let warns = r.body.matches("warning: unused variable").count();
        assert!(
            warns <= 2,
            "repeated identical warnings collapsed; got {warns}"
        );
    }

    #[test]
    fn an_all_signal_log_has_nothing_to_drop() {
        // Every line is loud, so head + tail + signal marks all of them and the
        // "nothing was dropped" guard declines. (The byte guard in `reduce`
        // would decline too, but this one is what keeps a pointless header off
        // an output that is already all signal.)
        let s = "error a\nerror b\nerror c\nerror d\nerror e\nerror f\nerror g\nerror h\n";
        assert!(reduce(s).is_none());
    }

    /// The local kept/total **line** ratio that used to guard this reducer
    /// rejected exactly the case where reduction pays most: a log that is short
    /// in lines and enormous in bytes. Six lines, five of them kept — an 83 %
    /// line ratio, so the old guard returned `None` and all 180 KB reached the
    /// model verbatim — while the per-line clamp and the one dropped line take
    /// the body to about a kilobyte. Whether a reduction is worth emitting is a
    /// byte question, and it is answered once, centrally.
    #[test]
    fn a_log_that_is_short_in_lines_and_huge_in_bytes_still_reduces() {
        // Eight lines — the log reducer's own minimum — of which head+tail+the
        // one loud line keep six. A 75 % line ratio, and 120 KB of the 180 KB
        // dropped outright with the rest clamped per line.
        let huge = "x".repeat(60_000);
        let s = format!(
            "$ build\nnoise a {huge}\nnoise b {huge}\nerror: boom\nnoise c {huge}\n\
             linking\nstripping\nBuild finished with 1 errors\n"
        );
        let r = reduce(&s).expect("a 180 KB log must reduce");
        assert!(
            r.tally.kept() * 10 >= r.tally.total() * 7,
            "precondition: the line ratio is high ({}/{}), which is what the old \
             guard rejected on",
            r.tally.kept(),
            r.tally.total()
        );
        assert!(
            r.body.len() < s.len() / 100,
            "byte-wise it is a >99 % reduction, got {} of {}",
            r.body.len(),
            s.len()
        );
        assert!(r.body.contains("error: boom"), "the signal survives");
        assert!(
            super::super::reduce(&s).is_some(),
            "and the central byte guard must accept it"
        );
    }

    /// pytest writes its assertion block at column 0, so an indentation-only
    /// context rule truncated the diff after the first line. The failing test's
    /// name, the assertion and its diff all have to survive.
    #[test]
    fn pytest_assertion_block_survives() {
        let mut s = String::from("..F...                              [100%]\n");
        for i in 0..300 {
            s.push_str(&format!("collected fixture chatter {i}\n"));
        }
        s.push_str(
            "=================================== FAILURES ===================================\n",
        );
        s.push_str("_______________________ test_totals ________________________\n");
        s.push_str("\n");
        s.push_str("    def test_totals():\n");
        s.push_str("        got = compute({'a': 1})\n");
        s.push_str(">       assert got == {'a': 2}\n");
        s.push_str("E       AssertionError: assert {'a': 1} == {'a': 2}\n");
        s.push_str("E         Differing items:\n");
        s.push_str("E         {'a': 1} != {'a': 2}\n");
        s.push_str("\n");
        s.push_str("test_totals.py:7: AssertionError\n");
        for i in 0..300 {
            s.push_str(&format!("teardown chatter {i}\n"));
        }
        s.push_str(
            "=========================== short test summary info ============================\n",
        );
        s.push_str("1 failed, 5 passed in 0.03s\n");

        let r = reduce(&s).expect("a pytest run must reduce");
        for needle in [
            "AssertionError: assert {'a': 1} == {'a': 2}",
            "Differing items",
            "!= {'a': 2}",
            "1 failed, 5 passed",
        ] {
            assert!(
                r.body.contains(needle),
                "the assertion block must survive; missing {needle:?} in:\n{}",
                r.body
            );
        }
        assert!(
            !r.body.contains("teardown chatter 150"),
            "the chatter must still be dropped"
        );
    }

    /// The other half of the same predicate: sibling `... ok` lines at the same
    /// indent must NOT be pulled in as context, which is what wasted two thirds of
    /// the budget before.
    #[test]
    fn sibling_ok_lines_are_not_kept_as_error_context() {
        let mut s = String::from("running 400 tests\n");
        for i in 0..400 {
            if i == 10 {
                s.push_str("test suite::boom ... FAILED\n");
            } else {
                s.push_str(&format!("test suite::case_{i} ... ok\n"));
            }
        }
        s.push_str("test result: FAILED. 399 passed; 1 failed\n");
        let r = reduce(&s).expect("must reduce");
        assert!(r.body.contains("suite::boom"), "the failure must survive");
        // The `... ok` lines that legitimately appear are the head/tail anchors;
        // what must NOT appear is the failure's immediate followers being dragged
        // in as "error context".
        assert!(
            !r.body.contains("case_11 ... ok"),
            "the sibling line after the failure must not be kept as context:\n{}",
            r.body
        );
        let oks = r.body.matches("... ok").count();
        assert!(
            oks <= KEEP_HEAD + KEEP_TAIL,
            "only head/tail anchors may be passing-test lines, got {oks} in:\n{}",
            r.body
        );
    }

    /// A tight budget must reach the reducer as fewer kept signal lines, not as a
    /// blind cut of a default-sized body afterwards.
    #[test]
    fn a_tight_budget_keeps_fewer_signal_lines() {
        let mut s = String::from("$ cargo test\n");
        for i in 0..60 {
            s.push_str(&format!("error[E{i:04}]: distinct failure {i}\n"));
            for q in 0..6 {
                s.push_str(&format!("   Compiling quiet-{i}-{q} v1.0.0\n"));
            }
        }
        s.push_str("test result: FAILED. 0 passed; 60 failed\n");
        let wide = reduce(&s).expect("default profile must reduce");
        let tight = reduce_log(&s, &Profile::for_token_budget(400)).expect("tight must reduce");
        assert!(
            tight.tally.kept() < wide.tally.kept(),
            "tight kept {} vs default {}",
            tight.tally.kept(),
            wide.tally.kept()
        );
        assert!(tight.body.len() < wide.body.len());
    }

    #[test]
    fn summary_detection_is_allocation_free_and_case_insensitive() {
        assert!(is_summary_line("test result: FAILED. 3 passed; 1 failed"));
        assert!(is_summary_line("Finished in 12 seconds"));
        assert!(is_summary_line("1 FAILED, 5 PASSED"));
        // A keyword with no digit is not a summary…
        assert!(!is_summary_line("running the tests now"));
        // …and a digit with no keyword is not either.
        assert!(!is_summary_line("   Compiling dep-42 v1.0.0"));
    }
}
