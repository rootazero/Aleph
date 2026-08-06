//! grep/ripgrep search-result reducer: keep the first and last match per file
//! plus any error-bearing matches, capping per-file and overall. A long search
//! sweep's signal is *which files matched and the salient lines* — not every
//! occurrence.
//!
//! The line parser carries headroom's Rust-port fixes: a leading Windows drive
//! letter (`C:\...`) is not mistaken for the `path:line` separator, and the
//! line-number marker is anchored on the earliest `<sep><digits><sep>` run so
//! filenames containing dashes or colons parse correctly.

use std::collections::HashMap;

use super::{is_error_signal, render_selected, ContentKind, Profile, Reduction, Tally};

/// Whether `p` is plausibly a file path, as opposed to a timestamp that happens
/// to contain `<sep><digits><sep>`.
///
/// Without this test, `match_path` accepted the clock prefix of essentially every
/// structured log line — `2026-07-30 12:30:45 INFO …` yielded `Some("2026")`,
/// `[2026-07-30T12:30:45Z …]` yielded `Some("[2026")`, and
/// `Jul 30 06:46:12 host …` yielded `Some("Jul 30 06")`. `looks_like_search` then
/// matched 100 % of the lines, and because `candidates()` offers Search before
/// Log, every `docker logs` / `journalctl` / tracing capture was routed to the
/// grep reducer and crushed to five lines per pseudo-file (the year, or the
/// hour), taking most of its ERROR lines with it.
///
/// Three cheap rules:
/// 1. A real path carries at least one letter. Kills `12` and `2026/07/30 12`.
/// 2. **No whitespace.** grep and ripgrep write the path at column 0, so a
///    candidate containing a space is a line *prefix*, not a path — which is
///    what a log line's `… 12:30:00 server.go` looks like, and a bare
///    "has a separator" test accepted it because of the `/` in the date. The one
///    exception is a Windows drive prefix (`C:\Program Files\…`), where a space
///    inside a genuine path is ordinary. The trade-off is deliberate: a POSIX
///    path containing a space loses *grouping* (its hits route to the log
///    reducer instead), whereas admitting prefixes crushed every timestamped log
///    to five lines.
/// 3. A path separator then settles it; otherwise only a filename-shaped token
///    counts — which keeps single-file grep working for `main.rs:42:` *and* for
///    extensionless `Makefile:12:` / `Dockerfile:3:`, while still rejecting
///    `[2026-07-30T12` (bracket) and `INFO` is harmless (it would need four such
///    lines at 60 % density to matter).
fn looks_like_path(p: &str) -> bool {
    if !p.chars().any(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    let windows_drive = {
        let b = p.as_bytes();
        b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && (b[2] == b'\\' || b[2] == b'/')
    };
    if !windows_drive && p.chars().any(char::is_whitespace) {
        return false;
    }
    if p.contains('/') || p.contains('\\') {
        return true;
    }
    // Separator-less: a filename-shaped token. Leading digits are excluded so a
    // clock or version fragment cannot pass as a bare filename.
    !p.starts_with(|c: char| c.is_ascii_digit())
        && p.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+' | '@' | '~'))
}

/// Parse a grep/ripgrep line, returning the file path on success. Matches the
/// `path<sep>line<sep>content` shape where `<sep>` is `:` (a match line) or `-`
/// (a ripgrep context line). Returns `None` when there is no line-number marker
/// (headers, blank lines, grouped-format content lines) or when the candidate
/// prefix isn't path-shaped (see [`looks_like_path`]).
fn match_path(line: &str) -> Option<&str> {
    let bytes = line.as_bytes();
    // Skip a leading Windows drive colon ("C:\" or "C:/") so it isn't taken for
    // the path/line separator.
    let mut i = if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
    {
        2
    } else {
        0
    };
    while i < bytes.len() {
        let c = bytes[i];
        if c == b':' || c == b'-' {
            // Require digits immediately after, then a matching separator:
            // anchors on the line-number marker, so dashed/colon'd filenames
            // before it stay part of the path.
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 1 && j < bytes.len() && (bytes[j] == b':' || bytes[j] == b'-') {
                let path = &line[..i];
                if !path.is_empty() && looks_like_path(path) {
                    return Some(path);
                }
            }
        }
        i += 1;
    }
    None
}

/// Cheap detector: most non-empty lines parse as `path:line:` matches.
pub(super) fn looks_like_search(lines: &[&str]) -> bool {
    let non_empty = lines.iter().filter(|l| !l.trim().is_empty()).count();
    if non_empty == 0 {
        return false;
    }
    let matches = lines.iter().filter(|l| match_path(l).is_some()).count();
    matches >= 4 && matches * 100 >= non_empty * 60
}

pub(super) fn reduce_search(lines: &[&str], profile: &Profile) -> Option<Reduction> {
    let total = lines.len();

    // Group match-line indices by file, preserving first-seen file order.
    //
    // The index is a `HashMap`, not a linear probe over the seen paths: at
    // ingress this runs synchronously on the tool-result path, and the probe was
    // quadratic in (matches × distinct paths) — a repo-wide `rg` producing
    // 120 000 hits across 40 000 files spent ~3 s of blocking CPU deciding what
    // to throw away, nearly all of it on groups past the file cap that are then
    // discarded unread. Collection also stops opening new groups at the cap.
    //
    // Every distinct path still gets an index entry, so `index.len()` is the true
    // file tally; paths past `Profile::search_files` map to `UNGROUPED` and open
    // no group.
    // Counting in the `None` arm instead counted a *line* per hit for every path
    // past the cap — the entry was only inserted when a group was created, so
    // those paths took the `None` arm again on every subsequent line, and the
    // model was told "1 200 more files matched" for 380.
    const UNGROUPED: usize = usize::MAX;
    let mut index: HashMap<&str, usize> = HashMap::new();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (idx, &line) in lines.iter().enumerate() {
        let Some(path) = match_path(line) else {
            continue;
        };
        match index.get(path) {
            Some(&UNGROUPED) => {}
            Some(&pos) => groups[pos].push(idx),
            None => {
                let pos = if groups.len() < profile.search_files {
                    groups.push(vec![idx]);
                    groups.len() - 1
                } else {
                    UNGROUPED
                };
                index.insert(path, pos);
            }
        }
    }
    if groups.is_empty() {
        return None;
    }
    let distinct_paths = index.len();

    let mut kept: Vec<usize> = Vec::new();
    for idxs in &groups {
        // Check before extending, so the overall cap cannot admit a group only
        // to leave it unrendered — the tally has to match what is shown.
        if kept.len() >= profile.search_total {
            break;
        }
        kept.extend(select_for_file(lines, idxs, profile.search_per_file));
    }
    kept.sort_unstable();
    kept.dedup();
    if kept.len() > profile.search_total {
        kept.truncate(profile.search_total);
    }
    if kept.len() >= total {
        return None; // nothing dropped
    }
    let mut body = render_selected(lines, &kept, total, profile);
    // Dropping whole files has to be visible: otherwise a sweep across 400 files
    // reads as a complete answer covering 20. Counted from what actually survived
    // rendering, not from the group count — the overall cap and the truncate
    // above can both drop a group that was admitted.
    let files_shown = kept
        .iter()
        .filter_map(|&i| match_path(lines[i]))
        .collect::<std::collections::HashSet<_>>()
        .len();
    if distinct_paths > files_shown {
        body.push_str(&format!(
            "\n… ({} more files matched, not shown) …",
            distinct_paths - files_shown
        ));
    }
    Some(Reduction {
        kind: ContentKind::Search,
        body,
        tally: Tally::Lines {
            kept: kept.len(),
            total,
        },
    })
}

/// Pick which of a file's match-line indices to keep: always the first and
/// last, then fill the middle budget with error-bearing matches first, then
/// the earliest remaining ones.
fn select_for_file(lines: &[&str], idxs: &[usize], max_per_file: usize) -> Vec<usize> {
    if idxs.len() <= max_per_file {
        return idxs.to_vec();
    }
    let first = idxs[0];
    let last = idxs[idxs.len() - 1];
    // `Profile::FLOOR` keeps this at 2, so the subtraction is saturating rather
    // than a floor assumption: at the floor a file keeps only its first and last
    // hit, which is still the shape the doc comment promises.
    let budget = max_per_file.saturating_sub(2); // slots between first and last
    let middle = &idxs[1..idxs.len() - 1];

    let mut sel = vec![first];
    let errs: Vec<usize> = middle
        .iter()
        .copied()
        .filter(|&i| is_error_signal(lines[i]))
        .collect();
    for &i in errs.iter().take(budget) {
        sel.push(i);
    }
    if sel.len() - 1 < budget {
        let remaining = budget - (sel.len() - 1);
        // Membership tested once per middle line; a linear scan over `errs`
        // made that quadratic in the per-file hit count.
        let err_set: std::collections::HashSet<usize> = errs.iter().copied().collect();
        for &i in middle
            .iter()
            .filter(|&&i| !err_set.contains(&i))
            .take(remaining)
        {
            sel.push(i);
        }
    }
    sel.push(last);
    sel.sort_unstable();
    sel.dedup();
    sel
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reduce(text: &str) -> Option<Reduction> {
        let lines: Vec<&str> = text.lines().collect();
        reduce_search(&lines, &Profile::DEFAULT)
    }

    #[test]
    fn parses_plain_grep_line() {
        assert_eq!(
            match_path("src/main.rs:42:    let x = 1;"),
            Some("src/main.rs")
        );
    }

    #[test]
    fn parses_dashed_filename() {
        // The dash in the filename must not be taken for the separator.
        assert_eq!(
            match_path("my-cool-file.rs:7:fn main() {}"),
            Some("my-cool-file.rs")
        );
    }

    #[test]
    fn parses_windows_drive_path() {
        assert_eq!(
            match_path(r"C:\proj\src\main.rs:99:code"),
            Some(r"C:\proj\src\main.rs")
        );
    }

    #[test]
    fn parses_ripgrep_context_dash() {
        assert_eq!(match_path("src/lib.rs-41-context line"), Some("src/lib.rs"));
    }

    #[test]
    fn rejects_non_match_line() {
        assert_eq!(match_path("just some prose without a marker"), None);
        assert_eq!(match_path("42:content"), None, "no path before the number");
    }

    #[test]
    fn detects_search_block() {
        let block = "a.rs:1:x\na.rs:2:y\nb.rs:3:z\nc.rs:4:w\nd.rs:5:v\n";
        let lines: Vec<&str> = block.lines().collect();
        assert!(looks_like_search(&lines));
    }

    #[test]
    fn reduces_many_matches_per_file() {
        let mut s = String::new();
        for i in 0..40 {
            s.push_str(&format!("big.rs:{i}:match number {i}\n"));
        }
        let r = reduce(&s).expect("should reduce");
        assert_eq!(r.kind, ContentKind::Search);
        assert!(
            r.tally.kept() <= Profile::DEFAULT.search_per_file,
            "per-file cap respected"
        );
        // First and last matches survive.
        assert!(r.body.contains("big.rs:0:"));
        assert!(r.body.contains("big.rs:39:"));
        assert!(r.body.contains("lines omitted"));
    }

    #[test]
    fn keeps_error_matches() {
        let mut s = String::new();
        s.push_str("f.rs:1:fine\n");
        for i in 2..8 {
            s.push_str(&format!("f.rs:{i}:fine line {i}\n"));
        }
        s.push_str("f.rs:8:ERROR something exploded\n");
        for i in 9..14 {
            s.push_str(&format!("f.rs:{i}:fine line {i}\n"));
        }
        let r = reduce(&s).expect("should reduce");
        assert!(
            r.body.contains("ERROR something exploded"),
            "error-bearing match must be kept; got:\n{}",
            r.body
        );
    }

    #[test]
    fn timestamped_log_lines_are_not_search_hits() {
        // Every one of these used to parse as a `path:line:` hit, which routed
        // whole logs to this reducer and crushed them to five lines.
        for line in [
            "2026-07-30 12:30:45 INFO aleph::gateway: started",
            "[2026-07-30T12:30:45Z INFO alephcore] booting",
            "Jul 30 06:46:12 host aleph[123]: hello",
            "12:30:45 INFO plain clock log line",
            "2026/07/30 12:30:00 server.go:15: listening",
            "2026-07-30T12:30:45.123Z WARN retrying in 5s",
        ] {
            assert_eq!(match_path(line), None, "must not parse as a hit: {line}");
        }
    }

    #[test]
    fn real_grep_shapes_still_parse() {
        for (line, want) in [
            ("src/main.rs:42: let x = 1;", "src/main.rs"),
            ("src/lib.rs-41-    context", "src/lib.rs"),
            ("main.rs:7: fn main() {}", "main.rs"),
            ("my-cool-file.rs:9: hit", "my-cool-file.rs"),
            // Extensionless and dot-prefixed files are real and common.
            ("Makefile:12: all:", "Makefile"),
            ("Dockerfile:3: RUN apt-get", "Dockerfile"),
            (".gitignore:4: target/", ".gitignore"),
            ("C:\\proj\\src\\main.rs:10: hit", "C:\\proj\\src\\main.rs"),
            // A Windows path with a space is the one whitespace exception.
            (
                "C:\\Program Files\\app\\x.rs:10: hit",
                "C:\\Program Files\\app\\x.rs",
            ),
        ] {
            assert_eq!(match_path(line), Some(want), "must parse: {line}");
        }
    }

    /// A tight budget must reach the grouping and per-file selection, not arrive
    /// as a blind cut of a default-sized body — and the "N more files matched"
    /// note must stay truthful under the tighter caps.
    #[test]
    fn a_tight_budget_narrows_the_sweep_and_keeps_the_tally_honest() {
        let mut s = String::new();
        for f in 0..30 {
            for l in 0..6 {
                s.push_str(&format!("src/f{f}.rs:{}: let target = 1;\n", l + 1));
            }
        }
        let wide = reduce(&s).expect("default must reduce");
        let tight_lines: Vec<&str> = s.lines().collect();
        let tight = reduce_search(&tight_lines, &Profile::for_token_budget(500))
            .expect("tight must reduce");
        assert!(
            tight.tally.kept() < wide.tally.kept(),
            "tight kept {} vs default {}",
            tight.tally.kept(),
            wide.tally.kept()
        );
        let shown: std::collections::HashSet<&str> =
            tight.body.lines().filter_map(match_path).collect();
        let note = tight
            .body
            .lines()
            .find(|l| l.contains("more files matched"))
            .expect("the note must be present");
        let reported: usize = note
            .split_whitespace()
            .find_map(|w| w.trim_start_matches('(').parse::<usize>().ok())
            .expect("the note carries a number");
        assert_eq!(reported, 30 - shown.len(), "note: {note}");
    }

    /// The tally is per FILE, not per matching line, and it counts what actually
    /// rendered — both halves were wrong and both over-reported.
    #[test]
    fn the_dropped_file_tally_counts_files_that_were_not_shown() {
        // 30 files x 4 hits: the default profile opens 20 groups, and its
        // 60-match overall cap stops well before all 20 are rendered.
        let mut s = String::new();
        for f in 0..30 {
            for l in 0..4 {
                s.push_str(&format!("src/f{f}.rs:{}: let target = 1;\n", l + 1));
            }
        }
        let r = reduce(&s).expect("must reduce");

        let shown: std::collections::HashSet<&str> =
            r.body.lines().filter_map(match_path).collect();
        let note = r
            .body
            .lines()
            .find(|l| l.contains("more files matched"))
            .expect("the note must be present");
        let reported: usize = note
            .split_whitespace()
            .find_map(|w| w.trim_start_matches('(').parse::<usize>().ok())
            .expect("the note carries a number");
        assert_eq!(
            reported,
            30 - shown.len(),
            "note said {reported} more; {} files shown of 30",
            shown.len()
        );
        assert!(
            reported < 30,
            "the tally must count files, not matching lines: {note}"
        );
    }
}
