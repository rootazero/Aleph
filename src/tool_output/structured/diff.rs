//! Unified-diff reducer: keep the `+`/`-` change lines and hunk/file headers,
//! trim surrounding context to a couple of lines. The changes *are* the signal;
//! the unchanged context is the noise a stale diff can shed.

use super::{render_selected, ContentKind, Profile, Reduction, Tally};

/// Context lines kept on each side of a run of changes.
///
/// Must stay **strictly below** git's default `-U3`, or the pass is a no-op by
/// construction: with a 3-line context every unchanged line is within 2 of an
/// anchor, so the old value of 2 kept 3011 of 3024 lines on a real 26-file diff
/// and the only thing that shrank the output was the blind head truncate below.
const MAX_CONTEXT: usize = 1;
/// Floor on lines allotted to each file when the cap forces a per-file split, so
/// every file keeps its header plus a little detail rather than vanishing.
///
/// Structural, not a size knob: below four lines a file's section cannot carry
/// its own header *and* a change line, and a "diff" with no change line in it is
/// not a diff. The cap it is measured against is [`Profile::diff_lines`].
const MIN_PER_FILE: usize = 4;

/// Cheap detector: a unified diff has unmistakable structural markers.
pub(super) fn looks_like_diff(lines: &[&str]) -> bool {
    let mut added = 0usize;
    let mut removed = 0usize;
    for &l in lines {
        if l.starts_with("diff --git ") {
            return true;
        }
        if l.starts_with("@@ ") && l.matches("@@").count() >= 2 {
            return true;
        }
        if is_change(l) {
            if l.starts_with('+') {
                added += 1;
            } else {
                removed += 1;
            }
        }
    }
    // A header-less fragment (e.g. a `diff -u` paste without `diff --git`) still
    // qualifies if a solid fraction of lines are `+`/`-` changes — but it must
    // carry **both** directions.
    //
    // "Enough lines start with `-`" alone is the shape of a **markdown bullet
    // list**: a changelog, a release-notes page or any fetched `- item` list is
    // ~100 % dash-prefixed and sailed through. That misclassification is fatal
    // rather than merely wasteful, because a Diff verdict is *exclusive* (see
    // `candidates`): the log and search reducers never get a look, and the diff
    // reducer — which keeps "change" lines and trims context — kept the first
    // `MAX_KEPT` bullets and dropped the error lines at the end, under a header
    // reading `[compacted diff: kept 240/603 lines]`.
    //
    // A header-less pure-deletion (or pure-addition) paste is genuinely rare and
    // indistinguishable from a list, so it falls through to the log reducer or
    // head/tail truncation — both of which are safe, unlike the reverse mistake.
    added >= 1 && removed >= 1 && added + removed >= 4 && (added + removed) * 2 >= lines.len()
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

pub(super) fn reduce_diff(lines: &[&str], profile: &Profile) -> Option<Reduction> {
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
    let files_total = count_file_starts(lines, 0..total);
    if kept.len() > profile.diff_lines {
        kept = trim_per_file(lines, &kept, profile.diff_lines);
    }
    let mut body = render_selected(lines, &kept, total, profile);
    let files_shown = count_file_starts(lines, kept.iter().copied());
    if files_total > files_shown {
        body.push_str(&format!(
            "\n… ({} more files changed, not shown) …",
            files_total - files_shown
        ));
    }
    Some(Reduction {
        kind: ContentKind::Diff,
        body,
        tally: Tally::Lines {
            kept: kept.len(),
            total,
        },
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

/// Spread `max_kept` across the files the diff touches instead of taking the
/// first `max_kept` kept lines.
///
/// The head truncate this replaces was the reason a 26-file diff reached the
/// model as **3 files** under a header (`kept 240/3024 lines`) that implied
/// uniform line-level thinning — so a model asked to review the change would
/// confidently report that it touched three files. Each file now keeps its
/// section header plus a share of its own detail, and change lines are preferred
/// over context lines inside that share.
fn trim_per_file(lines: &[&str], kept: &[usize], max_kept: usize) -> Vec<usize> {
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
    // push the total back past `max_kept` (500 files × 4 = 2000 lines, each
    // separated by its own omission marker). Bound the number of files that get
    // detail; the rest are reported by the trailing "N more files changed" note,
    // which is the honest answer at that width anyway.
    let max_files = (max_kept / MIN_PER_FILE).max(1);
    let runs = &runs[..runs.len().min(max_files)];
    let quota = (max_kept / runs.len().max(1)).max(MIN_PER_FILE);

    let mut out: Vec<usize> = Vec::new();
    for run in runs {
        if run.len() <= quota {
            out.extend(run.iter().copied());
            continue;
        }
        // Three priority tiers over this file's own lines: structural headers
        // (they name the file and its hunks), then changes, then context.
        //
        // Headers get at most *half* the quota. Taking them greedily starved the
        // tier that matters: git emits 4 file headers plus one `@@` per hunk, so
        // at 48+ files (quota == MIN_PER_FILE == 4) the header tier consumed every
        // slot and the reduction contained **zero change lines** — a diff summary
        // with no diff in it, under a header claiming it kept 240 lines.
        let mut picked = vec![false; run.len()];
        let mut n = 0usize;
        for tier in 0..3u8 {
            let tier_ceiling = match tier {
                0 => (quota / 2).max(1),
                _ => quota,
            };
            for (pos, &i) in run.iter().enumerate() {
                if n >= tier_ceiling {
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

    fn reduce(text: &str) -> Option<Reduction> {
        let lines: Vec<&str> = text.lines().collect();
        reduce_diff(&lines, &Profile::DEFAULT)
    }

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

    /// A markdown bullet list is ~100 % `-`-prefixed, which the old "enough
    /// lines start with a change marker" gate accepted outright. Because a Diff
    /// verdict is exclusive, a fetched changelog was then handed to the diff
    /// reducer, which kept the first `MAX_KEPT` bullets and dropped the error
    /// lines at the end — under a header claiming it had compacted a diff.
    #[test]
    fn a_markdown_bullet_list_is_not_a_diff() {
        let mut page = String::from("Release notes\n");
        for i in 0..600 {
            page.push_str(&format!(
                "- changelog entry {i} about nothing in particular\n"
            ));
        }
        page.push_str("error: the build failed\n");
        page.push_str("Total: 3 errors, 1 warning across 600 entries\n");
        let lines: Vec<&str> = page.lines().collect();
        assert!(
            !looks_like_diff(&lines),
            "a dash-prefixed list carries no additions and is not a diff"
        );
        // …and the log reducer, which does get a look now, keeps the signal.
        let r = super::super::reduce(&page).expect("a page with a summary reduces");
        assert_eq!(r.kind, ContentKind::Log);
        assert!(
            r.body.contains("error: the build failed"),
            "got:\n{}",
            r.body
        );
    }

    /// The real header-less fragment the fallback exists for still qualifies.
    #[test]
    fn a_headerless_fragment_with_both_directions_is_still_a_diff() {
        let frag = "-removed one\n-removed two\n+added one\n+added two\n+added three\n context\n";
        let lines: Vec<&str> = frag.lines().collect();
        assert!(looks_like_diff(&lines));
    }

    #[test]
    fn keeps_changes_drops_context() {
        let d = sample_diff();
        let r = reduce(&d).expect("diff should reduce");
        assert_eq!(r.kind, ContentKind::Diff);
        // The change lines survive.
        assert!(r.body.contains("-    let old = 1;"));
        assert!(r.body.contains("+    let new = 2;"));
        // The bulk of context is dropped.
        assert!(r.tally.kept() < r.tally.total());
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
        assert!(reduce(&s).is_none(), "an all-change diff should not reduce");
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
        let r = reduce(&d).expect("a 400-file diff must reduce");
        assert!(
            r.tally.kept() <= Profile::DEFAULT.diff_lines + MIN_PER_FILE,
            "kept must stay near the cap, got {}",
            r.tally.kept()
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

    /// A wide diff must still contain actual changes. Greedy header selection made
    /// a 48+-file diff reduce to nothing but per-file headers — zero `+`/`-` lines
    /// and zero hunk headers — while the summary claimed 240 kept lines.
    #[test]
    fn a_wide_diff_still_keeps_change_lines() {
        // 26 files of this shape is mostly signal, so `reduce` correctly declines
        // on the byte guard and head/tail truncation takes over. The starvation bug
        // lived at the widths where the per-file quota actually binds.
        for files in [48usize, 60, 120, 400] {
            let mut d = String::new();
            for f in 0..files {
                d.push_str(&format!("diff --git a/f{f}.rs b/f{f}.rs\n"));
                d.push_str("index 1111111..2222222 100644\n");
                d.push_str(&format!("--- a/f{f}.rs\n+++ b/f{f}.rs\n"));
                d.push_str("@@ -1,6 +1,6 @@\n");
                for c in 0..6 {
                    d.push_str(&format!(" context {c}\n"));
                }
                d.push_str("-let old = 1;\n+let new = 2;\n");
            }
            let r = super::super::reduce(&d).expect("must reduce");
            assert_eq!(r.kind, ContentKind::Diff, "files={files}");
            let changes = r.body.lines().filter(|l| is_change(l)).count();
            assert!(
                changes > 0,
                "files={files}: a diff reduction with no change lines is not a diff \
                 (kept {}/{})",
                r.tally.kept(),
                r.tally.total()
            );
        }
    }

    /// A tight budget must arrive as fewer kept lines *inside* the reducer, which
    /// is the only component that knows a `+`/`-` line outranks a context line.
    /// Before profiles, a 6 000-token tool got a 240-line body and
    /// `apply_result_budget` then head/tail-cut it — amputating the trailing
    /// "N more files changed" note along with the last files.
    #[test]
    fn a_tight_budget_keeps_fewer_lines_and_still_keeps_changes() {
        let mut d = String::new();
        for f in 0..60 {
            d.push_str(&format!("diff --git a/f{f}.rs b/f{f}.rs\n"));
            d.push_str("index 1111111..2222222 100644\n");
            d.push_str(&format!("--- a/f{f}.rs\n+++ b/f{f}.rs\n"));
            d.push_str("@@ -1,6 +1,6 @@\n");
            for c in 0..6 {
                d.push_str(&format!(" context {c}\n"));
            }
            d.push_str("-let old = 1;\n+let new = 2;\n");
        }
        let wide = reduce(&d).expect("default profile must reduce");
        let tight_lines: Vec<&str> = d.lines().collect();
        let tight =
            reduce_diff(&tight_lines, &Profile::for_token_budget(500)).expect("tight must reduce");
        assert!(
            tight.tally.kept() < wide.tally.kept(),
            "tight kept {} vs default {}",
            tight.tally.kept(),
            wide.tally.kept()
        );
        assert!(
            tight.body.lines().filter(|l| is_change(l)).count() > 0,
            "a diff reduction with no change lines is not a diff:\n{}",
            tight.body
        );
        assert!(
            tight.body.contains("more files changed, not shown"),
            "the dropped files must still be announced"
        );
    }

    /// git's default is `-U3`; a context window of 2 made the whole pass a no-op
    /// by construction, so this pins the invariant rather than the number.
    #[test]
    fn context_window_is_below_gits_default_u3() {
        // Const-evaluated: the invariant is a property of the constant, so a
        // failure is a compile error, not a runtime test failure.
        const {
            assert!(
                MAX_CONTEXT < 3,
                "MAX_CONTEXT must be < git's default -U3, or every context line \
                 sits within reach of an anchor and nothing is trimmed"
            );
        }
    }
}
