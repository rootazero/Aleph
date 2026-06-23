//! Unified-diff reducer: keep the `+`/`-` change lines and hunk/file headers,
//! trim surrounding context to a couple of lines. The changes *are* the signal;
//! the unchanged context is the noise a stale diff can shed.

use super::{render_selected, ContentKind, Reduction};

/// Context lines kept on each side of a run of changes.
const MAX_CONTEXT: usize = 2;
/// Hard cap on kept lines so a giant all-context diff still shrinks; the
/// surrounding stage's token guard provides the final safety net.
const MAX_KEPT: usize = 240;

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
    if kept.len() > MAX_KEPT {
        kept.truncate(MAX_KEPT);
    }
    let body = render_selected(&lines, &kept);
    Some(Reduction {
        kind: ContentKind::Diff,
        body,
        kept_lines: kept.len(),
        total_lines: total,
    })
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
}
