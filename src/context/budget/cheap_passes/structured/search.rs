//! grep/ripgrep search-result reducer: keep the first and last match per file
//! plus any error-bearing matches, capping per-file and overall. A long search
//! sweep's signal is *which files matched and the salient lines* — not every
//! occurrence.
//!
//! The line parser carries headroom's Rust-port fixes: a leading Windows drive
//! letter (`C:\...`) is not mistaken for the `path:line` separator, and the
//! line-number marker is anchored on the earliest `<sep><digits><sep>` run so
//! filenames containing dashes or colons parse correctly.

use super::{is_error_signal, render_selected, ContentKind, Reduction};

const MAX_PER_FILE: usize = 5;
const MAX_FILES: usize = 20;
const MAX_TOTAL: usize = 60;

/// Parse a grep/ripgrep line, returning the file path on success. Matches the
/// `path<sep>line<sep>content` shape where `<sep>` is `:` (a match line) or `-`
/// (a ripgrep context line). Returns `None` when there is no line-number
/// marker (headers, blank lines, grouped-format content lines).
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
                if !path.is_empty() {
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
    let mut order: Vec<&str> = Vec::new();
    let mut groups: Vec<(usize, Vec<usize>)> = Vec::new(); // parallel to `order`
    for (idx, &line) in lines.iter().enumerate() {
        let Some(path) = match_path(line) else {
            continue;
        };
        match order.iter().position(|&p| p == path) {
            Some(pos) => groups[pos].1.push(idx),
            None => {
                order.push(path);
                groups.push((idx, vec![idx]));
            }
        }
    }
    if groups.is_empty() {
        return None;
    }

    let mut kept: Vec<usize> = Vec::new();
    for (_first, idxs) in groups.iter().take(MAX_FILES) {
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
    let body = render_selected(&lines, &kept);
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
        assert_eq!(match_path("src/main.rs:42:    let x = 1;"), Some("src/main.rs"));
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
