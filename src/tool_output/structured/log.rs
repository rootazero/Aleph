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

use super::{is_error_signal, render_selected, ContentKind, Reduction};

const KEEP_HEAD: usize = 2;
const KEEP_TAIL: usize = 3;
/// Max loud lines kept (prevents a flood of identical warnings dominating).
const MAX_SIGNAL: usize = 24;
/// How much of [`MAX_SIGNAL`] is reserved for a backwards scan from the tail.
///
/// A single forward pass is the wrong shape for test-runner output, which is
/// where this reducer earns its keep. In `cargo test`, every `test x ... FAILED`
/// line is loud, so the first `MAX_SIGNAL` failures exhausted the budget in the
/// *roster* — and the `failures:` block, the `panicked at …` lines and the
/// `assertion \`left == right\` failed` diffs all sit **after** that point, so
/// they were dropped in their entirety. Every runner (cargo, pytest, jest, go
/// test) puts its diagnostics and its summary at the end, so half the budget is
/// spent scanning backwards from there.
const SIGNAL_FROM_TAIL: usize = MAX_SIGNAL / 2;
/// Lines kept after a loud line — captures the stack trace under an error.
const ERROR_CONTEXT: usize = 3;
/// Below this kept/total ratio (×10) the reduction isn't worth the header.
const MAX_KEPT_RATIO_X10: usize = 7; // keep only if kept < 70% of total

/// A test/build summary line: a digit alongside a status keyword. These usually
/// sit at the very end, but `keep_tail` may miss them in noisy output.
fn is_summary_line(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    let has_digit = l.bytes().any(|b| b.is_ascii_digit());
    has_digit
        && [
            "passed",
            "failed",
            "tests",
            "test result",
            "errors",
            "warnings",
            "skipped",
            "assertions",
            "finished",
        ]
        .iter()
        .any(|k| l.contains(k))
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

/// Mark loud lines encountered along `order`, plus their trailing context, until
/// `budget` *newly* marked loud lines are found. Returns how many it spent.
///
/// Context extension stops at the first follower that isn't indented deeper than
/// the loud line. That one predicate is what keeps the budget on diagnostics: a
/// compiler error's `--> src/main.rs:10:5` continuation and a panic's stack
/// frames are indented under it and get kept, whereas the line after
/// `test suite::case_7 ... FAILED` is `test suite::case_8 ... ok` at the same
/// indent — sibling noise that used to consume two thirds of everything kept.
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
            if !follower.trim().is_empty() && indent_of(follower) <= base_indent {
                break;
            }
            keep[i + k] = true;
        }
    }
    spent
}

pub(super) fn reduce_log(text: &str) -> Option<Reduction> {
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
    // first — that half of the budget is reserved for the failure roster and
    // summary a test runner writes last, and spending it first means a long
    // stream of loud lines earlier in the log can no longer starve it.
    let tail_budget = SIGNAL_FROM_TAIL.min(MAX_SIGNAL);
    let spent_tail = mark_signal(&lines, &mut keep, (0..total).rev(), tail_budget);
    mark_signal(
        &lines,
        &mut keep,
        0..total,
        MAX_SIGNAL.saturating_sub(spent_tail),
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
        return None;
    }
    // Worth the header only if it meaningfully shrinks the log.
    if kept.len() * 10 >= total * MAX_KEPT_RATIO_X10 {
        return None;
    }
    let body = render_selected(&lines, &kept, total);
    Some(Reduction {
        kind: ContentKind::Log,
        body,
        kept_lines: kept.len(),
        total_lines: total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let r = reduce_log(&s).expect("should reduce");
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
        assert!(r.kept_lines < r.total_lines);
    }

    #[test]
    fn collapses_repeated_warning_burst() {
        let mut s = String::from("$ make\n");
        for _ in 0..40 {
            s.push_str("warning: unused variable: `y`\n");
        }
        s.push_str("Build finished with 40 warnings\n");
        let r = reduce_log(&s).expect("should reduce");
        // Adjacent dedup means we keep far fewer than 40 warning lines.
        let warns = r.body.matches("warning: unused variable").count();
        assert!(
            warns <= 2,
            "repeated identical warnings collapsed; got {warns}"
        );
    }

    #[test]
    fn quiet_clean_log_not_worth_reducing() {
        // Mostly-signal short output shouldn't trip the ratio guard into a
        // pointless header. Here nearly every line is loud.
        let s = "error a\nerror b\nerror c\nerror d\nerror e\nerror f\nerror g\nerror h\n";
        // All 8 lines are loud → kept >= 70% → no reduction.
        assert!(reduce_log(s).is_none());
    }
}
