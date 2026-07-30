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

use super::{is_error_signal, render_selected, ContentKind, Reduction};

const MAX_PER_FILE: usize = 5;
const MAX_FILES: usize = 20;
const MAX_TOTAL: usize = 60;

/// Parse a grep/ripgrep line, returning the file path on success. Matches the
/// `path<sep>line<sep>content` shape where `<sep>` is `:` (a match line) or `-`
/// (a ripgrep context line). Returns `None` when there is no line-number
/// marker (headers, blank lines, grouped-format content lines).
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
/// Three cheap rules, in this order:
/// 1. A real path carries at least one letter. Kills `12`, `2026/07/30 12` and
///    Go's default `2009/11/10 23:00:00` log stamp.
/// 2. A path separator settles it — checked *before* the space rule so
///    `C:\Program Files\x.rs` still parses.
/// 3. Otherwise only a bare filename with a short alphanumeric extension counts,
///    which is what keeps single-file grep (`main.rs:42:`) working while
///    rejecting `[2026-07-30T12`.
fn looks_like_path(p: &str) -> bool {
    if !p.chars().any(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    if p.contains('/') || p.contains('\\') {
        return true;
    }
    if p.contains(' ') || p.contains('\t') {
        return false;
    }
    p.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty()
            && !ext.is_empty()
            && ext.len() <= 8
            && ext.chars().all(|c| c.is_ascii_alphanumeric())
    })
}

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

pub(super) fn reduce_search(text: &str) -> Option<Reduction> {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();

    // Group match-line indices by file, preserving first-seen file order.
    //
    // The index is a `HashMap`, not a linear probe over the seen paths: at
    // ingress this runs synchronously on the tool-result path, and the probe was
    // quadratic in (matches × distinct paths) — a repo-wide `rg` producing
    // 120 000 hits across 40 000 files spent ~3 s of blocking CPU deciding what
    // to throw away, nearly all of it on groups past `MAX_FILES` that are then
    // discarded unread. Collection also stops opening new groups at `MAX_FILES`
    // for the same reason; `distinct_paths` still counts them so the tally we
    // report to the model stays truthful.
    let mut index: HashMap<&str, usize> = HashMap::new();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut distinct_paths = 0usize;
    for (idx, &line) in lines.iter().enumerate() {
        let Some(path) = match_path(line) else {
            continue;
        };
        match index.get(path) {
            Some(&pos) => groups[pos].push(idx),
            None => {
                distinct_paths += 1;
                if groups.len() < MAX_FILES {
                    index.insert(path, groups.len());
                    groups.push(vec![idx]);
                }
            }
        }
    }
    if groups.is_empty() {
        return None;
    }

    let mut kept: Vec<usize> = Vec::new();
    for idxs in &groups {
        kept.extend(select_for_file(&lines, idxs));
        if kept.len() >= MAX_TOTAL {
            break;
        }
    }
    kept.sort_unstable();
    kept.dedup();
    if kept.len() > MAX_TOTAL {
        kept.truncate(MAX_TOTAL);
    }
    if kept.len() >= total {
        return None; // nothing dropped
    }
    let mut body = render_selected(&lines, &kept, total);
    // Dropping whole files has to be visible: otherwise a sweep across 400 files
    // reads as a complete answer covering 20.
    if distinct_paths > groups.len() {
        body.push_str(&format!(
            "\n… ({} more files matched, not shown) …",
            distinct_paths - groups.len()
        ));
    }
    Some(Reduction {
        kind: ContentKind::Search,
        body,
        kept_lines: kept.len(),
        total_lines: total,
    })
}

/// Pick which of a file's match-line indices to keep: always the first and
/// last, then fill the middle budget with error-bearing matches first, then
/// the earliest remaining ones.
fn select_for_file(lines: &[&str], idxs: &[usize]) -> Vec<usize> {
    if idxs.len() <= MAX_PER_FILE {
        return idxs.to_vec();
    }
    let first = idxs[0];
    let last = idxs[idxs.len() - 1];
    let budget = MAX_PER_FILE - 2; // slots between first and last
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
        for &i in middle
            .iter()
            .filter(|&&i| !errs.contains(&i))
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
        let r = reduce_search(&s).expect("should reduce");
        assert_eq!(r.kind, ContentKind::Search);
        assert!(r.kept_lines <= MAX_PER_FILE, "per-file cap respected");
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
        let r = reduce_search(&s).expect("should reduce");
        assert!(
            r.body.contains("ERROR something exploded"),
            "error-bearing match must be kept; got:\n{}",
            r.body
        );
    }
}
