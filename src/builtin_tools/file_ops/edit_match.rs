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
    /// Occurrences found only after expanding the needle's LF line endings to
    /// the file's CRLF. Callers splicing a replacement must convert its `\n`
    /// to `\r\n` too, or the edit writes mixed line endings into the file.
    Crlf(Vec<(usize, usize)>),
    /// Not found — carries an actionable diagnostic for the model.
    NotFound(String),
}

/// Locate every occurrence of `needle` in `content`.
///
/// Pass order: exact → CRLF-expanded exact → typographic fold (raw, then
/// CRLF-expanded) → diagnostic.
///
/// The CRLF pass exists because `file_read` renders via `str::lines()`, which
/// strips `\r` — so on a CRLF file the model *cannot* see or reproduce the
/// `\r`, and every multi-line `old_string` it copies misses the exact pass.
/// Worse, the whitespace-drift diagnostic then tells it to "re-copy the exact
/// text", which can never help. Expanding the needle's `\n` to `\r\n` is a
/// 1 char → 2 char rewrite of the *needle only*, so match offsets land
/// directly in the original content with no mapping step.
pub(super) fn locate(content: &str, needle: &str) -> LocateResult {
    let exact: Vec<(usize, usize)> = content
        .match_indices(needle)
        .map(|(i, m)| (i, i + m.len()))
        .collect();
    if !exact.is_empty() {
        return LocateResult::Exact(exact);
    }

    // Only a multi-line LF needle against a file that actually uses CRLF can
    // benefit from the expansion; anything else would re-run the exact pass.
    let crlf_needle = (needle.contains('\n') && !needle.contains('\r') && content.contains("\r\n"))
        .then(|| needle.replace('\n', "\r\n"));
    if let Some(ref expanded) = crlf_needle {
        let crlf: Vec<(usize, usize)> = content
            .match_indices(expanded.as_str())
            .map(|(i, m)| (i, i + m.len()))
            .collect();
        if !crlf.is_empty() {
            return LocateResult::Crlf(crlf);
        }
    }

    if content.len() <= FUZZY_MAX_BYTES {
        let folded = folded_match_ranges(content, needle);
        if !folded.is_empty() {
            return LocateResult::Folded(folded);
        }
        // Both drifts at once: CRLF file *and* typographic punctuation. Fold
        // the CRLF-expanded needle so neither miss masks the other. Reported
        // as Crlf because the replacement still needs its newlines converted.
        if let Some(ref expanded) = crlf_needle {
            let folded_crlf = folded_match_ranges(content, expanded);
            if !folded_crlf.is_empty() {
                return LocateResult::Crlf(folded_crlf);
            }
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
        if hay.get(i..i + ndl.len()) == Some(ndl.as_slice()) {
            ranges.push((
                *char_byte
                    .get(i)
                    .expect("invariant: i is within the folded haystack length"),
                *char_byte
                    .get(i + ndl.len())
                    .expect("invariant: i + ndl.len() is within the char byte map"),
            ));
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
        let matches = (0..n).all(|j| {
            let content_line = content_lines
                .get(start + j)
                .expect("invariant: start + j is within content_lines");
            let needle_line = needle_norm
                .get(j)
                .expect("invariant: j is within needle_norm");
            normalize_line(content_line) == *needle_line
        });
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

// =============================================================================
// Line-anchored fuzzy locator (apply_patch only)
// =============================================================================

/// Line-anchored fuzzy locator for `apply_patch` hunks.
///
/// Ported from codex's `apply-patch` `seek_sequence`. When the contiguous
/// substring search in [`locate`] fails to anchor a hunk, this performs a
/// *line-oriented* search with progressively looser per-line equality —
/// exact → trailing-whitespace-insensitive (`rstrip`) → both-ends trimmed →
/// typographically folded. This bridges the single most common `apply_patch`
/// failure mode: a context line whose trailing whitespace drifted between the
/// model's patch and the file on disk, which defeats the substring matcher
/// entirely (one stray `\n`-adjacent space breaks the whole multi-line block).
///
/// `eof` mirrors codex: when the hunk carried a `*** End of File` anchor, the
/// search starts at the last possible window so a pattern meant to match the
/// file's tail is applied there.
///
/// Returns a byte range over the *original* `content` spanning the matched
/// block — first line start .. last line end (excluding the trailing newline) —
/// matching the splice convention of [`apply_ranges`] so surrounding newlines
/// are preserved. Returns `None` when no pass matches.
///
/// Deliberately scoped to `apply_patch`: `file_edit` keeps its strict
/// exact/diagnostic model (a clean failure beats a wrong-whitespace edit), so
/// it does not call this.
pub(super) fn locate_lines(content: &str, needle: &str, eof: bool) -> Option<(usize, usize)> {
    let spans = line_spans(content);
    let pattern: Vec<&str> = needle.split('\n').collect();
    // A pattern longer than the file can never match; an empty `needle` is
    // never passed here (pure-add hunks are skipped upstream).
    if pattern.is_empty() || pattern.len() > spans.len() {
        return None;
    }

    // A file ending in '\n' yields a trailing empty span (a `split('\n')`
    // artifact, not a real line). Exclude it when computing the EOF anchor so
    // the search starts at the last *content* line rather than the phantom
    // empty one; the scan's upper bound keeps the empty span so a pattern that
    // legitimately ends in a blank line can still match. `saturating_sub`
    // guards the case where the pattern spans the whole effective file.
    let effective_len = if content.ends_with('\n') {
        spans.len() - 1
    } else {
        spans.len()
    };
    let search_start = if eof {
        effective_len.saturating_sub(pattern.len())
    } else {
        0
    };

    // Each pass is a full scan at one strictness level — codex semantics: try
    // the strictest first so an exact location always wins over a fuzzy one.
    let exact = |a: &str, b: &str| a == b;
    let rstrip = |a: &str, b: &str| a.trim_end() == b.trim_end();
    let trim = |a: &str, b: &str| a.trim() == b.trim();
    let fold = |a: &str, b: &str| normalize_line(a) == normalize_line(b);

    [
        scan(&spans, content, &pattern, search_start, exact),
        scan(&spans, content, &pattern, search_start, rstrip),
        scan(&spans, content, &pattern, search_start, trim),
        scan(&spans, content, &pattern, search_start, fold),
    ]
    .into_iter()
    .flatten()
    .next()
    .map(|i| {
        (
            spans
                .get(i)
                .expect("invariant: i returned by scan is within spans")
                .0,
            spans
                .get(i + pattern.len() - 1)
                .expect("invariant: scan guarantees enough trailing spans")
                .1,
        )
    })
}

/// Byte spans `(start, end)` for each line of `content` split on `'\n'`, where
/// `end` excludes the newline. Mirrors `str::split('\n')` exactly (a trailing
/// `'\n'` yields a final empty span), so spans align 1:1 with a needle that was
/// likewise produced by `split('\n')`. A trailing `'\r'` stays inside the slice
/// and folds away in the `rstrip` pass, so CRLF/LF drift is tolerated for free.
fn line_spans(content: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0;
    for (idx, b) in content.bytes().enumerate() {
        if b == b'\n' {
            spans.push((start, idx));
            start = idx + 1;
        }
    }
    spans.push((start, content.len()));
    spans
}

/// Scan `spans` from `search_start` for the first window where every line
/// satisfies `eq` against the corresponding `pattern` line.
fn scan(
    spans: &[(usize, usize)],
    content: &str,
    pattern: &[&str],
    search_start: usize,
    eq: impl Fn(&str, &str) -> bool,
) -> Option<usize> {
    let last = spans.len() - pattern.len();
    (search_start..=last).find(|&i| {
        (0..pattern.len()).all(|j| {
            let (s, e) = *spans
                .get(i + j)
                .expect("invariant: i + j is within spans during scan");
            let pattern_line = pattern
                .get(j)
                .expect("invariant: j is within pattern during scan");
            eq(&content[s..e], pattern_line)
        })
    })
}

/// Fold a single typographic-punctuation code point to its ASCII equivalent.
///
/// Ported from codex's `apply-patch` `seek_sequence::normalise`. The mapping is
/// strictly 1 char → 1 char so callers can map folded match positions back to
/// original byte offsets.
const fn fold_char(c: char) -> char {
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
                    LocateResult::Crlf(_) => "crlf",
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
    fn locate_lines_bridges_trailing_whitespace_drift() {
        // File line carries trailing spaces the model's patch context omitted.
        // The contiguous substring matcher fails (the `\n`-adjacent spaces break
        // the block); the line-anchored rstrip pass recovers the location.
        let content = "fn a() {\n    let x = 1;   \n    y\n}\n";
        let needle = "    let x = 1;\n    y";
        assert!(
            matches!(locate(content, needle), LocateResult::NotFound(_)),
            "substring matcher must miss so the line fallback is exercised"
        );
        let (s, e) = locate_lines(content, needle, false).expect("rstrip pass matches");
        assert_eq!(&content[s..e], "    let x = 1;   \n    y");
    }

    #[test]
    fn locate_lines_bridges_indentation_drift() {
        // Leading-whitespace drift: patch under-indents the context block.
        let content = "        deeply\n        nested\n";
        let needle = "deeply\nnested";
        let (s, e) = locate_lines(content, needle, false).expect("trim pass matches");
        assert_eq!(&content[s..e], "        deeply\n        nested");
    }

    #[test]
    fn locate_lines_eof_anchor_prefers_tail() {
        // Two identical blocks; the EOF anchor must select the last one.
        let content = "marker\nx\nmarker\n";
        let needle = "marker";
        assert_eq!(locate_lines(content, needle, true), Some((9, 15)));
        assert_eq!(&content["marker\nx\n".len()..15], "marker");
        // Without the anchor the first occurrence wins.
        assert_eq!(locate_lines(content, needle, false), Some((0, 6)));
    }

    #[test]
    fn locate_lines_pattern_longer_than_file_is_none() {
        assert_eq!(locate_lines("one line", "a\nb\nc", false), None);
    }

    #[test]
    fn locate_lines_exact_wins_over_fuzzy() {
        // An exact-matching window further down must not be pre-empted by a
        // fuzzy (whitespace-different) window earlier in the file.
        let content = "foo \nfoo\n";
        // Exact "foo" is line 2 (byte 5); line 1 "foo " only matches under rstrip.
        assert_eq!(locate_lines(content, "foo", false), Some((5, 8)));
    }

    #[test]
    fn crlf_pass_bridges_lf_needle_on_crlf_file() {
        // file_read strips '\r', so the model's multi-line needle is LF-only.
        let content = "fn a() {\r\n    let x = 1;\r\n}\r\n";
        match locate(content, "fn a() {\n    let x = 1;") {
            LocateResult::Crlf(ranges) => {
                assert_eq!(ranges.len(), 1);
                let (s, e) = ranges[0];
                assert_eq!(&content[s..e], "fn a() {\r\n    let x = 1;");
            }
            _ => panic!("expected CRLF-expanded match"),
        }
    }

    #[test]
    fn crlf_pass_combines_with_typographic_fold() {
        // CRLF file *and* a curly apostrophe the model typed as ASCII.
        let content = "it\u{2019}s here\r\nnext line\r\n";
        match locate(content, "it's here\nnext line") {
            LocateResult::Crlf(ranges) => {
                assert_eq!(ranges.len(), 1);
                let (s, e) = ranges[0];
                assert_eq!(&content[s..e], "it\u{2019}s here\r\nnext line");
            }
            _ => panic!("expected folded CRLF match"),
        }
    }

    #[test]
    fn crlf_pass_does_not_fire_on_lf_files() {
        // LF file + absent needle must still produce the diagnostic path —
        // the CRLF expansion must never conjure a match where none exists.
        let content = "alpha\nbeta\n";
        assert!(matches!(
            locate(content, "alpha\ngamma"),
            LocateResult::NotFound(_)
        ));
        // Single-line needles never take the CRLF pass (nothing to expand).
        assert!(matches!(locate("a\r\nb\r\n", "a"), LocateResult::Exact(_)));
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
