//! Semantic distillation of command / log tool output.
//!
//! Large command output (cargo builds, test runs, CI logs) usually buries its
//! signal — compile errors, panics, failing assertions, `file:line:col`
//! references — in the **middle** of the stream: compile-progress noise at the
//! head, a summary at the tail, the real errors in between. Head+tail
//! truncation (the previous fallback in [`crate::tools::result_processing`])
//! drops exactly that middle.
//!
//! This module extracts the salient lines locally so the model sees "only the
//! key errors, paths, context" without a re-read round-trip. It is pure and
//! dependency-free (no `regex`, no allocations beyond the kept lines): ANSI
//! escapes are stripped, consecutive duplicate noise is collapsed, error /
//! panic / path lines are kept in original order, and `file:line` references
//! are surfaced as a trailing index.
//!
//! Returns [`None`] when the text carries no salient signal, so the caller
//! falls back to existing truncation — non-breaking by construction.

use std::borrow::Cow;

/// Case-insensitive substring markers that flag an error / failure line.
/// Ordered cheapest-first is irrelevant (we scan a lowercased copy once).
const ERROR_MARKERS: &[&str] = &[
    "error[",
    "error:",
    "error ",
    "panicked",
    "panic:",
    "fatal:",
    "fatal error",
    "failed",
    "failure",
    " fail ",
    "assertion",
    "exception",
    "traceback",
    "unhandled",
    "segmentation fault",
    "stack overflow",
    "cannot find",
    "not found",
    "undefined reference",
    "unresolved",
    "exit code",
    "exit status",
];

/// Markers that flag a secondary diagnostic line worth keeping for context
/// (compiler `-->` source pointers, `note:` / `help:` follow-ups, warnings).
const CONTEXT_MARKERS: &[&str] = &["-->", "note:", "help:", "warning:", "expected", "found:"];

/// Hard cap on salient lines retained — a digest is meant to orient, not
/// reproduce the log. `pub(crate)` because it is also the *default* the caller
/// scales down from when it has a token budget: see
/// [`OutputDigest::render`]'s `max_salient` and
/// [`scale_to_budget`](super::scale_to_budget).
pub(crate) const MAX_SALIENT_LINES: usize = 60;

/// Hard cap on unique `file:line` references surfaced in the trailing index.
const MAX_PATHS: usize = 20;

/// Char cap per retained line — a single 4 000-char minified-JS error line
/// would otherwise blow the budget on its own.
const MAX_LINE_CHARS: usize = 400;

/// Minimum input size (bytes) below which distillation is pointless — small
/// output is already cheap to show verbatim.
const MIN_INPUT_BYTES: usize = 2 * 1024;

/// A distilled view of command output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputDigest {
    /// Total line count of the original (post-ANSI-strip) text.
    pub total_lines: usize,
    /// Number of lines classified as errors / failures.
    pub error_count: usize,
    /// Salient lines (errors + context), ANSI-stripped, de-noised, in original
    /// order, each capped at [`MAX_LINE_CHARS`].
    pub salient: Vec<String>,
    /// Unique `file:line(:col)` references discovered, in first-seen order.
    pub paths: Vec<String>,
}

/// Strip ANSI / VT100 escape sequences (CSI `ESC [ … final`, plus the common
/// OSC `ESC ] … BEL/ST` form) from a line. Returns [`Cow::Borrowed`] when the
/// line contains no escape byte, so the common (clean) case never allocates.
pub(crate) fn strip_ansi(line: &str) -> Cow<'_, str> {
    if !line.as_bytes().contains(&0x1b) {
        return Cow::Borrowed(line);
    }
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            // CSI: ESC [ … <final byte 0x40..=0x7e>
            Some('[') => {
                chars.next();
                for inner in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&inner) {
                        break;
                    }
                }
            }
            // OSC: ESC ] … (terminated by BEL or ST `ESC \`)
            Some(']') => {
                chars.next();
                while let Some(&inner) = chars.peek() {
                    if inner == '\u{07}' {
                        chars.next();
                        break;
                    }
                    if inner == '\u{1b}' {
                        chars.next();
                        if chars.peek() == Some(&'\\') {
                            chars.next();
                        }
                        break;
                    }
                    chars.next();
                }
            }
            // Other escapes (e.g. `ESC c` reset) — drop ESC + the next byte.
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    Cow::Owned(out)
}

/// Whether a (lowercased) line looks like an error / failure line.
fn is_error_line(lower: &str) -> bool {
    ERROR_MARKERS.iter().any(|m| lower.contains(m))
}

/// Whether a (lowercased) line is a secondary diagnostic worth keeping.
fn is_context_line(lower: &str) -> bool {
    CONTEXT_MARKERS.iter().any(|m| lower.contains(m))
}

/// Extract a `file:line(:col)` reference from a line, if present. Dependency-
/// free: scans whitespace tokens for `<path-with-dot>:<digits>[:<digits>]`,
/// trimming common surrounding punctuation (`-->`, parens, quotes, commas).
fn extract_path(line: &str) -> Option<String> {
    for raw in line.split_whitespace() {
        let tok =
            raw.trim_matches(|c: char| matches!(c, '(' | ')' | '\'' | '"' | ',' | '`' | '[' | ']'));
        // A reference may carry more than one colon: a Windows drive prefix
        // (`C:\…`) puts a colon *before* the path, and a `:col` suffix puts one
        // after the line number. Try each colon as the path/line separator and
        // accept the first that yields a path-ish left side followed by a line
        // number. The first qualifying colon is taken, so Unix `path:line:col`
        // behaves exactly as before (its first colon already qualifies).
        for (colon, _) in tok.match_indices(':') {
            let path_part = &tok[..colon];
            if path_part.is_empty() {
                continue;
            }
            // Accept paths that have an extension dot, a Unix slash, or a Windows backslash.
            if !path_part.contains('.') && !path_part.contains('/') && !path_part.contains('\\') {
                continue;
            }
            let rest = &tok[colon + 1..]; // drop the ':'
                                          // rest must start with digits (the line number).
            let line_digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if line_digits.is_empty() {
                continue;
            }
            // Optional `:col`.
            let after = &rest[line_digits.len()..];
            let col = after.strip_prefix(':').map(|c| {
                c.chars()
                    .take_while(|ch| ch.is_ascii_digit())
                    .collect::<String>()
            });
            return Some(match col {
                Some(col) if !col.is_empty() => format!("{path_part}:{line_digits}:{col}"),
                _ => format!("{path_part}:{line_digits}"),
            });
        }
    }
    None
}

/// Char-safe cap (P7 UTF-8 safety): never slice on a byte boundary.
fn cap_chars(s: &str) -> String {
    if s.chars().count() <= MAX_LINE_CHARS {
        return s.to_string();
    }
    let kept: String = s.chars().take(MAX_LINE_CHARS).collect();
    format!("{kept}…")
}

/// Distill command output into a salient digest, or [`None`] when there is no
/// error / failure / path signal worth surfacing (caller should then fall back
/// to verbatim or head+tail truncation).
///
/// Small inputs (`< MIN_INPUT_BYTES`) always return [`None`]: they are cheap to
/// show in full and distillation would only lose context.
///
/// A payload with **no newline at all** also returns [`None`], and that is a
/// precondition rather than an optimisation. This distiller is line-oriented:
/// it walks `text.lines()`, classifies each line, and renders the salient ones.
/// Handed a single line it reports `total_lines: 1`, matches an `"error"`
/// substring somewhere inside it, and renders `[Output digest: 1 lines, 1
/// error]` above a 400-char *prefix* of that line — a guess dressed up as a
/// signal. The shape is not hypothetical: a flattened tool envelope and a
/// compact JSON API response are both exactly one line, and one of the two
/// callers that knew this ([`inline_error_digest`](
/// crate::tools::result_processing)) declined by hand while the other
/// ([`hygiene::clean_result_value`](crate::tool_output::hygiene)) replaced a
/// 300 KB response with 400 characters of its envelope. A predicate that both
/// faces of the same distiller must honour belongs on the distiller.
pub fn distill_output(text: &str) -> Option<OutputDigest> {
    if text.len() < MIN_INPUT_BYTES || !text.contains('\n') {
        return None;
    }

    let mut total_lines = 0usize;
    let mut error_count = 0usize;
    let mut salient: Vec<String> = Vec::new();
    let mut paths: Vec<String> = Vec::new();

    // Collapse consecutive duplicate (post-strip) lines — progress bars and
    // repeated dots otherwise dominate. Track only the previous stripped line.
    let mut prev_stripped: Option<String> = None;

    for raw_line in text.lines() {
        total_lines += 1;
        let stripped = strip_ansi(raw_line);
        let stripped = stripped.trim_end();
        if stripped.is_empty() {
            continue;
        }

        // Duplicate collapse.
        if prev_stripped.as_deref() == Some(stripped) {
            continue;
        }
        prev_stripped = Some(stripped.to_string());

        let lower = stripped.to_ascii_lowercase();
        let err = is_error_line(&lower);
        let ctx = is_context_line(&lower);

        if let Some(p) = extract_path(stripped) {
            if !paths.contains(&p) && paths.len() < MAX_PATHS {
                paths.push(p);
            }
        }

        if err {
            error_count += 1;
        }
        if (err || ctx) && salient.len() < MAX_SALIENT_LINES {
            salient.push(cap_chars(stripped));
        }
    }

    // No signal → let the caller truncate instead.
    if error_count == 0 && paths.is_empty() {
        return None;
    }

    Some(OutputDigest {
        total_lines,
        error_count,
        salient,
        paths,
    })
}

impl OutputDigest {
    /// Render a compact, model-facing digest block. `max_salient` caps how many
    /// salient lines are emitted (the caller can shrink this to honour a token
    /// budget); paths are always summarised on a trailing line.
    pub fn render(&self, max_salient: usize) -> String {
        let mut out = String::new();
        let err_word = if self.error_count == 1 {
            "error"
        } else {
            "errors"
        };
        out.push_str(&format!(
            "[Output digest: {} lines, {} {} — full output truncated]\n",
            self.total_lines, self.error_count, err_word
        ));

        let shown = self.salient.len().min(max_salient);
        for line in self.salient.iter().take(shown) {
            out.push_str(line);
            out.push('\n');
        }
        if self.salient.len() > shown {
            out.push_str(&format!(
                "[... {} more diagnostic lines omitted]\n",
                self.salient.len() - shown
            ));
        }

        if !self.paths.is_empty() {
            out.push_str(&format!("[Files: {}]", self.paths.join(", ")));
        } else {
            // Trim the trailing newline left by the last salient line.
            while out.ends_with('\n') {
                out.pop();
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_removes_csi_color_codes() {
        let input = "\u{1b}[31merror\u{1b}[0m: boom";
        assert_eq!(strip_ansi(input), "error: boom");
    }

    #[test]
    fn strip_ansi_borrows_when_clean() {
        let input = "plain line, no escapes";
        assert!(matches!(strip_ansi(input), Cow::Borrowed(_)));
    }

    #[test]
    fn strip_ansi_removes_osc_sequence() {
        // OSC set-title terminated by BEL.
        let input = "\u{1b}]0;my title\u{07}done";
        assert_eq!(strip_ansi(input), "done");
    }

    #[test]
    fn extract_path_rust_style() {
        assert_eq!(
            extract_path("  --> src/main.rs:42:9"),
            Some("src/main.rs:42:9".to_string())
        );
    }

    #[test]
    fn extract_path_line_only() {
        assert_eq!(
            extract_path("at lib/foo.py:88 in handler"),
            Some("lib/foo.py:88".to_string())
        );
    }

    #[test]
    fn extract_path_windows_drive_letter() {
        // The drive-letter colon must not be mistaken for the path/line
        // separator — the first qualifying colon is the one before `42`.
        assert_eq!(
            extract_path("  --> C:\\proj\\src\\main.rs:42:9"),
            Some("C:\\proj\\src\\main.rs:42:9".to_string())
        );
        assert_eq!(
            extract_path("at C:\\app\\lib.rs:88"),
            Some("C:\\app\\lib.rs:88".to_string())
        );
    }

    #[test]
    fn extract_path_rejects_non_path() {
        assert_eq!(extract_path("time: 12:30"), None); // no dot in "time"
        assert_eq!(extract_path("just words here"), None);
    }

    #[test]
    fn small_input_returns_none() {
        let small = "error: boom\n--> src/x.rs:1:1";
        assert!(distill_output(small).is_none());
    }

    /// A single-line payload cannot be line-distilled. Before this precondition
    /// lived on the distiller, a compact JSON API response containing the word
    /// "error" anywhere reached the model as `[Output digest: 1 lines, 1 error]`
    /// above 400 characters of its own envelope — a prefix slice presented as an
    /// error preview, and a 99.9 % silent loss of the actual response.
    #[test]
    fn a_payload_with_no_newline_is_never_distilled() {
        let one_line = format!(
            r#"{{"error":null,"status":"ok","data":"{}"}}"#,
            "d".repeat(8_000)
        );
        assert!(one_line.len() > MIN_INPUT_BYTES, "precondition: big enough");
        assert!(
            distill_output(&one_line).is_none(),
            "a line-oriented distiller has nothing to say about one line"
        );
    }

    #[test]
    fn distills_errors_from_middle_of_large_output() {
        // Head noise + middle error + tail summary, > MIN_INPUT_BYTES.
        let mut s = String::new();
        for i in 0..400 {
            s.push_str(&format!("   Compiling crate_{i} v0.1.0\n"));
        }
        s.push_str("error[E0382]: borrow of moved value: `x`\n");
        s.push_str("  --> src/main.rs:42:9\n");
        s.push_str("thread 'tests::foo' panicked at src/lib.rs:88:5\n");
        for i in 0..400 {
            s.push_str(&format!("   Finished step_{i}\n"));
        }
        assert!(s.len() > MIN_INPUT_BYTES);

        let digest = distill_output(&s).expect("should distill");
        assert_eq!(digest.error_count, 2);
        assert!(digest.salient.iter().any(|l| l.contains("E0382")));
        assert!(digest.salient.iter().any(|l| l.contains("panicked")));
        assert!(digest.paths.contains(&"src/main.rs:42:9".to_string()));
        assert!(digest.paths.contains(&"src/lib.rs:88:5".to_string()));

        let rendered = digest.render(20);
        assert!(rendered.contains("2 errors"));
        assert!(rendered.contains("E0382"));
        assert!(rendered.contains("[Files: "));
        // The head/tail noise must be gone.
        assert!(!rendered.contains("Compiling crate_0"));
        assert!(!rendered.contains("Finished step_0"));
    }

    #[test]
    fn collapses_consecutive_duplicates() {
        let mut s = String::new();
        s.push_str("error: real problem at app.rs:5\n");
        for _ in 0..500 {
            s.push_str("....\n"); // repeated noise
        }
        assert!(s.len() > MIN_INPUT_BYTES);
        let digest = distill_output(&s).expect("has error");
        // The noise line is not an error/context line, so it never enters
        // salient regardless; assert the error survived and salient is tiny.
        assert_eq!(digest.error_count, 1);
        assert!(digest.salient.len() <= 2);
    }

    #[test]
    fn no_signal_returns_none() {
        // Large but signal-free output: caller should truncate instead.
        let s = "lorem ipsum dolor sit amet ".repeat(200);
        assert!(s.len() > MIN_INPUT_BYTES);
        assert!(distill_output(&s).is_none());
    }

    #[test]
    fn render_caps_salient_lines() {
        let mut s = String::new();
        for i in 0..100 {
            s.push_str(&format!("error: failure number {i} at f{i}.rs:1\n"));
        }
        assert!(s.len() > MIN_INPUT_BYTES);
        let digest = distill_output(&s).unwrap();
        let rendered = digest.render(5);
        assert!(rendered.contains("more diagnostic lines omitted"));
    }

    #[test]
    fn long_line_is_char_capped() {
        let mut s = String::new();
        s.push_str(&format!("error: {}\n", "x".repeat(5000)));
        s.push_str("padding line to exceed min input size ".repeat(60).as_str());
        let digest = distill_output(&s).unwrap();
        let err_line = digest
            .salient
            .iter()
            .find(|l| l.contains("error:"))
            .unwrap();
        assert!(err_line.chars().count() <= MAX_LINE_CHARS + 1); // +1 for the ellipsis
        assert!(err_line.ends_with('…'));
    }
}
