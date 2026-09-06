//! The ONLY `similar` call site in alephcore. Turns a pre-image + post-image
//! into the wire `FileChange` a file-mutating tool attaches under
//! `_presentation` (hoisted by `apply_layer_two`, never seen by the model).

use aleph_protocol::file_change::{
    FileChange, FileChangeKind, Hunk, HunkLine, LineTag, Presentation, Unavailable, CONTEXT_LINES,
    MAX_HUNK_LINES,
};
use similar::{ChangeTag, TextDiff};

/// Pre-images larger than this are not diffed (stats still counted by lines).
pub(crate) const MAX_DIFF_INPUT_BYTES: usize = 2 * 1024 * 1024;

fn count_lines(s: &str) -> u32 {
    u32::try_from(s.lines().count()).unwrap_or(u32::MAX)
}

/// `before == None` → Created; `after == None` → Deleted; both → Modified.
/// Both `None` is a programming error and yields `ToolFailed`.
pub(crate) fn compute_file_change(path: &str, before: Option<&str>, after: Option<&str>) -> FileChange {
    let kind = match (before, after) {
        (None, Some(_)) => FileChangeKind::Created,
        (Some(_), None) => FileChangeKind::Deleted,
        (Some(_), Some(_)) => FileChangeKind::Modified,
        (None, None) => return FileChange::unavailable(path, FileChangeKind::Modified, Unavailable::ToolFailed),
    };
    let old = before.unwrap_or("");
    let new = after.unwrap_or("");
    if old.len() > MAX_DIFF_INPUT_BYTES || new.len() > MAX_DIFF_INPUT_BYTES {
        let mut c = FileChange::unavailable(path, kind, Unavailable::TooLarge);
        c.added = if before.is_none() { count_lines(new) } else { 0 };
        c.removed = if after.is_none() { count_lines(old) } else { 0 };
        return c;
    }

    let diff = TextDiff::from_lines(old, new);
    let mut added = 0u32;
    let mut removed = 0u32;
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut total_lines = 0usize;
    for group in diff.grouped_ops(CONTEXT_LINES) {
        let Some(first) = group.first() else { continue };
        let (old_start, new_start) = (first.old_range().start + 1, first.new_range().start + 1);
        let mut lines: Vec<HunkLine> = Vec::new();
        for op in &group {
            for change in diff.iter_changes(op) {
                let tag = match change.tag() {
                    ChangeTag::Equal => LineTag::Ctx,
                    ChangeTag::Delete => { removed += 1; LineTag::Del }
                    ChangeTag::Insert => { added += 1; LineTag::Add }
                };
                let text = change.value().trim_end_matches(['\n', '\r']).to_string();
                lines.push(HunkLine { tag, text });
            }
        }
        total_lines += lines.len();
        hunks.push(Hunk {
            old_start: u32::try_from(old_start).unwrap_or(u32::MAX),
            new_start: u32::try_from(new_start).unwrap_or(u32::MAX),
            lines,
        });
    }
    let mut change = FileChange { path: path.to_string(), kind, hunks, added, removed, unavailable: None };
    if total_lines > MAX_HUNK_LINES {
        change.hunks.clear();
        change.unavailable = Some(Unavailable::TooLarge);
    }
    change
}

pub(crate) fn presentation_for(changes: Vec<FileChange>) -> Presentation {
    Presentation::FileChanges { changes }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_one_line_edit_yields_one_hunk_with_context_and_exact_stats() {
        let old = (1..=20).map(|i| format!("line {i}\n")).collect::<String>();
        let new = old.replace("line 10\n", "line ten\n");
        let c = compute_file_change("a.rs", Some(&old), Some(&new));
        assert_eq!(c.kind, FileChangeKind::Modified);
        assert_eq!((c.added, c.removed), (1, 1));
        assert_eq!(c.hunks.len(), 1);
        let h = &c.hunks[0];
        assert_eq!((h.old_start, h.new_start), (6, 6)); // 4 context lines before line 10
        assert_eq!(h.lines.iter().filter(|l| l.tag == LineTag::Del).count(), 1);
        assert_eq!(h.lines.iter().filter(|l| l.tag == LineTag::Add).count(), 1);
        assert!(h.lines.iter().all(|l| !l.text.ends_with('\n')));
    }

    #[test]
    fn two_distant_edits_yield_two_hunks() {
        let old = (1..=100).map(|i| format!("l{i}\n")).collect::<String>();
        let new = old.replace("l5\n", "L5\n").replace("l90\n", "L90\n");
        let c = compute_file_change("a", Some(&old), Some(&new));
        assert_eq!(c.hunks.len(), 2);
    }

    #[test]
    fn created_and_deleted_files_are_all_adds_or_all_dels() {
        let c = compute_file_change("n.rs", None, Some("a\nb\n"));
        assert_eq!(c.kind, FileChangeKind::Created);
        assert_eq!((c.added, c.removed), (2, 0));
        let d = compute_file_change("n.rs", Some("a\nb\nc\n"), None);
        assert_eq!(d.kind, FileChangeKind::Deleted);
        assert_eq!((d.added, d.removed), (0, 3));
    }

    #[test]
    fn a_huge_change_keeps_stats_but_drops_hunks_as_too_large() {
        let old = (1..=1000).map(|i| format!("{i}\n")).collect::<String>();
        let new = (1..=1000).map(|i| format!("{}\n", i * 2)).collect::<String>();
        let c = compute_file_change("a", Some(&old), Some(&new));
        assert_eq!(c.unavailable, Some(Unavailable::TooLarge));
        assert!(c.hunks.is_empty());
        assert!(c.added > 0 && c.removed > 0, "stats survive the cap");
    }

    #[test]
    fn crlf_text_diffs_without_carriage_returns_leaking_into_rows() {
        let c = compute_file_change("w.txt", Some("a\r\nb\r\n"), Some("a\r\nB\r\n"));
        assert!(c.hunks[0].lines.iter().all(|l| !l.text.contains('\r')));
    }

    #[test]
    fn identical_content_is_a_modified_change_with_no_hunks() {
        let c = compute_file_change("a", Some("x\n"), Some("x\n"));
        assert!(c.hunks.is_empty());
        assert_eq!((c.added, c.removed), (0, 0));
        assert!(c.unavailable.is_none());
    }
}
