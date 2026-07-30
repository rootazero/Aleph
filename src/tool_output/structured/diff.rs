//! Unified-diff reducer: keep the `+`/`-` change lines and hunk/file headers,
//! trim surrounding context to a couple of lines. The changes *are* the signal;
//! the unchanged context is the noise a stale diff can shed.

use super::{render_selected, ContentKind, Reduction};

/// Context lines kept on each side of a run of changes.
///
/// Must stay **strictly below** git's default `-U3`, or the pass is a no-op by
/// construction: with a 3-line context every unchanged line is within 2 of an
/// anchor, so the old value of 2 kept 3011 of 3024 lines on a real 26-file diff
/// and the only thing that shrank the output was the blind head truncate below.
const MAX_CONTEXT: usize = 1;
/// Hard cap on kept lines so a giant all-context diff still shrinks; the
/// surrounding stage's token guard provides the final safety net.
const MAX_KEPT: usize = 240;
/// Floor on lines allotted to each file when the cap forces a per-file split, so
/// every file keeps its header plus a little detail rather than vanishing.
const MIN_PER_FILE: usize = 4;

/// Cheap detector: a unified diff has unmistakable structural markers.
pub(super) fn looks_like_diff(lines: &[&str]) -> bool {
    let mut changes = 0usize;
    for &l in lines {
        if l.starts_with("diff --git ") {
            return true;
        }
        if l.starts_with("@@ ") && l.matches("@@").count() >= 2 {
            return true;
        }
        if is_change(l) {
            changes += 1;
        }
    }
    // A header-less fragment (e.g. a `diff -u` paste without `diff --git`) still
    // qualifies if a solid fraction of lines are `+`/`-` changes.
    changes >= 4 && changes * 2 >= lines.len()
}

/// Structural diff metadata lines (file headers, hunk headers, mode/rename).
fn is_header(l: &str) -> bool {
    l.starts_with("diff ")
        || l.starts_with("index ")
        || l.starts_with("+++ ")
        || l.starts_with("--- ")
        || l.starts_with("@@ ")
        || l.starts_with("new file")
        || l.starts_with("deleted file")
        || l.starts_with("rename ")
        || l.starts_with("copy ")
        || l.starts_with("similarity ")
        || l.starts_with("old mode")
        || l.starts_with("new mode")
        || l.starts_with("Binary files")
}

/// An actual added/removed line. `+++ `/`--- ` are file headers (handled by
/// [`is_header`]) and must not count as changes.
fn is_change(l: &str) -> bool {
    (l.starts_with('+') && !l.starts_with("+++ ")) || (l.starts_with('-') && !l.starts_with("--- "))
}

/// True for lines that anchor kept context (headers and changes).
fn is_anchor(l: &str) -> bool {
    is_header(l) || is_change(l)
}

pub(super) fn reduce_diff(text: &str) -> Option<Reduction> {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    let mut keep = vec![false; total];

    // Anchors (headers + change lines) are always kept.
    for (i, &l) in lines.iter().enumerate() {
        if is_anchor(l) {
            keep[i] = true;
        }
    }
    // Keep a little context around anchors so each hunk stays readable. Context
    // never chains off context — the window is tested against the original
    // anchor predicate, not the (mutating) keep array.
    for (i, keep_i) in keep.iter_mut().enumerate() {
        if *keep_i {
            continue;
        }
        let lo = i.saturating_sub(MAX_CONTEXT);
        let hi = (i + MAX_CONTEXT).min(total - 1);
        if (lo..=hi).any(|j| is_anchor(lines[j])) {
            *keep_i = true;
        }
    }

    let mut kept: Vec<usize> = (0..total).filter(|&i| keep[i]).collect();
    if kept.len() >= total {
        return None; // all signal — nothing to drop
    }
    let files_total = count_file_starts(&lines, 0..total);
    if kept.len() > MAX_KEPT {
        kept = trim_per_file(&lines, &kept);
    }
    let mut body = render_selected(&lines, &kept, total);
    let files_shown = count_file_starts(&lines, kept.iter().copied());
    if files_total > files_shown {
        body.push_str(&format!(
            "\n… ({} more files changed, not shown) …",
            files_total - files_shown
        ));
    }
    Some(Reduction {
        kind: ContentKind::Diff,
        body,
        kept_lines: kept.len(),
        total_lines: total,
    })
}

/// Whether line `i` begins a new file's section.
///
/// `git diff` emits both `diff --git …` and `--- a/…` per file, so counting both
/// would double every file. When any `diff ` header is present it is the sole
/// marker; a header-less `diff -u` paste falls back to `--- `.
fn starts_file(lines: &[&str], i: usize, git_style: bool) -> bool {
    if git_style {
        lines[i].starts_with("diff ")
    } else {
        lines[i].starts_with("--- ")
    }
}

fn is_git_style(lines: &[&str]) -> bool {
    lines.iter().any(|l| l.starts_with("diff "))
}

fn count_file_starts(lines: &[&str], idxs: impl Iterator<Item = usize>) -> usize {
    let git_style = is_git_style(lines);
    idxs.filter(|&i| starts_file(lines, i, git_style)).count()
}

/// Spread [`MAX_KEPT`] across the files the diff touches instead of taking the
/// first `MAX_KEPT` kept lines.
///
/// The head truncate this replaces was the reason a 26-file diff reached the
/// model as **3 files** under a header (`kept 240/3024 lines`) that implied
/// uniform line-level thinning — so a model asked to review the change would
/// confidently report that it touched three files. Each file now keeps its
/// section header plus a share of its own detail, and change lines are preferred
/// over context lines inside that share.
fn trim_per_file(lines: &[&str], kept: &[usize]) -> Vec<usize> {
    let git_style = is_git_style(lines);
    // Split `kept` into per-file runs. Indices before the first file header
    // (a header-less fragment) form an initial run of their own.
    let mut runs: Vec<Vec<usize>> = Vec::new();
    for &i in kept {
        if runs.is_empty() || starts_file(lines, i, git_style) {
            runs.push(Vec::new());
        }
        runs.last_mut()
            .expect("invariant: a run was just pushed")
            .push(i);
    }
    // `MIN_PER_FILE` is a floor, so on a very wide diff a per-file share would
    // push the total back past `MAX_KEPT` (500 files × 4 = 2000 lines, each
    // separated by its own omission marker). Bound the number of files that get
    // detail; the rest are reported by the trailing "N more files changed" note,
    // which is the honest answer at that width anyway.
    let max_files = (MAX_KEPT / MIN_PER_FILE).max(1);
    let runs = &runs[..runs.len().min(max_files)];
    let quota = (MAX_KEPT / runs.len().max(1)).max(MIN_PER_FILE);

    let mut out: Vec<usize> = Vec::new();
    for run in runs {
        if run.len() <= quota {
            out.extend(run.iter().copied());
            continue;
        }
        // Three priority tiers over this file's own lines: structural headers
        // (they name the file and its hunks), then changes, then context.
        let mut picked = vec![false; run.len()];
        let mut n = 0usize;
        for tier in 0..3u8 {
            for (pos, &i) in run.iter().enumerate() {
                if n >= quota {
                    break;
                }
                if picked[pos] {
                    continue;
                }
                let wanted = match tier {
                    0 => is_header(lines[i]),
                    1 => is_change(lines[i]),
                    _ => true,
                };
                if wanted {
                    picked[pos] = true;
                    n += 1;
                }
            }
        }
        out.extend(
            run.iter()
                .enumerate()
                .filter(|(pos, _)| picked[*pos])
                .map(|(_, &i)| i),
        );
    }
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_diff() -> String {
        let mut s = String::from("diff --git a/src/x.rs b/src/x.rs\n");
        s.push_str("index abc1234..def5678 100644\n");
        s.push_str("--- a/src/x.rs\n");
        s.push_str("+++ b/src/x.rs\n");
        s.push_str("@@ -10,6 +10,6 @@ fn foo() {\n");
        // a big block of unchanged context
        for i in 0..30 {
            s.push_str(&format!(" unchanged context line {i}\n"));
        }
        s.push_str("-    let old = 1;\n");
        s.push_str("+    let new = 2;\n");
        for i in 0..30 {
            s.push_str(&format!(" trailing context line {i}\n"));
        }
        s
    }

    #[test]
    fn detects_git_diff() {
        let d = sample_diff();
        let lines: Vec<&str> = d.lines().collect();
        assert!(looks_like_diff(&lines));
    }

    #[test]
    fn detects_headerless_fragment() {
        let frag = "-removed one\n-removed two\n+added one\n+added two\n+added three\n context\n";
        let lines: Vec<&str> = frag.lines().collect();
        assert!(looks_like_diff(&lines));
    }

    #[test]
    fn prose_is_not_a_diff() {
        let prose = "- a bullet point\nsome prose here\nmore prose\n";
        let lines: Vec<&str> = prose.lines().collect();
        assert!(!looks_like_diff(&lines), "a stray bullet is not a diff");
    }

    #[test]
    fn keeps_changes_drops_context() {
        let d = sample_diff();
        let r = reduce_diff(&d).expect("diff should reduce");
        assert_eq!(r.kind, ContentKind::Diff);
        // The change lines survive.
        assert!(r.body.contains("-    let old = 1;"));
        assert!(r.body.contains("+    let new = 2;"));
        // The bulk of context is dropped.
        assert!(r.kept_lines < r.total_lines);
        assert!(
            r.body.contains("lines omitted"),
            "expected an omission marker; got:\n{}",
            r.body
        );
        // Headers are preserved.
        assert!(r.body.contains("@@ -10,6 +10,6 @@"));
    }

    #[test]
    fn all_changes_no_reduction() {
        // A diff that is entirely change lines has nothing to drop.
        let mut s = String::from("@@ -1,4 +1,4 @@\n");
        for i in 0..10 {
            s.push_str(&format!("-old {i}\n+new {i}\n"));
        }
        assert!(
            reduce_diff(&s).is_none(),
            "an all-change diff should not reduce"
        );
    }

    /// A wide diff must stay bounded and must say how many files it dropped —
    /// the old head truncate silently amputated 23 of 26 files under a header
    /// that implied uniform line-level thinning.
    #[test]
    fn a_wide_diff_stays_bounded_and_reports_dropped_files() {
        let mut d = String::new();
        for f in 0..400 {
            d.push_str(&format!("diff --git a/f{f}.rs b/f{f}.rs\n"));
            d.push_str("index 1111111..2222222 100644\n");
            d.push_str(&format!("--- a/f{f}.rs\n+++ b/f{f}.rs\n"));
            d.push_str("@@ -1,6 +1,6 @@\n");
            for c in 0..6 {
                d.push_str(&format!(" context {c}\n"));
            }
            d.push_str("-let old = 1;\n+let new = 2;\n");
        }
        let r = reduce_diff(&d).expect("a 400-file diff must reduce");
        assert!(
            r.kept_lines <= MAX_KEPT + MIN_PER_FILE,
            "kept must stay near the cap, got {}",
            r.kept_lines
        );
        assert!(
            r.body.contains("more files changed, not shown"),
            "dropped files must be announced; body tail:\n{}",
            &r.body[r.body.len().saturating_sub(200)..]
        );
        assert!(
            r.body.len() < d.len() / 2,
            "the body must be substantially smaller: {} vs {}",
            r.body.len(),
            d.len()
        );
    }

    /// git's default is `-U3`; a context window of 2 made the whole pass a no-op
    /// by construction, so this pins the invariant rather than the number.
    #[test]
    fn context_window_is_below_gits_default_u3() {
        assert!(
            MAX_CONTEXT < 3,
            "MAX_CONTEXT={MAX_CONTEXT} must be < git's default -U3, or every \
             context line sits within reach of an anchor and nothing is trimmed"
        );
    }
}
