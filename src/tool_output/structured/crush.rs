//! Lossy line-importance crusher for the two high-variance content kinds
//! (spec §2b, the "headroom SmartCrusher" ledger item): when the kind-specific
//! reducer's artifact still exceeds the caller's byte allowance, score every
//! line by importance and keep the highest-scoring ones that fit, folding the
//! rest behind omission markers.

use crate::context::budget::pressure::chars_for_result_token_budget;

use super::{
    contains_ignore_ascii_case, is_error_signal, render_selected, ContentKind, Profile, Reduction,
    Tally, MIN_INPUT_BYTES,
};

/// Bytes reserved for the `[compacted kind: kept X/Y lines]` header the caller
/// renders on top of the crushed body, so the body's budget is `allowance -
/// HEADER_RESERVE` and the fully rendered artifact is guaranteed to fit.
const HEADER_RESERVE: usize = 64;

/// Summary hints promoted above noise in a crushed log: a bounded count of
/// passes/skips means the line is load-bearing orientation, not chatter.
const SUMMARY_HINTS: [&str; 6] = [
    "passed",
    "test result",
    "finished",
    "completed",
    "assertions",
    "skipped",
];

/// Crush `text` to fit `budget_tokens`, or decline.
///
/// YAGNI gate (spec §2b): only Log and Search get a crusher. Diff and Json are
/// structure-sensitive — deleting "unimportant" lines from either produces
/// something that still parses but describes a different change or document,
/// which is worse than the caller's head/tail truncation.
///
/// Loss-before-loss gate: the existing kind reducer runs first, sized by
/// [`Profile::for_token_budget`]; when its artifact already fits the allowance
/// this returns `None`, so the lossy path engages only when no lossless one is
/// left. A reducer that declined entirely (all-signal input) also declines
/// here: crushing pure signal ranks errors against errors with no principled
/// winner, and the caller's head/tail fallback at least keeps boundary
/// structure.
///
/// Whether the crushed result is worth emitting is not decided here: whatever
/// the selection produces goes through the module's central
/// [`Reduction::is_meaningful_shrink`] byte guard, and a selection that cannot
/// fit the budget at all declines honestly instead of emitting over-budget
/// bytes.
pub(crate) fn crush_within(text: &str, kind: ContentKind, budget_tokens: usize) -> Option<String> {
    if !matches!(kind, ContentKind::Log | ContentKind::Search) {
        return None;
    }
    if text.len() < MIN_INPUT_BYTES {
        return None;
    }
    let profile = Profile::for_token_budget(budget_tokens);
    let allowance = chars_for_result_token_budget(budget_tokens);
    let lines: Vec<&str> = text.lines().collect();

    // Loss-before-loss: the existing reducer's artifact must demonstrably
    // exceed the allowance before crushing is worth its information loss.
    // Declined reducers (kept == total) fall through with it.
    let reduced = match kind {
        ContentKind::Log => super::log::reduce_log(&lines, &profile),
        ContentKind::Search => super::search::reduce_search(&lines, &profile),
        ContentKind::Diff | ContentKind::Json => return None,
    };
    match reduced {
        Some(reduction) if reduction.render().len() <= allowance => return None,
        None => return None,
        _ => {}
    }

    let body = crush_body(
        &lines,
        kind,
        &profile,
        allowance.saturating_sub(HEADER_RESERVE),
    )?;
    let total = lines.len();
    let kept = kept_line_count(&body);
    let reduction = Reduction {
        kind,
        body,
        tally: Tally::Lines { kept, total },
    };
    if reduction.is_meaningful_shrink(text) {
        Some(reduction.body)
    } else {
        None
    }
}

/// Kept-line tally for a crushed body: every omission marker the renderer
/// synthesizes starts with `… (`; original lines are kept verbatim and never
/// do. Used by the [`super::reduce_within`] router to rebuild an honest
/// [`Tally`] for the crushed artifact.
pub(super) fn kept_line_count(body: &str) -> usize {
    body.lines().filter(|l| !l.starts_with("… (")).count()
}

/// Select the highest-importance lines that fit `budget`, in original order.
/// The greedy pass fills by estimated per-line cost (clamped line width plus
/// one for the newline); the exact pass then renders with [`render_selected`]
/// — whose omission markers the estimate cannot predict — and drops the
/// lowest-priority picked line until the true render fits. `picked` is a
/// score-descending stack, so trimming is O(dropped) pops, not rescans.
fn crush_body(
    lines: &[&str],
    kind: ContentKind,
    profile: &Profile,
    budget: usize,
) -> Option<String> {
    let total = lines.len();
    let anchors = anchor_indices(lines);
    let scores: Vec<u8> = lines
        .iter()
        .enumerate()
        .map(|(i, line)| score_line(kind, line, i, lines, &anchors))
        .collect();
    let mut picked: Vec<usize> = (0..total).filter(|&i| scores[i] > 0).collect();
    picked.sort_by(|&a, &b| scores[b].cmp(&scores[a]).then_with(|| a.cmp(&b)));

    let mut kept: Vec<usize> = Vec::new();
    let mut spent = 0usize;
    for &i in &picked {
        let cost = line_cost(lines[i], profile.line_chars);
        if spent + cost > budget {
            continue;
        }
        kept.push(i);
        spent += cost;
    }

    loop {
        if kept.is_empty() {
            return None;
        }
        kept.sort_unstable();
        let body = render_selected(lines, &kept, total, profile);
        if body.len() <= budget {
            return Some(body);
        }
        // The estimated costs undercount markers and clamping; drop the
        // lowest-priority picked line until the exact render fits. Picks that
        // the greedy pass already skipped cost nothing to pop.
        let drop = picked.pop()?;
        if let Some(pos) = kept.iter().position(|&x| x == drop) {
            kept.swap_remove(pos);
        }
    }
}

/// Importance score for one line, higher being harder to drop:
///
/// - **4 — error signal**: reuses the log reducer's own [`is_error_signal`]
///   needles (error/panic/failed/fatal/exception/traceback/aborted), so
///   "important" can never drift between the lossless and the lossy paths.
/// - **3 — kind signal**: a summary line with a bounded count (log), or a
///   rigid `path:line:content` match shape (search).
/// - **2 — continuation**: indented lines hanging off an error block
///   (rust-style `  --> file.rs:10:5` locations, stack frames).
/// - **1 — anchor / filler**: head/tail orientation lines (log), any non-empty
///   non-match line (search).
/// - **0 — noise / burst copy**: everything else, plus any line identical to
///   its immediate predecessor — however loud the content, the copy carries
///   nothing the first occurrence does not, so bursts collapse to one
///   representative.
fn score_line(kind: ContentKind, line: &str, idx: usize, lines: &[&str], anchors: &[usize]) -> u8 {
    if idx > 0 && lines[idx - 1] == line {
        return 0;
    }
    if is_error_signal(line) {
        return 4;
    }
    match kind {
        ContentKind::Log => {
            if crush_is_summary(line) {
                3
            } else if is_continuation(line) {
                2
            } else if anchors.contains(&idx) {
                1
            } else {
                0
            }
        }
        ContentKind::Search => {
            if crush_is_match_line(line) {
                3
            } else if !line.trim().is_empty() {
                1
            } else {
                0
            }
        }
        ContentKind::Diff | ContentKind::Json => 0,
    }
}

/// First-two / last-three non-empty line indices — the log reducer's head/tail
/// anchors — computed once, since per-line anchor scoring would otherwise be
/// O(n²) over whole-tool-output inputs.
fn anchor_indices(lines: &[&str]) -> Vec<usize> {
    let mut anchors: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .take(2)
        .map(|(i, _)| i)
        .collect();
    anchors.extend(
        lines
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, line)| !line.trim().is_empty())
            .take(3)
            .map(|(i, _)| i),
    );
    anchors
}

/// Summary line carrying a bounded count of work: `passed`, `finished`,
/// `completed`, … plus a digit, so `Compiling dep-1 v1.0.0` — every build
/// log's noise — is not promoted by the digit alone.
fn crush_is_summary(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    SUMMARY_HINTS
        .iter()
        .any(|hint| contains_ignore_ascii_case(&lower, hint))
        && line.chars().any(|c| c.is_ascii_digit())
}

/// Indented continuation of the block above it — mirrors the log reducer's
/// [`super::log::is_continuation`] so a line that survived losslessly keeps
/// the same rank when crushing.
fn is_continuation(line: &str) -> bool {
    let mut chars = line.trim_start().chars();
    matches!(
        (chars.next(), chars.next()),
        (Some(marker), Some(space)) if !marker.is_alphanumeric() || space.is_whitespace()
    )
}

/// Rigid search-result shape `path:line:` / `path-line:` / `path(line,)` —
/// the same shape the search reducer's [`super::search::is_match_line`]
/// recognizes. Deliberately stricter about the separator than the reducer:
/// the reducers ask "is this a result at all" (for classification), the
/// crusher asks "is this line worth bytes under pressure".
fn crush_is_match_line(line: &str) -> bool {
    let bytes = line.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b':' if i >= 2 => {
                let j = i + 1;
                let mut k = j;
                while k < bytes.len() && bytes[k].is_ascii_digit() {
                    k += 1;
                }
                if k > j && (k == bytes.len() || matches!(bytes[k], b':' | b' ' | b'\t')) {
                    return true;
                }
            }
            b'-' | b'(' if i >= 2 && bytes.get(i + 1).is_some_and(u8::is_ascii_digit) => {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// Estimated per-line cost: the clamped render width plus one for the newline.
fn line_cost(line: &str, max_chars: usize) -> usize {
    line.chars()
        .take(max_chars)
        .map(|c| c.len_utf8())
        .sum::<usize>()
        + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A realistic `cargo test` failure log. The banner/head-tail lines are
    /// long (real tool output is) — that is what pushes the *lossless*
    /// reducer's artifact over a tight allowance, which is the only condition
    /// under which crushing may engage; the signal lines stay short so they
    /// all fit the crush budget.
    fn failing_test_log(failures: usize, ok: usize) -> String {
        let mut s = String::from(
            "$ cargo test --workspace --all-targets --all-features --release --no-fail-fast\n",
        );
        s.push_str("   Compiling alephcore v0.1.0 (D:\\Workspace\\Aleph\\context-fabric-line2)\n");
        s.push_str("    Finished test profile [unoptimized + debuginfo] in 12.34s\n");
        s.push_str("     Running unittests src/lib.rs (target\\debug\\deps\\alephcore-0123456789abcdef.exe)\n");
        for i in 0..60 {
            s.push_str(&format!("   Compiling dep-{i} v1.0.0 (lib)\n"));
        }
        for i in 0..ok {
            s.push_str(&format!("test case_{i} ... ok\n"));
        }
        for k in 0..failures {
            s.push_str(&format!("test suite::fail_{k} ... FAILED\n"));
        }
        s.push_str("failures:\n");
        for k in 0..failures {
            s.push_str(&format!("    suite::fail_{k}\n"));
        }
        s.push_str("thread 'suite::fail_0' panicked at src/lib.rs:42:5:\n");
        s.push_str("assertion `left == right` failed\n");
        s.push_str("  left: 1\n");
        s.push_str(" right: 2\n");
        for i in 0..50 {
            s.push_str(&format!(
                "teardown chatter {i} about resource cleanup and async runtime shutdown in worker pool iteration {i}\n"
            ));
        }
        s.push_str("test result: FAILED. {failures} failed; {ok} passed; 0 ignored\n");
        s
    }

    #[test]
    fn crush_keeps_error_lines_over_noise() {
        let s = failing_test_log(8, 400);
        const BUDGET: usize = 240; // allowance = 600 chars
        let body = crush_within(&s, ContentKind::Log, BUDGET).expect("must crush");
        let allowance = chars_for_result_token_budget(BUDGET);
        assert!(
            body.len() <= allowance,
            "crushed output must fit the caller's allowance: {} > {allowance}",
            body.len()
        );
        for k in 0..8 {
            assert!(
                body.contains(&format!("suite::fail_{k}")),
                "failure {k} must survive within budget:\n{body}"
            );
        }
        assert!(
            body.contains("panicked at src/lib.rs:42"),
            "the panic location must survive:\n{body}"
        );
        assert!(
            body.contains("test result: FAILED"),
            "the summary must survive"
        );
        assert!(
            !body.contains("Compiling dep-"),
            "build noise must be dropped"
        );
        assert!(body.contains("lines omitted"), "omissions must be marked");
    }

    #[test]
    fn crush_keeps_match_lines() {
        let mut s = String::new();
        for f in 0..40 {
            for l in 0..8 {
                s.push_str(&format!(
                    "src/f{f}.rs:{}:    let ret = registry.compute_value_for_cache_key_and_validate_input_bounds({l})?;\n",
                    l + 1
                ));
            }
        }
        s.push_str("src/net.rs:88:    ERROR connection refused, retrying\n");
        s.push_str("rg: 321 matches across 41 files\n");
        const BUDGET: usize = 200; // allowance = 500 chars
                                   // The crush gate is "the lossless reducer's artifact must exceed the
                                   // allowance" — surface both numbers so a gate miss reads as data, not
                                   // as a mystery `None`.
        let profile = Profile::for_token_budget(BUDGET);
        let lines: Vec<&str> = s.lines().collect();
        let reduced = super::super::search::reduce_search(&lines, &profile)
            .expect("the lossless search reducer must produce an artifact here");
        let allowance = chars_for_result_token_budget(BUDGET);
        assert!(
            reduced.render().len() > allowance,
            "fixture must need crushing: reducer artifact {} <= {} allowance",
            reduced.render().len(),
            allowance
        );
        let body = crush_within(&s, ContentKind::Search, BUDGET).expect("must crush");
        assert!(body.len() <= allowance, "{} > {allowance}", body.len());
        assert!(
            body.contains("ERROR connection refused"),
            "the error-bearing match must survive:\n{body}"
        );
        assert!(
            !body.contains("321 matches across 41 files"),
            "the non-match summary line is the first to go:\n{body}"
        );
        let match_lines = body.lines().filter(|l| crush_is_match_line(l)).count();
        assert!(
            match_lines >= 4,
            "a spread of real match lines survives alongside the error match, got {match_lines}"
        );
    }

    #[test]
    fn crush_diff_returns_none() {
        let mut d = String::from("diff --git a/x.rs b/x.rs\n");
        for i in 0..60 {
            d.push_str(&format!(
                "@@ -{i},3 +{i},3 @@\n-old line {i}\n+new line {i}\n context\n"
            ));
        }
        assert!(
            crush_within(&d, ContentKind::Diff, 200).is_none(),
            "diff is structure-sensitive; the crusher declines (spec YAGNI gate)"
        );
        assert!(
            crush_within(&d, ContentKind::Json, 200).is_none(),
            "json is structure-sensitive; the crusher declines (spec YAGNI gate)"
        );
    }

    #[test]
    fn crush_respects_byte_guard() {
        // The crusher never decides for itself whether the result is smaller:
        // the selection it produces must pass the module's central byte guard.
        let s = failing_test_log(12, 200);
        const BUDGET: usize = 200;
        let body = crush_within(&s, ContentKind::Log, BUDGET).expect("must crush");
        let reduction = Reduction {
            kind: ContentKind::Log,
            body,
            tally: Tally::Lines { kept: 1, total: 1 },
        };
        assert!(
            reduction.is_meaningful_shrink(&s),
            "crush output must pass the central byte guard"
        );

        // An allowance too small for even one clamped line plus its marker:
        // decline honestly rather than emit over-budget bytes.
        assert!(
            crush_within(&s, ContentKind::Log, 20).is_none(),
            "a selection that cannot fit the budget declines"
        );
    }

    #[test]
    fn crush_declines_when_artifact_fits() {
        // 能低损就不有损: at a generous budget the existing reducer's artifact
        // already fits the allowance — the crusher must decline and let it
        // stand rather than crush by reflex.
        let s = failing_test_log(8, 400);
        assert!(crush_within(&s, ContentKind::Log, 8_000).is_none());
    }

    #[test]
    fn crush_collapses_repeated_burst() {
        // One contiguous burst of identical warnings between distinct errors:
        // however loud each copy is on its own, a line identical to its
        // predecessor carries nothing new, so the burst contributes zero
        // kept lines.
        let mut s = String::from("$ cargo build --workspace --all-targets --release\n");
        for i in 0..30 {
            s.push_str(&format!(
                "error[E{i:03}]: distinct failure number {i} while lowering module in codegen backend emission\n"
            ));
        }
        for _ in 0..120 {
            s.push_str("warning: unused variable: `y`\n");
        }
        s.push_str("error: could not compile `app` due to previous errors\n");
        const BUDGET: usize = 190; // allowance = 475 chars
        let body = crush_within(&s, ContentKind::Log, BUDGET).expect("must crush");
        let warns = body.matches("warning: unused variable").count();
        assert!(
            warns <= 1,
            "the burst collapses to one representative, got {warns}:\n{body}"
        );
        assert!(body.contains("error[E000]"), "the first error survives");
    }

    #[test]
    fn an_all_signal_log_is_not_crushed() {
        // Every line is an error line: the structured reducer declines
        // (nothing to drop) and crushing pure signal has no principled winner.
        let mut s = String::new();
        for i in 0..40 {
            s.push_str(&format!("error[E{i:03}]: distinct failure number {i}\n"));
        }
        assert!(crush_within(&s, ContentKind::Log, 200).is_none());
    }
}
