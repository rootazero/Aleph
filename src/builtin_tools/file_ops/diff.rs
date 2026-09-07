//! The ONLY `similar` call site in alephcore. Turns a pre-image + post-image
//! into the wire `FileChange` a file-mutating tool attaches under
//! `_presentation` (hoisted by `apply_layer_two`, never seen by the model).

use aleph_protocol::file_change::{
    FileChange, FileChangeKind, Hunk, HunkLine, LineTag, Presentation, Unavailable, CONTEXT_LINES,
    MAX_HUNK_LINES,
};
use similar::{ChangeTag, TextDiff};

/// Pre-images larger than this are not diffed. For `Created`/`Deleted` the
/// line count of the one existing side is still exact (it is trivially all
/// added or all removed). For `Modified`, neither side's line count is a
/// real stat on its own, so `added`/`removed` are reported as `0` —
/// **`0` under `Unavailable::TooLarge` always means "not computed", never
/// "no changes"**: the hunk-count cap further down only ever trips on a
/// diff that has real changes, so a genuinely too-large diff can never
/// legitimately show `0`/`0`. `shared-ui-logic::transcript::diff_view::
/// stats_label` relies on this invariant to avoid rendering `+0 -0` for a
/// diff it could not compute.
///
/// `TooLarge` has three producers and the invariant holds for all three: this
/// byte cap, the `MAX_HUNK_LINES` cap below, and [`DIFF_TIME_BUDGET`] (a diff
/// that exhausts a 250 ms budget is likewise never a no-change diff, which
/// costs microseconds). A fourth arrives from `execute_write` /
/// `apply_patch`'s pre-image read via [`read_pre_image`] when the OLD side is
/// over this same cap.
pub(crate) const MAX_DIFF_INPUT_BYTES: usize = 2 * 1024 * 1024;

/// Wall-clock ceiling on ONE `TextDiff::from_lines` call.
///
/// The byte cap above only refuses inputs *larger* than 2 MiB; at or below it
/// Myers is O(N·D), so two ~40k-line texts with maximal difference can block
/// this (synchronous, on a tokio worker) for far longer than the whole write
/// it decorates. `MAX_HUNK_LINES` does not help: it is applied AFTER the diff
/// is computed, so the pathological case is paid for in full and then thrown
/// away.
///
/// **Why 250 ms.** Any diff whose hunks survive `MAX_HUNK_LINES = 400` is by
/// construction a small edit script, and Myers resolves those in single-digit
/// milliseconds even at the 2 MiB input cap — so this is ~2 orders of
/// magnitude of headroom for every diff that would actually be shown. It is
/// also per-`FileChange`, and `apply_patch` calls this once per file in an
/// envelope, so the bound has to leave room for that multiplier inside the
/// 180 s tool budget.
const DIFF_TIME_BUDGET: std::time::Duration = std::time::Duration::from_millis(250);

fn count_lines(s: &str) -> u32 {
    u32::try_from(s.lines().count()).unwrap_or(u32::MAX)
}

/// The `TooLarge` degrade, with the stats that are still honest.
///
/// For `Created`/`Deleted` the one existing side's line count IS the exact
/// stat; for `Modified` neither side's count is a real stat, so both stay `0`
/// — the "not computed" reading `MAX_DIFF_INPUT_BYTES`'s doc owns. Shared by
/// the byte cap and the time budget so the two degrades cannot drift apart.
fn too_large(
    path: &str,
    kind: FileChangeKind,
    before: Option<&str>,
    after: Option<&str>,
) -> FileChange {
    let mut c = FileChange::unavailable(path, kind, Unavailable::TooLarge);
    c.added = match after {
        Some(new) if before.is_none() => count_lines(new),
        _ => 0,
    };
    c.removed = match before {
        Some(old) if after.is_none() => count_lines(old),
        _ => 0,
    };
    c
}

/// Best-effort pre-image read for the `_presentation` diff side-channel, and
/// the single owner of "why couldn't we read it".
///
/// `Ok(Some(text))` — diffable. `Ok(None)` — the file is **confirmed absent**
/// (`NotFound`), which is the only state a caller may read as `Created`.
/// `Err(reason)` — it exists (or we cannot tell) and the wire reason says
/// which of the three causes it was, rather than collapsing them into one
/// flag the way `file_write` and `apply_patch` each used to.
///
/// A non-`NotFound` `metadata` error is `PreImageUnavailable`, NOT `Ok(None)`:
/// "I could not stat it" is not "it did not exist", and reading it as the
/// latter makes a `Modified` change render as a brand-new file with the whole
/// post-image as additions (判据 §8 — a fail-closed answer consumed as a
/// value).
pub(crate) async fn read_pre_image(path: &std::path::Path) -> Result<Option<String>, Unavailable> {
    match tokio::fs::metadata(path).await {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(Unavailable::PreImageUnavailable),
        Ok(meta) if meta.len() > MAX_DIFF_INPUT_BYTES as u64 => return Err(Unavailable::TooLarge),
        Ok(_) => {}
    }
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|_| Unavailable::PreImageUnavailable)?;
    if super::is_binary(&bytes) {
        return Err(Unavailable::Binary);
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| Unavailable::Binary)
}

/// `before == None` → Created; `after == None` → Deleted; both → Modified.
/// Both `None` is a programming error and yields `ToolFailed`.
pub(crate) fn compute_file_change(
    path: &str,
    before: Option<&str>,
    after: Option<&str>,
) -> FileChange {
    compute_file_change_by(
        path,
        before,
        after,
        std::time::Instant::now() + DIFF_TIME_BUDGET,
    )
}

/// [`compute_file_change`] with the deadline supplied, so the degrade has a
/// test that can actually go red (hand it an already-elapsed instant) rather
/// than one that waits for a pathological input to be slow enough.
fn compute_file_change_by(
    path: &str,
    before: Option<&str>,
    after: Option<&str>,
    deadline: std::time::Instant,
) -> FileChange {
    let kind = match (before, after) {
        (None, Some(_)) => FileChangeKind::Created,
        (Some(_), None) => FileChangeKind::Deleted,
        (Some(_), Some(_)) => FileChangeKind::Modified,
        (None, None) => {
            return FileChange::unavailable(path, FileChangeKind::Modified, Unavailable::ToolFailed)
        }
    };
    let old = before.unwrap_or("");
    let new = after.unwrap_or("");
    if old.len() > MAX_DIFF_INPUT_BYTES || new.len() > MAX_DIFF_INPUT_BYTES {
        return too_large(path, kind, before, after);
    }

    // Bounded Myers: past the deadline `similar` stops looking for a middle
    // snake and emits a valid-but-not-minimal script (whole-range delete +
    // insert) — so the run terminates, but `added`/`removed` counted off it
    // would be the approximation's numbers, not the file's. There is no
    // "was I truncated" flag on `TextDiff`, so ask the clock immediately
    // after construction: if the deadline has passed, the algorithm MAY have
    // approximated and we report nothing rather than something wrong. The
    // false-positive window is a diff that finished optimally within
    // microseconds of the deadline; it degrades to `TooLarge`, which is the
    // fail-closed side.
    let diff = TextDiff::configure()
        .deadline(deadline)
        .diff_lines(old, new);
    if std::time::Instant::now() > deadline {
        return too_large(path, kind, before, after);
    }
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
                    ChangeTag::Delete => {
                        removed += 1;
                        LineTag::Del
                    }
                    ChangeTag::Insert => {
                        added += 1;
                        LineTag::Add
                    }
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
    let mut change = FileChange {
        path: path.to_string(),
        kind,
        hunks,
        added,
        removed,
        unavailable: None,
    };
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
        let new = (1..=1000)
            .map(|i| format!("{}\n", i * 2))
            .collect::<String>();
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

    #[test]
    fn a_modified_change_over_the_byte_cap_reports_too_large_with_unknown_zero_stats() {
        // Both sides are `Some` and both exceed `MAX_DIFF_INPUT_BYTES`, the
        // realistic case for this cap (editing an already-large file).
        let old = "a".repeat(MAX_DIFF_INPUT_BYTES + 1);
        let new = "b".repeat(MAX_DIFF_INPUT_BYTES + 1);
        let c = compute_file_change("big.txt", Some(&old), Some(&new));
        assert_eq!(c.kind, FileChangeKind::Modified);
        assert_eq!(c.unavailable, Some(Unavailable::TooLarge));
        assert!(c.hunks.is_empty());
        // Neither side's line count is a real stat here — both are 0,
        // meaning "not computed", per the invariant documented on
        // `MAX_DIFF_INPUT_BYTES`.
        assert_eq!((c.added, c.removed), (0, 0));
    }

    #[test]
    fn both_sides_missing_is_a_programming_error_reported_as_tool_failed() {
        let c = compute_file_change("a", None, None);
        assert_eq!(c.unavailable, Some(Unavailable::ToolFailed));
        assert!(c.hunks.is_empty());
        assert_eq!((c.added, c.removed), (0, 0));
    }

    #[test]
    fn a_created_file_over_the_byte_cap_still_reports_its_exact_line_count() {
        // The one existing side IS the exact stat — the shared `too_large`
        // degrade must not flatten this to the `Modified` case's zeros.
        let lines = MAX_DIFF_INPUT_BYTES / 2 + 1; // 2 bytes per line ⇒ over the cap
        let new = "x\n".repeat(lines);
        let c = compute_file_change("big.txt", None, Some(&new));
        assert_eq!(c.kind, FileChangeKind::Created);
        assert_eq!(c.unavailable, Some(Unavailable::TooLarge));
        assert_eq!((c.added, c.removed), (lines as u32, 0));
    }

    #[test]
    fn a_diff_that_runs_out_of_time_degrades_instead_of_reporting_approximated_stats() {
        // Past deadline ⇒ `similar` stops looking for a middle snake and
        // emits a valid-but-not-minimal script. Counting THAT would report
        // numbers the file does not have, so the answer is "not computed".
        let old = (1..=200).map(|i| format!("l{i}\n")).collect::<String>();
        let new = (1..=200).map(|i| format!("L{i}\n")).collect::<String>();
        let c =
            compute_file_change_by("slow.rs", Some(&old), Some(&new), std::time::Instant::now());
        assert_eq!(c.unavailable, Some(Unavailable::TooLarge));
        assert!(c.hunks.is_empty());
        assert_eq!(
            (c.added, c.removed),
            (0, 0),
            "0/0 under TooLarge is the documented 'not computed'"
        );
        // Same inputs with a real budget compute exact stats — so the guard
        // above is the deadline talking, not these inputs being undiffable.
        let ok = compute_file_change("slow.rs", Some(&old), Some(&new));
        assert_eq!(
            (ok.added, ok.removed),
            (200, 200),
            "with a real budget the same inputs are diffed exactly"
        );
    }

    #[tokio::test]
    async fn read_pre_image_names_the_cause_instead_of_collapsing_three_into_one() {
        let dir = tempfile::tempdir().unwrap();

        let missing = dir.path().join("nope.txt");
        assert_eq!(read_pre_image(&missing).await, Ok(None), "absent ⇒ Created");

        let text = dir.path().join("t.txt");
        tokio::fs::write(&text, "a\nb\n").await.unwrap();
        assert_eq!(read_pre_image(&text).await, Ok(Some("a\nb\n".to_string())));

        let bin = dir.path().join("b.dat");
        tokio::fs::write(&bin, [0xff, 0x00, 0xfe]).await.unwrap();
        assert_eq!(read_pre_image(&bin).await, Err(Unavailable::Binary));

        let big = dir.path().join("big.txt");
        tokio::fs::write(&big, vec![b'x'; MAX_DIFF_INPUT_BYTES + 1])
            .await
            .unwrap();
        assert_eq!(read_pre_image(&big).await, Err(Unavailable::TooLarge));
    }
}
