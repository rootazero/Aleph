//! Edit-target location for `file_edit`.
//!
//! `file_edit` historically did a single exact `str::matches` pass: one stray
//! typographic character or a copied line-number prefix would fail the whole
//! edit with an opaque "not found". This module adds two improvements inspired
//! by codex's `apply-patch` `seek_sequence`, adapted to Aleph's exact-match
//! editing model:
//!
//!  1. **Typographic folding (auto-applied).** When the exact pass finds
//!     nothing, retry after folding Unicode dashes / curly quotes / exotic
//!     spaces to ASCII. The fold is strictly 1 char → 1 char, so it preserves
//!     character structure and *cannot* match across indentation — only the
//!     intended typographic drift is bridged.
//!  2. **Actionable diagnostics (never auto-applied).** When nothing matches,
//!     detect *why*: a copied line-number prefix, or a whitespace/indentation
//!     drift, and tell the model exactly how to fix its input. Indentation is
//!     never silently "corrected" — a clean failure beats a wrong edit.

/// Upper bound on file size for the typographic-folding fallback. Beyond this
/// the (transient) `Vec<char>` buffers are not worth it; exact matching still
/// works, and edits to multi-megabyte files essentially never need fuzzing.
const FUZZY_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Result of locating `old_string` within file content.
pub(super) enum LocateResult {
    /// Exact byte-substring occurrences (ascending, non-overlapping).
    Exact(Vec<(usize, usize)>),
    /// Occurrences found only after folding typographic punctuation.
    Folded(Vec<(usize, usize)>),
    /// Not found — carries an actionable diagnostic for the model.
    NotFound(String),
}

/// Locate every occurrence of `needle` in `content`.
///
/// Tries an exact pass first, then the typographic-folding fallback, and
/// finally produces a diagnostic explaining the miss.
pub(super) fn locate(content: &str, needle: &str) -> LocateResult {
    let exact: Vec<(usize, usize)> = content
        .match_indices(needle)
        .map(|(i, m)| (i, i + m.len()))
        .collect();
    if !exact.is_empty() {
        return LocateResult::Exact(exact);
    }

    if content.len() <= FUZZY_MAX_BYTES {
        let folded = folded_match_ranges(content, needle);
        if !folded.is_empty() {
            return LocateResult::Folded(folded);
        }
    }

    LocateResult::NotFound(diagnose(content, needle))
}

/// Splice `replacement` into `content` at each `range` (byte ranges, ascending,
/// non-overlapping). Applied back-to-front so earlier edits never shift the
/// offsets of later ones.
pub(super) fn apply_ranges(content: &str, ranges: &[(usize, usize)], replacement: &str) -> String {
    let mut result = content.to_string();
    for &(start, end) in ranges.iter().rev() {
        result.replace_range(start..end, replacement);
    }
    result
}

/// Find non-overlapping occurrences of `needle` in `haystack` after folding
/// typographic punctuation on both sides. Returns byte ranges in the *original*
/// `haystack`.
fn folded_match_ranges(haystack: &str, needle: &str) -> Vec<(usize, usize)> {
    let hay: Vec<char> = haystack.chars().map(fold_char).collect();
    let ndl: Vec<char> = needle.chars().map(fold_char).collect();
    if ndl.is_empty() || ndl.len() > hay.len() {
        return Vec::new();
    }

    // Byte offset in the original `haystack` for each char index, plus a
    // sentinel at the end. `fold_char` maps 1 char → 1 char, so char indices
    // are identical between the original and folded forms.
    let mut char_byte: Vec<usize> = haystack.char_indices().map(|(b, _)| b).collect();
    char_byte.push(haystack.len());

    let mut ranges = Vec::new();
    let mut i = 0;
    while i + ndl.len() <= hay.len() {
        if hay[i..i + ndl.len()] == ndl[..] {
            ranges.push((char_byte[i], char_byte[i + ndl.len()]));
            i += ndl.len(); // non-overlapping, matching `str::match_indices`
        } else {
            i += 1;
        }
    }
    ranges
}

/// Explain why `needle` could not be located, with a fix the model can act on.
fn diagnose(content: &str, needle: &str) -> String {
    if let Some(stripped) = strip_line_number_prefixes(needle) {
        if !stripped.is_empty() && content.contains(&stripped) {
            return "old_string was not found — it appears to include the line-number \
                    prefixes shown by file_read (e.g. \"   42\\t\"). Pass the raw file \
                    text only, without those prefixes."
                .to_string();
        }
    }

    if let Some((first, last)) = whitespace_near_match(content, needle) {
        return format!(
            "old_string was not found exactly. A block differing only in whitespace or \
             indentation exists at lines {first}-{last}. Re-copy the exact text from the \
             file, preserving its leading whitespace."
        );
    }

    "old_string was not found in the file. It must match the file contents exactly, \
     including whitespace and indentation."
        .to_string()
}

/// If a unique run of file lines matches `needle` line-for-line *after* trimming
/// and folding, return that run as a 1-based inclusive line range. Used purely
/// for diagnostics — the edit itself is never applied on this basis.
fn whitespace_near_match(content: &str, needle: &str) -> Option<(usize, usize)> {
    let content_lines: Vec<&str> = content.lines().collect();
    let needle_lines: Vec<&str> = needle.lines().collect();
    let n = needle_lines.len();
    if n == 0 || n > content_lines.len() {
        return None;
    }

    let needle_norm: Vec<String> = needle_lines.iter().map(|l| normalize_line(l)).collect();
    let mut first_hit = None;
    for start in 0..=content_lines.len() - n {
        let matches = (0..n).all(|j| normalize_line(content_lines[start + j]) == needle_norm[j]);
        if matches {
            if first_hit.is_some() {
                return None; // ambiguous — not a useful diagnostic
            }
            first_hit = Some(start);
        }
    }
    first_hit.map(|s| (s + 1, s + n))
}

/// Strip a leading `<spaces><digits>\t` prefix from every line, as emitted by
/// `file_read`. Returns `None` unless *every* line carries such a prefix — so a
/// model that passed raw file text is never second-guessed.
fn strip_line_number_prefixes(s: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for line in s.split('\n') {
        out.push(strip_one_line_number(line)?);
    }
    Some(out.join("\n"))
}

fn strip_one_line_number(line: &str) -> Option<&str> {
    let after_spaces = line.trim_start_matches(' ');
    let digit_len = after_spaces.bytes().take_while(u8::is_ascii_digit).count();
    if digit_len == 0 {
        return None;
    }
    after_spaces[digit_len..].strip_prefix('\t')
}

/// Trim both ends and fold typographic punctuation — the comparison key for the
/// whitespace-drift diagnostic.
fn normalize_line(line: &str) -> String {
    line.trim().chars().map(fold_char).collect()
}

/// Fold a single typographic-punctuation code point to its ASCII equivalent.
///
/// Ported from codex's `apply-patch` `seek_sequence::normalise`. The mapping is
/// strictly 1 char → 1 char so callers can map folded match positions back to
/// original byte offsets.
fn fold_char(c: char) -> char {
    match c {
        // Dash / hyphen code points → ASCII '-'
        '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
        | '\u{2212}' => '-',
        // Curly single quotes → '\''
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
        // Curly double quotes → '"'
        '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
        // Non-breaking and other exotic spaces → ASCII space
        '\u{00A0}' | '\u{2002}' | '\u{2003}' | '\u{2004}' | '\u{2005}' | '\u{2006}'
        | '\u{2007}' | '\u{2008}' | '\u{2009}' | '\u{200A}' | '\u{202F}' | '\u{205F}'
        | '\u{3000}' => ' ',
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match_returns_byte_ranges() {
        match locate("foo bar foo", "foo") {
            LocateResult::Exact(ranges) => assert_eq!(ranges, vec![(0, 3), (8, 11)]),
            _ => panic!("expected exact match"),
        }
    }

    #[test]
    fn folded_match_bridges_typographic_dash() {
        // File has an em-dash (U+2014); the model typed an ASCII hyphen.
        let content = "let x = a \u{2014} b;";
        match locate(content, "a - b") {
            LocateResult::Folded(ranges) => {
                assert_eq!(ranges.len(), 1);
                let (s, e) = ranges[0];
                // Range must land on valid char boundaries of the original.
                assert_eq!(&content[s..e], "a \u{2014} b");
            }
            other => panic!(
                "expected folded match, got {}",
                match other {
                    LocateResult::Exact(_) => "exact",
                    LocateResult::NotFound(_) => "not found",
                    LocateResult::Folded(_) => unreachable!(),
                }
            ),
        }
    }

    #[test]
    fn apply_ranges_splices_back_to_front() {
        let content = "foo bar foo";
        let ranges = vec![(0, 3), (8, 11)];
        assert_eq!(apply_ranges(content, &ranges, "X"), "X bar X");
        // Single-range slice replaces only the first occurrence.
        assert_eq!(apply_ranges(content, &ranges[..1], "X"), "X bar foo");
    }

    #[test]
    fn folded_apply_produces_valid_utf8() {
        let content = "a \u{2014} b";
        if let LocateResult::Folded(ranges) = locate(content, "a - b") {
            assert_eq!(apply_ranges(content, &ranges, "a + b"), "a + b");
        } else {
            panic!("expected folded match");
        }
    }

    #[test]
    fn no_match_reports_whitespace_drift() {
        let content = "fn main() {\n    let x = 1;\n}\n";
        // Same line, but the model over-indented it — so it is not even a
        // substring of the (less-indented) file.
        match locate(content, "            let x = 1;") {
            LocateResult::NotFound(msg) => {
                assert!(msg.contains("whitespace"), "msg was: {msg}");
                assert!(msg.contains("line"), "msg was: {msg}");
            }
            _ => panic!("expected not-found with whitespace diagnostic"),
        }
    }

    #[test]
    fn no_match_reports_line_number_prefix() {
        let content = "alpha\nbeta\ngamma\n";
        let needle = "    1\talpha\n    2\tbeta";
        match locate(content, needle) {
            LocateResult::NotFound(msg) => {
                assert!(msg.contains("line-number"), "msg was: {msg}");
            }
            _ => panic!("expected not-found with line-number diagnostic"),
        }
    }

    #[test]
    fn unrelated_miss_gets_generic_diagnostic() {
        match locate("hello world", "totally absent") {
            LocateResult::NotFound(msg) => {
                assert!(
                    msg.contains("match the file contents exactly"),
                    "msg was: {msg}"
                );
                // The generic message must not pinpoint a line range — that is
                // reserved for the whitespace-drift diagnostic.
                assert!(!msg.contains("at lines"), "msg was: {msg}");
            }
            _ => panic!("expected generic not-found"),
        }
    }

    #[test]
    fn folding_only_bridges_typographic_drift() {
        // No typographic chars and a genuinely absent needle → folding cannot
        // conjure a match.
        assert!(folded_match_ranges("plain ascii content", "missing text").is_empty());
        // A verbatim needle is still found (the fold is the identity here).
        assert_eq!(folded_match_ranges("plain ascii", "ascii"), vec![(6, 11)]);
    }
}
