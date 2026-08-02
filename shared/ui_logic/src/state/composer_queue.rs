//! Pure decision logic for the chat composer's prompt queue.
//!
//! The queue lets a user line up follow-up prompts while a turn is still
//! running; they auto-drain when the turn settles *naturally*. An explicit
//! Stop must suppress exactly one auto-drain — otherwise cancelling a turn
//! would flip busy → idle, the queue would immediately re-fire its head, and
//! the agent would keep running (the "Stop button does nothing" trap).
//!
//! Mirrors hermes-agent's `shouldAutoDrainOnSettle`, ported to Rust so the
//! decision is host-testable without a browser (the WASM panel only owns the
//! signals and side effects).

/// Decide whether the composer should auto-send the next queued prompt when a
/// turn settles (busy transitions `true → false`).
///
/// * `was_busy` — whether a run was active on the previous observation.
/// * `is_busy` — whether a run is active now.
/// * `queue_len` — number of prompts waiting in the queue.
/// * `user_interrupted` — whether the user pressed Stop since the last settle.
#[must_use]
pub const fn should_auto_drain_on_settle(
    was_busy: bool,
    is_busy: bool,
    queue_len: usize,
    user_interrupted: bool,
) -> bool {
    // Only react to a genuine busy → idle edge; ignore steady state and entry.
    if is_busy || !was_busy {
        return false;
    }
    // An explicit Stop suppresses exactly one auto-drain.
    if user_interrupted {
        return false;
    }
    queue_len > 0
}

/// Decide whether queued prompts should be flushed mid-run at a turn
/// boundary (the agent just crossed into a new Think iteration). Unlike
/// [`should_auto_drain_on_settle`], this fires *while the run is still
/// active* — the flush rides the gateway's Steer path so the agent weaves
/// the queued prompts into the ongoing run at its next turn.
#[must_use]
pub const fn should_flush_on_turn_boundary(queue_len: usize, is_busy: bool) -> bool {
    is_busy && queue_len > 0
}

/// Decide whether a bare `ArrowUp` in the composer should recall the newest
/// queued prompt instead of moving the caret.
///
/// A bare `ArrowUp` is only free to mean "recall" when the textarea has no
/// caret work to do — i.e. the draft text is empty. Any non-empty draft (even
/// a single line) keeps `ArrowUp` for text editing; the modifier binding
/// (`Alt`/`Option` + `ArrowUp`) covers that case instead. Attachments do not
/// enter the decision: they are not text the caret can move through.
#[must_use]
pub const fn should_recall_on_bare_arrow_up(queue_len: usize, draft_is_empty: bool) -> bool {
    queue_len > 0 && draft_is_empty
}

/// Merge a recalled queued prompt back into whatever the composer already
/// holds, **recalled text first**.
///
/// Recall pops from the *tail* of the queue (newest first), so putting the
/// recalled text ahead of the draft is what makes repeated recalls rebuild the
/// original queue order: recalling `b` then `a` from queue `[a, b]` yields
/// `"a\n\nb"`. Reference implementations diverge here — codex's `alt+Up`
/// replaces the composer outright (losing an in-progress draft) while pi
/// restores the whole queue at once; prepending gets pi's ordering with
/// codex's one-at-a-time granularity and loses nothing.
///
/// A blank side is dropped rather than contributing a leading or trailing
/// separator. The draft is otherwise preserved verbatim, inner blank lines
/// included.
#[must_use]
pub fn merge_recalled_draft(recalled: &str, draft: &str) -> String {
    match (recalled.trim().is_empty(), draft.trim().is_empty()) {
        (true, true) => String::new(),
        (true, false) => draft.to_string(),
        (false, true) => recalled.to_string(),
        (false, false) => format!("{recalled}\n\n{draft}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_arrow_up_recalls_only_when_the_textarea_has_nothing_to_move_through() {
        // Empty draft + something queued → the key has no caret work to do.
        assert!(should_recall_on_bare_arrow_up(1, true));
        // A draft is being edited → ArrowUp belongs to the caret.
        assert!(!should_recall_on_bare_arrow_up(1, false));
        // Nothing queued → nothing to recall, whatever the draft is.
        assert!(!should_recall_on_bare_arrow_up(0, true));
        assert!(!should_recall_on_bare_arrow_up(0, false));
    }

    #[test]
    fn recall_puts_the_queued_text_ahead_of_the_draft() {
        assert_eq!(merge_recalled_draft("queued", "typing"), "queued\n\ntyping");
    }

    #[test]
    fn recalling_twice_from_the_tail_rebuilds_queue_order() {
        // Queue was [a, b]; the user recalls from the tail, newest first.
        let after_b = merge_recalled_draft("b", "");
        let after_a = merge_recalled_draft("a", &after_b);
        assert_eq!(after_a, "a\n\nb");
    }

    #[test]
    fn recall_never_clobbers_a_whitespace_only_draft_into_a_leading_gap() {
        assert_eq!(merge_recalled_draft("queued", "   \n "), "queued");
    }

    #[test]
    fn recall_preserves_the_draft_verbatim_including_inner_blank_lines() {
        assert_eq!(
            merge_recalled_draft("q", "line1\n\nline2"),
            "q\n\nline1\n\nline2"
        );
    }

    #[test]
    fn an_empty_recall_leaves_the_draft_untouched() {
        assert_eq!(merge_recalled_draft("  ", "draft"), "draft");
        assert_eq!(merge_recalled_draft("", ""), "");
    }

    #[test]
    fn drains_on_natural_settle_with_queue() {
        assert!(should_auto_drain_on_settle(true, false, 1, false));
    }

    #[test]
    fn no_drain_when_still_busy() {
        assert!(!should_auto_drain_on_settle(true, true, 3, false));
    }

    #[test]
    fn no_drain_without_a_busy_to_idle_edge() {
        // Steady idle — never was busy.
        assert!(!should_auto_drain_on_settle(false, false, 2, false));
    }

    #[test]
    fn no_drain_when_queue_empty() {
        assert!(!should_auto_drain_on_settle(true, false, 0, false));
    }

    #[test]
    fn explicit_stop_suppresses_exactly_one_drain() {
        // The interrupted settle is suppressed...
        assert!(!should_auto_drain_on_settle(true, false, 2, true));
        // ...but a subsequent natural settle (flag reset) drains again.
        assert!(should_auto_drain_on_settle(true, false, 2, false));
    }

    #[test]
    fn flushes_on_boundary_when_busy_with_queue() {
        assert!(should_flush_on_turn_boundary(1, true));
    }

    #[test]
    fn no_flush_when_queue_empty() {
        assert!(!should_flush_on_turn_boundary(0, true));
    }

    #[test]
    fn no_flush_when_idle() {
        assert!(!should_flush_on_turn_boundary(2, false));
    }
}
