//! Smart message splitting that respects paragraph and code block boundaries.
//!
//! This is the single canonical splitter for the whole gateway: outbound
//! channel adapters reach it via [`crate::gateway::formatter::MessageFormatter::split`]
//! and the streaming reply path delegates to it through
//! `ReplyEmitter::split_message`. Fence detection is delegated to the
//! [`crate::markdown::fences`] parser (handles ``` and ~~~, indented fences and
//! language tags) — there is deliberately no second backtick-counting heuristic.

use crate::markdown::fences::{
    find_fence_at, get_fence_split, is_safe_fence_break, parse_fence_spans, FenceSpan,
};

/// Split a message into chunks of at most `max_len` bytes.
///
/// Guarantees:
/// - Every chunk is at most `max_len` bytes (UTF-8 boundary safe).
/// - A fenced code block that fits in `max_len` is never split.
/// - A fenced code block larger than `max_len` is split with the fence closed
///   on the trailing chunk and re-opened (preserving the language tag) on the
///   following chunk, so no chunk ever carries an unbalanced fence.
pub(super) fn split_message(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_string()];
    }

    let mut chunks: Vec<String> = Vec::new();
    // Owned buffer: when we split inside an open fence we must *prepend* the
    // re-opening fence line to the remainder, which a borrowed `&str` can't do.
    let mut buf = text.to_string();

    while !buf.is_empty() {
        if buf.len() <= max_len {
            chunks.push(buf);
            break;
        }

        let spans = parse_fence_spans(&buf);

        // When the buffer begins *inside* an unclosed fence that overflows the
        // window, we will have to hard-split within it and append a closing
        // line. Reserve room for that closing line so the emitted chunk still
        // respects `max_len`.
        let reserve = spans
            .first()
            .filter(|f| f.start() == 0 && f.end() > max_len)
            .map_or(0, |f| f.close_line().len() + 1);
        let budget = max_len.saturating_sub(reserve).max(1);

        // Nearest char boundary at or before `budget`.
        let mut safe_max = budget.min(buf.len());
        while safe_max > 0 && !buf.is_char_boundary(safe_max) {
            safe_max -= 1;
        }
        if safe_max == 0 {
            // Degenerate case: a single codepoint wider than the budget. Advance
            // to the next boundary to guarantee forward progress.
            safe_max = buf.len().min(max_len + 3);
            while safe_max < buf.len() && !buf.is_char_boundary(safe_max) {
                safe_max += 1;
            }
        }

        let candidate = &buf[..safe_max];
        let mut split_pos = find_split_point(candidate, &spans);
        // Character-level fallback: guarantee forward progress and the contract.
        if split_pos == 0 {
            split_pos = safe_max;
        }

        // If the chosen break lands strictly inside a fence, close it on this
        // chunk and re-open it on the remainder.
        let fence_split = get_fence_split(&spans, split_pos);

        let chunk = buf[..split_pos].trim_end().to_string();
        let rest = buf[split_pos..].trim_start_matches('\n').to_string();

        match fence_split {
            Some(fs) => {
                if !chunk.is_empty() {
                    chunks.push(format!("{}\n{}", chunk, fs.close_line));
                }
                if rest.is_empty() {
                    buf.clear();
                } else {
                    buf = format!("{}\n{}", fs.reopen_line, rest);
                }
            }
            None => {
                if !chunk.is_empty() {
                    chunks.push(chunk);
                }
                buf = rest;
            }
        }
    }

    if chunks.is_empty() {
        chunks.push(String::new());
    }

    chunks
}

/// Find the best byte offset to split `candidate`.
///
/// Prefers paragraph boundaries, then line boundaries. Never splits inside a
/// fenced code block: fence detection is delegated to the canonical
/// [`crate::markdown::fences`] parser rather than a local backtick count.
fn find_split_point(candidate: &str, spans: &[FenceSpan]) -> usize {
    // If the window end lands inside a fenced block, back off to the start of
    // that block so the fence is never split mid-way — unless the block itself
    // starts at the buffer head (it overflows the window and the caller closes
    // and re-opens it instead).
    if let Some(fence) = find_fence_at(spans, candidate.len()) {
        if fence.start() > 0 {
            return fence.start();
        }
    }

    // Prefer the latest paragraph boundary that is a safe (non-fenced) break.
    if let Some(pos) = rfind_safe_break(candidate, "\n\n", spans) {
        return pos;
    }

    // Then the latest safe single-newline boundary.
    if let Some(pos) = rfind_safe_break(candidate, "\n", spans) {
        return pos;
    }

    // Last resort: split at the window edge.
    candidate.len()
}

/// Find the last occurrence of `sep` in `candidate` whose byte offset is a safe
/// fence break (not strictly inside a code fence). Searches progressively
/// earlier matches until a safe one is found.
fn rfind_safe_break(candidate: &str, sep: &str, spans: &[FenceSpan]) -> Option<usize> {
    let mut search_end = candidate.len();
    while let Some(pos) = candidate[..search_end].rfind(sep) {
        if pos > 0 && is_safe_fence_break(spans, pos) {
            return Some(pos);
        }
        if pos == 0 {
            break;
        }
        search_end = pos;
    }
    None
}
