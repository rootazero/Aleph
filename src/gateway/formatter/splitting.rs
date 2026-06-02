//! Smart message splitting that respects paragraph and code block boundaries.

use crate::markdown::fences::{find_fence_at, is_safe_fence_break, parse_fence_spans, FenceSpan};

/// Split a message into chunks of at most `max_len` bytes.
pub(super) fn split_message(text: &str, max_len: usize) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if remaining.len() <= max_len {
            chunks.push(remaining.to_string());
            break;
        }

        // Fence spans for the current remainder; offsets line up with `candidate`
        // below since it is always a prefix of `remaining`.
        let spans = parse_fence_spans(remaining);

        // Find the nearest char boundary at or before max_len to avoid
        // splitting in the middle of a multi-byte UTF-8 character.
        let mut safe_max = max_len;
        while safe_max > 0 && !remaining.is_char_boundary(safe_max) {
            safe_max -= 1;
        }
        if safe_max == 0 {
            // Degenerate case: entire prefix is a single huge codepoint.
            // Advance to the next boundary to make progress.
            safe_max = remaining.len().min(max_len + 3);
            while safe_max < remaining.len() && !remaining.is_char_boundary(safe_max) {
                safe_max += 1;
            }
        }

        // Try to find the best split point within safe_max.
        let candidate = &remaining[..safe_max];

        let mut split_pos = find_split_point(candidate, &spans);

        // Character-level fallback: if find_split_point returned 0 (no viable
        // boundary found), force a hard split at safe_max to guarantee forward
        // progress and the max_len contract.
        if split_pos == 0 {
            split_pos = safe_max;
        }

        let (chunk, rest) = remaining.split_at(split_pos);
        let chunk = chunk.trim_end();
        if !chunk.is_empty() {
            chunks.push(chunk.to_string());
        }
        remaining = rest.trim_start_matches('\n');
        if remaining.is_empty() {
            break;
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
/// [`crate::markdown::fences`] parser (handles ``` and ~~~, indented fences,
/// and language tags) rather than a local backtick count.
fn find_split_point(candidate: &str, spans: &[FenceSpan]) -> usize {
    // If the window end lands inside a fenced block, back off to the start of
    // that block so the fence is never split mid-way.
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

    // Last resort: split at max_len.
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
