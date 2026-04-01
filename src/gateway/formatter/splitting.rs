//! Smart message splitting that respects paragraph and code block boundaries.

/// Split a message into chunks of at most `max_len` bytes.
pub(super) fn split_message(text: &str, max_len: usize) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if remaining.len() <= max_len {
            chunks.push(remaining.to_string());
            break;
        }

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

        let mut split_pos = find_split_point(candidate);

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
/// Prefers paragraph boundaries, then line boundaries. Avoids splitting inside
/// fenced code blocks.
fn find_split_point(candidate: &str) -> usize {
    // Count fence openings/closings in the candidate to detect if we're mid-block.
    let fence_count = candidate.matches("```").count();
    let in_code_block = !fence_count.is_multiple_of(2);

    if in_code_block {
        // We're in the middle of a code block. Try to split BEFORE the opening
        // fence of the last unclosed block.
        if let Some(pos) = candidate.rfind("```") {
            if pos > 0 {
                return pos;
            }
        }
    }

    // Prefer double newline (paragraph boundary).
    if let Some(pos) = candidate.rfind("\n\n") {
        if pos > 0 {
            return pos;
        }
    }

    // Prefer single newline (line boundary).
    if let Some(pos) = candidate.rfind('\n') {
        if pos > 0 {
            return pos;
        }
    }

    // Last resort: split at max_len.
    candidate.len()
}
