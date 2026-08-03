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

/// Reduce the previous observation of "was a run active" to the `was_busy`
/// [`should_auto_drain_on_settle`] should see, given that the composer watches
/// the *foreground* conversation and a tab swap re-projects it onto a different
/// one.
///
/// Two failures come out of ignoring the swap. Reading the previous
/// observation across it fabricates a busy → idle edge — leave a busy
/// conversation, open an idle one, and the newly-opened conversation's queue
/// fires on arrival, having settled nothing. And a conversation whose run
/// settles while it is in the background gets no edge at all, because both
/// drain triggers live in the single foreground component: its queue is
/// stranded until something else happens to it.
///
/// Treating a switch as "was busy" fixes both: there is no fabricated edge
/// (arriving at a *busy* conversation still yields no drain, since `is_busy`
/// is true), and arriving at an idle conversation that still has queued
/// prompts is read as the settle it missed.
#[must_use]
pub const fn was_busy_across_switch(prev_busy: Option<bool>, same_conversation: bool) -> bool {
    match prev_busy {
        Some(busy) if same_conversation => busy,
        // No comparable previous observation: first run, or it described
        // another conversation.
        _ => true,
    }
}

/// Decide whether a bare `ArrowUp` in the composer should recall the newest
/// queued prompt instead of moving the caret.
///
/// A bare `ArrowUp` is only free to mean "recall" when the textarea has no
/// caret work to do — i.e. the draft is blank. Any draft with content (even a
/// single line) keeps `ArrowUp` for text editing; the modifier binding
/// (`Alt`/`Option` + `ArrowUp`) covers that case instead. Attachments do not
/// enter the decision: they are not text the caret can move through.
///
/// `draft_is_empty` is the caller's *trimmed* emptiness — whitespace has no
/// line to move to either, and every other draft test in the composer trims.
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
        // Whitespace-only counts as empty: the caller trims before asking.
        assert!(should_recall_on_bare_arrow_up(1, "   ".trim().is_empty()));
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
    fn a_tab_swap_does_not_fabricate_a_settle() {
        // Left a busy conversation, opened a busy one: no edge either way.
        let was_busy = was_busy_across_switch(Some(true), false);
        assert!(!should_auto_drain_on_settle(was_busy, true, 2, false));
    }

    #[test]
    fn opening_a_conversation_that_settled_in_the_background_drains_it() {
        // Its run finished while another tab was in front, so no edge was ever
        // observed for it. Arriving to find it idle with ghosts IS that edge.
        let was_busy = was_busy_across_switch(Some(false), false);
        assert!(should_auto_drain_on_settle(was_busy, false, 1, false));
    }

    #[test]
    fn a_stop_on_the_conversation_still_suppresses_the_drain_on_arrival() {
        let was_busy = was_busy_across_switch(Some(false), false);
        assert!(!should_auto_drain_on_settle(was_busy, false, 1, true));
    }

    #[test]
    fn staying_on_one_conversation_reads_its_own_previous_observation() {
        assert!(was_busy_across_switch(Some(true), true));
        assert!(!was_busy_across_switch(Some(false), true));
        // Steady idle on one conversation must not drain — that is the case
        // `should_auto_drain_on_settle` rejects via `!was_busy`.
        let was_busy = was_busy_across_switch(Some(false), true);
        assert!(!should_auto_drain_on_settle(was_busy, false, 2, false));
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
