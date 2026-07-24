//! Incremental Markdown rendering for streamed assistant text.
//!
//! The `ask`/`chat-control send --stream` paths previously buffered every
//! `ResponseChunk` and rendered the whole body once at end-of-run, so the
//! terminal showed live tool activity but a frozen body until the final
//! token arrived. This module ports hermes-agent's *stable-prefix* trick
//! (`streamingMarkdown.tsx`): the streamed buffer is split at the last safe
//! block boundary — a blank line outside any fenced code block — and the
//! stable prefix is rendered and flushed immediately, while only the
//! unstable suffix stays buffered. Each completed block is rendered with the
//! existing [`super::markdown::render`] pipeline (wire-first: zero new
//! dependencies, same ANSI/`NO_COLOR` degradation).
//!
//! Trade-off (same as hermes): blocks separated by blank lines are rendered
//! independently, so cross-block state (e.g. footnote definitions) does not
//! carry over. Assistant replies essentially never use those constructs;
//! ordered-list ordinals survive because Markdown numbers items explicitly.

use super::markdown;

/// Accumulates streamed Markdown and yields rendered ANSI text for each
/// completed block prefix. Pure (no I/O) — the caller decides where flushed
/// blocks go, so this unit-tests without a daemon or TTY.
#[derive(Default)]
pub struct MarkdownStream {
    buf: String,
}

impl MarkdownStream {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a streamed delta. When the buffer now contains one or more
    /// complete blocks, renders and returns them, retaining only the
    /// unstable suffix. Returns `None` while no safe boundary exists.
    pub fn push(&mut self, delta: &str) -> Option<String> {
        self.buf.push_str(delta);
        let cut = last_block_boundary(&self.buf)?;
        let stable: String = self.buf.drain(..cut).collect();
        let rendered = markdown::render(&stable);
        if rendered.trim().is_empty() {
            None
        } else {
            Some(rendered)
        }
    }

    /// Flush the remaining suffix at end-of-stream. Idempotent: the buffer
    /// is drained, so a second call returns `None`.
    pub fn finish(&mut self) -> Option<String> {
        let rest = std::mem::take(&mut self.buf);
        if rest.trim().is_empty() {
            return None;
        }
        let rendered = markdown::render(&rest);
        if rendered.trim().is_empty() {
            None
        } else {
            Some(rendered)
        }
    }
}

/// Byte offset just past the last blank line that sits outside a fenced code
/// block, or `None` when no safe boundary exists yet. Only complete lines
/// (terminated by `\n`) are considered, so a trailing partial line can never
/// be split mid-token.
fn last_block_boundary(s: &str) -> Option<usize> {
    let mut in_fence = false;
    let mut boundary = None;
    let mut offset = 0usize;
    for line in s.split_inclusive('\n') {
        let complete = line.ends_with('\n');
        let trimmed = line.trim();
        if complete && (trimmed.starts_with("```") || trimmed.starts_with("~~~")) {
            // Toggle fence state. Mismatched fences degrade to "hold
            // everything", which is the safe direction.
            in_fence = !in_fence;
        } else if complete && !in_fence && trimmed.is_empty() {
            boundary = Some(offset + line.len());
        }
        offset += line.len();
    }
    boundary.filter(|&b| b > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Strip ANSI SGR/OSC-8 noise so assertions target structure.
    fn strip_ansi(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                match chars.peek() {
                    Some(']') => {
                        // OSC … terminated by ESC \ or BEL.
                        while let Some(n) = chars.next() {
                            if n == '\x07' || (n == '\x1b' && chars.next() == Some('\\')) {
                                break;
                            }
                        }
                    }
                    _ => {
                        for n in chars.by_ref() {
                            if n == 'm' {
                                break;
                            }
                        }
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn holds_until_blank_line_then_flushes_prefix() {
        let mut s = MarkdownStream::new();
        assert!(s.push("first paragraph still strea").is_none());
        let out = s.push("ming\n\nsecond par").expect("boundary crossed");
        assert!(strip_ansi(&out).contains("first paragraph still streaming"));
        // The unstable suffix stays buffered for finish().
        let rest = s.finish().expect("suffix flushed");
        assert!(strip_ansi(&rest).contains("second par"));
        assert!(s.finish().is_none(), "finish drains the buffer");
    }

    #[test]
    fn never_splits_inside_code_fence() {
        let mut s = MarkdownStream::new();
        // Blank line *inside* the fence must not be treated as a boundary.
        assert!(s.push("```rust\nlet a = 1;\n\nlet b = 2;\n").is_none());
        let out = s
            .push("```\n\nafter\n")
            .expect("fence closed, boundary found");
        let plain = strip_ansi(&out);
        assert!(plain.contains("let a = 1;"));
        assert!(plain.contains("let b = 2;"));
        let rest = s.finish().expect("trailing text");
        assert!(strip_ansi(&rest).contains("after"));
    }

    #[test]
    fn boundary_completes_across_pushes() {
        let mut s = MarkdownStream::new();
        // "\n\n" split across two pushes: the first push ends on a complete
        // but non-blank line (no boundary); the bare "\n" tail completes the
        // blank line and flushes the paragraph.
        assert!(s.push("para\n").is_none());
        let out = s.push("\n").expect("blank line completed → flush");
        assert!(strip_ansi(&out).contains("para"));
        assert!(s.finish().is_none(), "nothing left after the boundary");
    }

    #[test]
    fn single_final_chunk_renders_via_finish() {
        // Instant-buffer servers send one big final chunk with no incremental
        // deltas — everything lands in finish().
        let mut s = MarkdownStream::new();
        let pushed = s.push("# Title\n\nbody");
        let mut plain = String::new();
        if let Some(p) = pushed {
            plain.push_str(&strip_ansi(&p));
        }
        if let Some(r) = s.finish() {
            plain.push_str(&strip_ansi(&r));
        }
        assert!(plain.contains("Title"));
        assert!(plain.contains("body"));
    }

    #[test]
    fn cjk_text_streams_without_splitting_scalars() {
        let mut s = MarkdownStream::new();
        assert!(s.push("中文段落一\n\n中文").is_some());
        let rest = s.finish().expect("suffix");
        assert!(strip_ansi(&rest).contains("中文"));
    }
}
