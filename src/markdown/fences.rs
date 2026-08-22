//! Markdown code fence parsing.
//!
//! Parses code fence spans from text for safe break point detection
//! during block chunking. Ensures we never split inside a code block.

use regex::Regex;
use std::sync::LazyLock;

/// Regex for matching code fence opening/closing lines.
/// Matches: optional indent (0-3 spaces) + fence marker (``` or ~~~) + optional language tag
static FENCE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^( {0,3})(`{3,}|~{3,})(.*)$").expect("hardcoded fence regex must compile")
});

/// A span representing a code fence block in text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FenceSpan {
    /// Byte offset of the opening fence line start.
    pub(crate) start: usize,
    /// Byte offset of the closing fence line start (or text end if unclosed).
    pub(crate) end: usize,
    /// Just the fence marker (e.g., "```" or "~~~~").
    pub(crate) marker: String,
    /// Leading indentation (0-3 spaces).
    pub(crate) indent: String,
    /// Language tag if present (e.g., "rust", "javascript").
    pub(crate) language: Option<String>,
    info: String,
}

/// Internal state for a fence that has been opened but not yet closed.
#[derive(Debug, Clone)]
struct OpenFence {
    start: usize,
    marker: String,
    indent: String,
    language: Option<String>,
    info: String,
}

impl FenceSpan {
    /// Byte offset of the opening fence line start.
    #[must_use]
    pub const fn start(&self) -> usize {
        self.start
    }

    /// Byte offset of the closing fence line start (or text end if unclosed).
    #[must_use]
    pub const fn end(&self) -> usize {
        self.end
    }

    /// The fence marker (e.g., "```" or "~~~~").
    #[must_use]
    pub fn marker(&self) -> &str {
        &self.marker
    }

    /// Leading indentation (0-3 spaces).
    #[must_use]
    pub fn indent(&self) -> &str {
        &self.indent
    }

    /// Language tag if present (e.g., "rust", "javascript").
    #[must_use]
    pub fn language(&self) -> Option<&str> {
        self.language.as_deref()
    }

    /// Check if a byte index falls strictly inside this fence span.
    ///
    /// Returns `true` if `index` is strictly between `start` and `end`
    /// (exclusive on both boundaries). Boundary positions themselves are
    /// considered outside the fence, making them safe split points.
    ///
    /// **Caller contract:** `end` is the byte offset of the *closing fence
    /// line start*, so the closing marker's own bytes (`end..end +
    /// marker.len()`) lie OUTSIDE this span. A caller that picks a break
    /// position from raw byte budgets (rather than newline boundaries) could
    /// therefore land inside the closing marker — fracturing it — or exactly
    /// at `end`, emitting a chunk whose fence is left unclosed. Always gate
    /// such byte-level splits through [`get_fence_split`], which closes and
    /// reopens the fence correctly. Newline-anchored breaks are inherently
    /// safe: the `\n` preceding the closing fence sits at `end - 1`, which is
    /// still inside the span.
    #[must_use]
    pub const fn contains(&self, index: usize) -> bool {
        index > self.start && index < self.end
    }

    /// Get the closing fence line for this span.
    #[must_use]
    pub fn close_line(&self) -> String {
        format!("{}{}", self.indent, self.marker)
    }

    /// Get the reopening fence line (preserves language tag).
    #[must_use]
    pub fn reopen_line(&self) -> String {
        if !self.info.is_empty() {
            format!("{}{}{}", self.indent, self.marker, self.info)
        } else {
            self.close_line()
        }
    }
}

/// Result of attempting to split at a fence boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FenceSplit {
    /// Line to close the fence before the break
    pub close_line: String,
    /// Line to reopen the fence after the break
    pub reopen_line: String,
}

/// Parse all code fence spans from text.
///
/// Scans line-by-line for fence markers, tracking open/close pairs.
/// Unclosed fences extend to end of text.
///
/// **Note:** This parser recognizes `\n` and `\r\n` line endings. Legacy Mac
/// `\r`-only line endings are not supported and will cause the entire text to
/// be treated as a single line.
///
/// # Example
///
/// ```
/// use alephcore::markdown::fences::parse_fence_spans;
///
/// let text = "Hello\n```rust\nfn main() {}\n```\nWorld";
/// let spans = parse_fence_spans(text);
/// assert_eq!(spans.len(), 1);
/// assert_eq!(spans[0].language(), Some("rust"));
/// ```
pub fn parse_fence_spans(text: &str) -> Vec<FenceSpan> {
    let mut spans = Vec::new();
    let mut current_fence: Option<OpenFence> = None;
    let mut offset = 0;

    for line in text.lines() {
        let line_start = offset;
        // lines() strips \n and \r\n, so compute actual line length from source text
        // to correctly advance offset past the line terminator
        let line_end = offset + line.len();

        if let Some(caps) = FENCE_REGEX.captures(line) {
            let indent = caps.get(1).map_or("", |m| m.as_str());
            let marker = caps.get(2).map_or("", |m| m.as_str());
            let info = caps.get(3).map_or("", |m| m.as_str().trim());

            if let Some(open) = current_fence.take() {
                // Check if this closes the current fence.
                // Closing fence must:
                // 1. Use same character type (` or ~)
                // 2. Have marker length >= opening marker length
                // 3. Have no info string (just marker)
                let same_char = marker.chars().next() == open.marker.chars().next();
                let long_enough = marker.len() >= open.marker.len();
                let no_info = info.is_empty();
                let valid_indent = indent.len() <= open.indent.len();

                if same_char && long_enough && no_info && valid_indent {
                    spans.push(FenceSpan {
                        start: open.start,
                        end: line_start,
                        marker: open.marker,
                        indent: open.indent,
                        language: open.language,
                        info: open.info,
                    });
                } else {
                    current_fence = Some(open);
                }
            } else {
                let language = if info.is_empty() {
                    None
                } else {
                    info.split_whitespace().next().map(|s| s.to_string())
                };

                current_fence = Some(OpenFence {
                    start: line_start,
                    marker: marker.to_string(),
                    indent: indent.to_string(),
                    language,
                    info: info.to_string(),
                });
            }
        }

        // `str::lines()` strips `\n`/`\r\n`, so re-check the original bytes
        // to keep `offset` aligned with the source text byte positions.
        let bytes = text.as_bytes();
        offset = if bytes.get(line_end) == Some(&b'\r') && bytes.get(line_end + 1) == Some(&b'\n') {
            line_end + 2
        } else if line_end < text.len() {
            line_end + 1
        } else {
            line_end
        };
    }

    // Handle unclosed fence (extends to end of text)
    if let Some(open) = current_fence {
        spans.push(FenceSpan {
            start: open.start,
            end: text.len(),
            marker: open.marker,
            indent: open.indent,
            language: open.language,
            info: open.info,
        });
    }

    spans
}

/// Check if an index is a safe place to break (not inside any fence).
///
/// Returns `true` if the index is outside all fence spans.
#[must_use]
pub fn is_safe_fence_break(spans: &[FenceSpan], index: usize) -> bool {
    !spans.iter().any(|span| span.contains(index))
}

/// Find the fence span containing the given index, if any.
#[must_use]
pub fn find_fence_at(spans: &[FenceSpan], index: usize) -> Option<&FenceSpan> {
    spans.iter().find(|span| span.contains(index))
}

/// Get fence split information if breaking at the given index would split a fence.
///
/// Returns `Some(FenceSplit)` if the index is inside a fence, containing
/// the lines needed to close and reopen the fence.
#[must_use]
pub fn get_fence_split(spans: &[FenceSpan], index: usize) -> Option<FenceSplit> {
    find_fence_at(spans, index).map(|span| FenceSplit {
        close_line: span.close_line(),
        reopen_line: span.reopen_line(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_fence() {
        let text = "Hello\n```rust\nfn main() {}\n```\nWorld";
        let spans = parse_fence_spans(text);

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].language(), Some("rust"));
        assert_eq!(spans[0].marker(), "```");
        assert_eq!(spans[0].indent(), "");
    }

    #[test]
    fn test_parse_tilde_fence() {
        let text = "~~~python\nprint('hello')\n~~~";
        let spans = parse_fence_spans(text);

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].language(), Some("python"));
        assert_eq!(spans[0].marker(), "~~~");
    }

    #[test]
    fn test_parse_indented_fence() {
        let text = "  ```js\n  console.log('x');\n  ```";
        let spans = parse_fence_spans(text);

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].indent(), "  ");
        assert_eq!(spans[0].language(), Some("js"));
    }

    #[test]
    fn test_parse_unclosed_fence() {
        let text = "Start\n```\ncode without closing";
        let spans = parse_fence_spans(text);

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].end(), text.len());
    }

    #[test]
    fn test_parse_multiple_fences() {
        let text = "```rust\nfn a() {}\n```\nBetween\n```python\ndef b():\n    pass\n```";
        let spans = parse_fence_spans(text);

        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].language(), Some("rust"));
        assert_eq!(spans[1].language(), Some("python"));
    }

    #[test]
    fn test_parse_no_language() {
        let text = "```\nplain code\n```";
        let spans = parse_fence_spans(text);

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].language(), None);
    }

    #[test]
    fn test_parse_longer_closing() {
        // Closing marker can be longer than opening
        let text = "```js\ncode\n`````";
        let spans = parse_fence_spans(text);

        assert_eq!(spans.len(), 1);
        // Should be closed (longer closing is valid)
    }

    #[test]
    fn test_parse_shorter_closing_invalid() {
        // Closing marker shorter than opening doesn't close
        let text = "````js\ncode\n```\nmore";
        let spans = parse_fence_spans(text);

        assert_eq!(spans.len(), 1);
        // Fence extends to end because ``` doesn't close ````
        assert_eq!(spans[0].end(), text.len());
    }

    #[test]
    fn test_is_safe_fence_break() {
        let text = "Hello\n```\ncode\n```\nWorld";
        let spans = parse_fence_spans(text);

        // Before fence
        assert!(is_safe_fence_break(&spans, 3));
        // Inside fence
        assert!(!is_safe_fence_break(&spans, 10));
        // After fence
        assert!(is_safe_fence_break(&spans, 20));
    }

    #[test]
    fn test_find_fence_at() {
        let text = "Hello\n```rust\ncode\n```\nWorld";
        let spans = parse_fence_spans(text);

        assert!(find_fence_at(&spans, 3).is_none());
        assert!(find_fence_at(&spans, 12).is_some());
        assert_eq!(find_fence_at(&spans, 12).unwrap().language(), Some("rust"));
    }

    #[test]
    fn test_fence_split() {
        let text = "```rust\nfn main() {\n    // long code\n}\n```";
        let spans = parse_fence_spans(text);

        let split = get_fence_split(&spans, 15).unwrap();
        assert_eq!(split.close_line, "```");
        assert_eq!(split.reopen_line, "```rust");
    }

    #[test]
    fn test_fence_split_indented() {
        let text = "  ```python\n  def foo():\n      pass\n  ```";
        let spans = parse_fence_spans(text);

        let split = get_fence_split(&spans, 20).unwrap();
        assert_eq!(split.close_line, "  ```");
        assert_eq!(split.reopen_line, "  ```python");
    }

    #[test]
    fn reopen_preserves_fence_info() {
        let spans = parse_fence_spans("```rust title=example\ncode\n```");
        assert_eq!(
            get_fence_split(&spans, 15).unwrap().reopen_line,
            "```rust title=example"
        );
    }

    #[test]
    fn closing_fence_cannot_be_more_indented_than_opening() {
        let spans = parse_fence_spans("```\ncode\n   ```\n");
        assert_eq!(spans.len(), 1);
        assert_eq!(
            spans[0].end(),
            spans[0].start() + "```\ncode\n   ```\n".len()
        );
    }

    #[test]
    fn test_close_reopen_lines() {
        let span = FenceSpan {
            start: 0,
            end: 100,
            marker: "```".to_string(),
            indent: "".to_string(),
            language: Some("typescript".to_string()),
            info: "typescript".to_string(),
        };

        assert_eq!(span.close_line(), "```");
        assert_eq!(span.reopen_line(), "```typescript");
    }

    #[test]
    fn test_mixed_fence_types() {
        // Tilde fence should not be closed by backtick fence
        let text = "~~~\ncode\n```\nmore\n~~~";
        let spans = parse_fence_spans(text);

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].marker(), "~~~");
    }

    #[test]
    fn test_empty_text() {
        let spans = parse_fence_spans("");
        assert!(spans.is_empty());
    }

    #[test]
    fn test_no_fences() {
        let text = "Just regular text\nwith multiple lines\nno code fences";
        let spans = parse_fence_spans(text);
        assert!(spans.is_empty());
    }

    #[test]
    fn test_fence_with_info_string() {
        // Info string can have more than just language
        let text = "```rust,ignore\ncode\n```";
        let spans = parse_fence_spans(text);

        assert_eq!(spans.len(), 1);
        // Info string without whitespace is stored as-is
        assert_eq!(spans[0].language(), Some("rust,ignore"));
    }

    #[test]
    fn test_parse_empty_fence_body() {
        let text = "```\n```";
        let spans = parse_fence_spans(text);

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].start(), 0);
        assert_eq!(spans[0].end(), 4);
        assert_eq!(spans[0].language(), None);
    }

    #[test]
    fn test_get_fence_split_none() {
        let text = "Hello\n```rust\ncode\n```\nWorld";
        let spans = parse_fence_spans(text);
        let span = &spans[0];

        // Before fence - should return None
        assert!(get_fence_split(&spans, 3).is_none());
        // After fence - should return None
        assert!(get_fence_split(&spans, span.end() + 1).is_none());
        // At exact fence boundary (start) - should return None
        assert!(get_fence_split(&spans, span.start()).is_none());
        // At exact fence boundary (end) - should return None
        assert!(get_fence_split(&spans, span.end()).is_none());
    }

    #[test]
    fn test_unicode_byte_offsets() {
        // Multi-byte UTF-8 characters before and inside fence
        let text = "Hello 🦀\n```rust\nfn main() {}\n```\nWorld 🌍";
        let spans = parse_fence_spans(text);

        assert_eq!(spans.len(), 1);
        // "Hello 🦀" = 10 bytes (🦀 is 4 bytes), plus \n = 1 byte
        // So fence starts at byte offset 11
        assert_eq!(spans[0].start(), 11);
        // Verify byte indexing is consistent
        assert!(spans[0].contains(spans[0].start() + 1));
        assert!(!spans[0].contains(spans[0].start()));
    }

    #[test]
    fn test_fence_boundary_conditions() {
        let text = "ab\n```rust\ncode\n```\ncd";
        let spans = parse_fence_spans(text);
        let span = &spans[0];

        // Boundary positions are NOT inside fence (exclusive bounds)
        assert!(!span.contains(span.start()));
        assert!(!span.contains(span.end()));

        // One byte inside boundaries IS inside fence
        assert!(span.contains(span.start() + 1));
        assert!(span.contains(span.end() - 1));

        // Outside boundaries is NOT inside fence
        assert!(!span.contains(span.start() - 1));
        assert!(!span.contains(span.end() + 1));
    }

    #[test]
    fn test_fence_split_none_outside_all_fences() {
        let text = "no fences here at all";
        let spans = parse_fence_spans(text);
        assert!(spans.is_empty());
        assert!(get_fence_split(&spans, 5).is_none());
    }

    #[test]
    fn test_closing_fence_not_inside_span() {
        let text = "```rust\ncode\n```";
        let spans = parse_fence_spans(text);
        let span = &spans[0];

        assert_eq!(span.start(), 0);
        assert_eq!(span.end(), 13);

        assert!(!span.contains(span.start()));
        assert!(!span.contains(span.end()));
        assert!(span.contains(8));

        assert!(!span.contains(13));
        assert!(!span.contains(14));
        assert!(!span.contains(15));

        assert!(get_fence_split(&spans, 13).is_none());
        assert!(get_fence_split(&spans, 14).is_none());
        assert!(get_fence_split(&spans, 15).is_none());
    }

    #[test]
    fn test_language_extracts_first_word_only() {
        let text = "```rust ignore\ncode\n```";
        let spans = parse_fence_spans(text);

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].language(), Some("rust"));
    }

    #[test]
    fn test_whitespace_only_info_string() {
        let text = "```   \ncode\n```";
        let spans = parse_fence_spans(text);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].language(), None);
    }

    #[test]
    fn test_cr_line_endings() {
        // Legacy Mac CR-only format — str::lines() does NOT split on \r alone
        let text = "foo\r```\rcode\r```\rbar";
        let spans = parse_fence_spans(text);
        // Entire text treated as single line; fences not recognized
        assert!(spans.is_empty());
    }

    #[test]
    fn test_crlf_line_endings_offsets() {
        // \r\n line endings exercise the `line_end + 2` advancement branch.
        // Byte layout: "```rust"(0..7) \r\n(7,8) "code"(9..13) \r\n(13,14)
        //              "```"(15..18) \r\n(18,19) "after"(20..25)
        let text = "```rust\r\ncode\r\n```\r\nafter";
        let spans = parse_fence_spans(text);

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].start(), 0);
        // `end` is the closing fence line start, not shifted by the \r\n width.
        assert_eq!(spans[0].end(), 15);
        assert_eq!(spans[0].language(), Some("rust"));
        // Boundary semantics survive \r\n offset arithmetic.
        assert!(spans[0].contains(8));
        assert!(!spans[0].contains(spans[0].end()));
        assert!(is_safe_fence_break(&spans, 0));
    }
}
