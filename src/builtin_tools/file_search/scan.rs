//! Line matching for [`grep`](super::grep) — pure, no filesystem, no runtime.
//!
//! Kept separate from the tool so the part that decides *what a match looks
//! like* can be tested against strings instead of against a temp directory.
//! The tool above supplies bytes and concurrency; everything below is a
//! function of `(text, regex, options)`.

use regex::Regex;

use crate::builtin_tools::file_ops::clamp_line_to;

/// Characters kept from one matched line.
///
/// Deliberately far below `file_read`'s 2 000: a grep line is a **locator**,
/// not the content. The model reads the neighbourhood with
/// `file_read{offset,limit}` once it knows where to look, so keeping a whole
/// minified bundle line here would spend thousands of tokens to say "it is on
/// line 1".
pub(super) const MATCH_LINE_CHARS: usize = 240;

/// How one file was scanned.
pub(super) struct ScanOptions {
    /// Lines of context rendered either side of each match.
    pub context: usize,
    /// Rendered-match ceiling for a single file, so one generated file cannot
    /// take the whole page. `total` below still counts every match.
    pub max_per_file: usize,
}

/// One rendered match: the block of lines a reader sees.
///
/// There is deliberately no `line_no` field beside `block`. The match's line
/// number is already in the rendered text (`path:N:`), which is the form a
/// follow-up `file_read{offset}` reads it from — a second copy would be a
/// field with no consumer.
pub(super) struct RenderedMatch {
    /// Rendered lines *without* the file path — the caller prefixes it, since
    /// only the caller knows how the path should be spelled relative to root.
    pub block: Vec<RenderedLine>,
}

pub(super) struct RenderedLine {
    pub line_no: usize,
    pub text: String,
    /// `true` for a context line (rendered with `-`), `false` for the match
    /// line itself (rendered with `:`) — the same convention `grep -C` uses.
    pub is_context: bool,
}

/// Result of scanning one file's text.
pub(super) struct FileScan {
    /// Every match in the file, whether or not it was rendered.
    pub total: usize,
    /// At most `max_per_file` entries.
    pub rendered: Vec<RenderedMatch>,
}

impl FileScan {
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }
}

/// Scan `text` for `re`, rendering at most `opts.max_per_file` matches.
///
/// `total` is exact regardless of the render cap: a count is cheap and a count
/// that silently equalled the cap would tell the model the file was fully
/// covered when it was not.
pub(super) fn scan_text(text: &str, re: &Regex, opts: &ScanOptions) -> FileScan {
    let lines: Vec<&str> = text.lines().collect();
    let mut total = 0usize;
    let mut rendered = Vec::new();

    for (idx, line) in lines.iter().enumerate() {
        if !re.is_match(line) {
            continue;
        }
        total += 1;
        if rendered.len() >= opts.max_per_file {
            continue;
        }
        let line_no = idx + 1;
        let start = line_no.saturating_sub(opts.context).max(1);
        let end = (line_no + opts.context).min(lines.len());
        let block = (start..=end)
            .map(|n| RenderedLine {
                line_no: n,
                text: clamp_line_to(lines[n - 1].trim_end_matches('\r'), MATCH_LINE_CHARS),
                is_context: n != line_no,
            })
            .collect();
        rendered.push(RenderedMatch { block });
    }

    FileScan { total, rendered }
}

/// Render one match as the `path:line: text` / `path-line- text` lines a reader
/// (and every downstream grep-shaped reducer) already understands.
pub(super) fn render(path: &str, m: &RenderedMatch) -> Vec<String> {
    m.block
        .iter()
        .map(|l| {
            let sep = if l.is_context { '-' } else { ':' };
            format!("{path}{sep}{}{sep} {}", l.line_no, l.text)
        })
        .collect()
}

/// Build the regex a scan runs.
///
/// `literal` escapes the pattern rather than switching engines, so one code
/// path serves both modes and a literal search cannot behave differently from
/// its escaped-regex equivalent.
pub(super) fn build_regex(
    pattern: &str,
    literal: bool,
    ignore_case: bool,
) -> Result<Regex, regex::Error> {
    let source = if literal {
        regex::escape(pattern)
    } else {
        pattern.to_string()
    };
    regex::RegexBuilder::new(&source)
        .case_insensitive(ignore_case)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(context: usize, max_per_file: usize) -> ScanOptions {
        ScanOptions {
            context,
            max_per_file,
        }
    }

    const SAMPLE: &str = "alpha\nbeta\ngamma\nbeta again\ndelta\n";

    #[test]
    fn counts_every_match_and_renders_the_line() {
        let re = build_regex("beta", false, false).unwrap();
        let scan = scan_text(SAMPLE, &re, &opts(0, 10));
        assert_eq!(scan.total, 2);
        assert_eq!(scan.rendered.len(), 2);
        assert_eq!(render("f.txt", &scan.rendered[0]), vec!["f.txt:2: beta"]);
        assert_eq!(
            render("f.txt", &scan.rendered[1]),
            vec!["f.txt:4: beta again"]
        );
    }

    #[test]
    fn context_lines_use_the_dash_convention() {
        let re = build_regex("gamma", false, false).unwrap();
        let scan = scan_text(SAMPLE, &re, &opts(1, 10));
        assert_eq!(
            render("f.txt", &scan.rendered[0]),
            vec!["f.txt-2- beta", "f.txt:3: gamma", "f.txt-4- beta again"]
        );
    }

    #[test]
    fn context_clamps_at_both_file_edges() {
        let re = build_regex("alpha|delta", false, false).unwrap();
        let scan = scan_text(SAMPLE, &re, &opts(5, 10));
        // Both windows clamp to the whole 5-line file; only which line is the
        // match differs, and that is exactly what the `:` / `-` split encodes.
        let first = render("f.txt", &scan.rendered[0]);
        assert!(first[0].starts_with("f.txt:1:"), "{first:?}");
        assert!(first.last().unwrap().starts_with("f.txt-5-"), "{first:?}");
        let last = render("f.txt", &scan.rendered[1]);
        assert!(last[0].starts_with("f.txt-1-"), "{last:?}");
        assert!(last.last().unwrap().starts_with("f.txt:5:"), "{last:?}");
    }

    /// The per-file cap bounds what is *rendered*, never what is *counted* —
    /// a total that quietly equalled the cap would report full coverage of a
    /// file the scan only sampled.
    #[test]
    fn per_file_cap_bounds_rendering_but_not_the_count() {
        let text = "hit\n".repeat(50);
        let re = build_regex("hit", false, false).unwrap();
        let scan = scan_text(&text, &re, &opts(0, 3));
        assert_eq!(scan.total, 50);
        assert_eq!(scan.rendered.len(), 3);
    }

    #[test]
    fn literal_mode_escapes_regex_metacharacters() {
        let text = "a.c\nabc\n";
        let literal = build_regex("a.c", true, false).unwrap();
        assert_eq!(scan_text(text, &literal, &opts(0, 10)).total, 1);
        let as_regex = build_regex("a.c", false, false).unwrap();
        assert_eq!(scan_text(text, &as_regex, &opts(0, 10)).total, 2);
    }

    #[test]
    fn ignore_case_is_honoured() {
        let re = build_regex("BETA", false, true).unwrap();
        assert_eq!(scan_text(SAMPLE, &re, &opts(0, 10)).total, 2);
    }

    /// Alternation is how one call answers "any of these N symbols" — the
    /// reason this tool has no separate multi-pattern verb.
    #[test]
    fn alternation_is_one_pass_over_the_text() {
        let re = build_regex("alpha|delta|nothing", false, false).unwrap();
        let scan = scan_text(SAMPLE, &re, &opts(0, 10));
        assert_eq!(scan.total, 2);
    }

    #[test]
    fn a_minified_line_is_clamped_to_a_locator() {
        let text = format!("{}needle{}\n", "x".repeat(5000), "y".repeat(5000));
        let re = build_regex("needle", false, false).unwrap();
        let scan = scan_text(&text, &re, &opts(0, 10));
        let line = &render("min.js", &scan.rendered[0])[0];
        assert!(line.len() < 600, "line was {} bytes", line.len());
        assert!(line.contains("line truncated"), "{line}");
    }

    #[test]
    fn crlf_text_does_not_leak_carriage_returns() {
        let re = build_regex("beta", false, false).unwrap();
        let scan = scan_text("alpha\r\nbeta\r\n", &re, &opts(0, 10));
        assert_eq!(render("f.txt", &scan.rendered[0]), vec!["f.txt:2: beta"]);
    }

    #[test]
    fn an_invalid_regex_is_an_error_not_a_silent_zero() {
        assert!(build_regex("a(", false, false).is_err());
    }
}
