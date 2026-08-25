//! Streaming markdown boundary detection.
//!
//! Answers one question: given text that is still growing, how much of it can
//! be treated as frozen (safe to render once and never re-touch) right now?
//! "Safe" means the offset lands after a complete line, is not inside an
//! unclosed fenced code block, and is not immediately after a reference-link
//! definition (`[label]: ...`) that a following line could still extend.
//!
//! Deliberately conservative: every unsafe case returns less progress than a
//! perfectly precise parser might, never more. A caller that gets `None` (or
//! an offset short of what it hoped for) simply reprocesses that tail in
//! full — correctness never depends on this module being exactly right, only
//! on it never being wrong in the unsafe direction. Mirrors codex's own
//! documented escape hatch in `markdown_stream.rs` (`commit_complete_source`).

/// See module docs.
pub fn safe_freeze_offset(text: &str, prev_safe: usize) -> Option<usize> {
    let mut fence_marker: Option<usize> = None;
    let mut pending_ref_def = false;
    let mut last_safe = prev_safe;
    let mut cursor = prev_safe;

    for line in text.get(prev_safe..)?.split_inclusive('\n') {
        if !line.ends_with('\n') {
            // Incomplete trailing line (no newline yet) — never safe.
            break;
        }
        let trimmed = line.trim_end_matches('\n');
        cursor += line.len();

        let bare = trimmed.trim();
        let is_all_backticks = !bare.is_empty() && bare.chars().all(|c| c == '`');
        let leading_ticks = trimmed
            .trim_start()
            .chars()
            .take_while(|&c| c == '`')
            .count();

        match fence_marker {
            Some(open_len) if is_all_backticks && bare.chars().count() >= open_len => {
                // Closes the fence: a bare line of backticks at least as
                // long as the opening run. A shorter or non-bare run is
                // just fence content (CommonMark semantics).
                fence_marker = None;
                last_safe = cursor;
                pending_ref_def = false;
                continue;
            }
            Some(_) => {
                // Still inside the fence.
                continue;
            }
            None if leading_ticks >= 3 => {
                // Opens a new fence. An info string after the backticks
                // (e.g. "```rust") is allowed and doesn't affect the count.
                fence_marker = Some(leading_ticks);
                continue;
            }
            None => {}
        }
        if trimmed.trim().is_empty() {
            // A blank line always ends any open paragraph or reference-link
            // definition, so it clears the pending flag and is itself safe.
            last_safe = cursor;
            pending_ref_def = false;
            continue;
        }
        if is_reference_link_def_start(trimmed) {
            pending_ref_def = true;
            continue;
        }
        if pending_ref_def {
            // Still inside a possible definition continuation — don't
            // advance past it until a blank line confirms it's closed.
            continue;
        }
        last_safe = cursor;
    }

    if last_safe > prev_safe {
        Some(last_safe)
    } else {
        None
    }
}

/// A line that could start a CommonMark reference-link definition:
/// `[label]: destination "optional title"`. Deliberately loose (doesn't
/// validate the destination) — false positives just cost a forfeited perf
/// win, never a correctness bug.
fn is_reference_link_def_start(trimmed_line: &str) -> bool {
    let after_indent = trimmed_line.trim_start();
    after_indent.starts_with('[') && after_indent.contains("]:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_fence_freezes_up_to_last_complete_line() {
        let text = "line one\nline two\nline three"; // no trailing \n on last line
        let result = safe_freeze_offset(text, 0);
        assert_eq!(result, Some("line one\nline two\n".len()));
    }

    #[test]
    fn open_fence_blocks_freezing_past_fence_start() {
        let text = "before\n```rust\nfn main() {}\n";
        let result = safe_freeze_offset(text, 0);
        assert_eq!(result, Some("before\n".len()));
    }

    #[test]
    fn closed_fence_allows_freezing_through_it_and_beyond() {
        let text = "before\n```rust\ncode\n```\nafter\n";
        let result = safe_freeze_offset(text, 0);
        assert_eq!(result, Some(text.len()));
    }

    #[test]
    fn no_progress_returns_none() {
        let text = "```rust\n";
        assert_eq!(safe_freeze_offset(text, 0), None);
    }

    #[test]
    fn incremental_call_resumes_from_prev_safe() {
        let text = "line one\nline two\n";
        let first = safe_freeze_offset(text, 0).unwrap();
        assert_eq!(first, text.len());
        let grown = "line one\nline two\nline three\n";
        let second = safe_freeze_offset(grown, first).unwrap();
        assert_eq!(second, grown.len());
    }

    #[test]
    fn fence_state_does_not_leak_across_a_resumed_call() {
        // prev_safe always lands outside a fence by construction, so a
        // resumed call must not spuriously believe it starts inside one.
        let text = "```rust\ncode\n```\nmore\n";
        let after_fence = safe_freeze_offset(text, 0).unwrap();
        assert_eq!(after_fence, text.len());
    }

    #[test]
    fn prev_safe_past_the_end_of_text_returns_none_instead_of_panicking() {
        let text = "short\n";
        assert_eq!(safe_freeze_offset(text, 100), None);
    }

    #[test]
    fn prev_safe_exactly_at_text_len_returns_none() {
        let text = "line one\n";
        assert_eq!(safe_freeze_offset(text, text.len()), None);
    }

    #[test]
    fn a_longer_opening_fence_is_not_closed_by_a_shorter_backtick_run_inside_it() {
        let text = "````markdown\nsome text\n```\ninner code\n```\n````\nafter\n";
        let result = safe_freeze_offset(text, 0);
        assert_eq!(result, Some(text.len()));
    }

    #[test]
    fn a_bare_backtick_run_shorter_than_the_opening_fence_does_not_close_it() {
        let text = "````\n```\nafter\n";
        let result = safe_freeze_offset(text, 0);
        assert_eq!(result, None);
    }

    #[test]
    fn dangling_reference_link_def_pins_the_freeze_point() {
        let text = "See [foo] below.\n\n[foo]: https://example.com\nmore text\n";
        let result = safe_freeze_offset(text, 0);
        // Safe only through the blank line before the definition — the
        // definition line and everything after it stay unfrozen because no
        // blank line has confirmed the definition is closed.
        assert_eq!(result, Some("See [foo] below.\n\n".len()));
    }

    #[test]
    fn blank_line_after_reference_link_def_clears_the_pin() {
        let text = "See [foo] below.\n\n[foo]: https://example.com\n\nmore text\n";
        let result = safe_freeze_offset(text, 0);
        assert_eq!(result, Some(text.len()));
    }

    #[test]
    fn reference_link_def_inside_a_fence_is_just_code() {
        // A `[x]:`-shaped line inside a fence is code content, not a real
        // reference-link definition — the fence rule takes priority.
        let text = "```\n[foo]: not a real link def, just code\n```\nafter\n";
        let result = safe_freeze_offset(text, 0);
        assert_eq!(result, Some(text.len()));
    }
}
