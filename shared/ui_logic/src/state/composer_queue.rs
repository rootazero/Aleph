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

/// Decide whether an Up-arrow press should **retract** the newest queued
/// prompt back into the composer instead of doing what Up normally does.
///
/// Ported from codex's `chat.edit_queued_message` action
/// (`tui/src/chatwidget/interaction.rs`, default `Alt+Up` / `Shift+Left`),
/// with one deliberate divergence and one addition:
///
/// * **Divergence — plain Up is gated on an empty draft.** Codex binds the
///   action to a *modified* chord because plain Up is history recall in its
///   composer. Aleph's Panel composer is a `<textarea>`: plain Up is caret
///   movement between lines, and hijacking it unconditionally would break
///   multi-line editing. An empty textarea has nowhere for the caret to go,
///   so gating on "no draft" makes the plain-key gesture unambiguous — and it
///   is exactly the moment the gesture is wanted (Enter queued the line and
///   cleared the box; Up takes it back).
/// * **Addition — `modified` bypasses the draft gate.** `Alt/⌥+Up` matches
///   codex's own binding and works mid-draft. That is only safe here because
///   Aleph's retract is non-destructive: the draft is folded back into the
///   queue rather than overwritten. Codex can afford the overwrite because
///   its composer has Up/Ctrl+R history recall to undo it; the Panel composer
///   has no such undo, so an overwrite there would be unrecoverable.
///
/// `popup_open` covers the slash-command palette and the @-mention palette,
/// which own the arrow keys while they are up (codex's
/// `no_modal_or_popup_active`).
#[must_use]
pub const fn should_retract_on_up(
    queue_len: usize,
    draft_is_empty: bool,
    popup_open: bool,
    modified: bool,
) -> bool {
    if queue_len == 0 || popup_open {
        return false;
    }
    modified || draft_is_empty
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn plain_up_retracts_only_from_an_empty_draft() {
        assert!(should_retract_on_up(1, true, false, false));
        // A draft is being edited — Up is caret movement, hands off.
        assert!(!should_retract_on_up(1, false, false, false));
    }

    #[test]
    fn modified_up_retracts_even_mid_draft() {
        // Safe because retract folds the draft back into the queue rather
        // than overwriting it (the codex binding, without the codex data loss).
        assert!(should_retract_on_up(1, false, false, true));
    }

    #[test]
    fn nothing_queued_means_nothing_to_retract() {
        assert!(!should_retract_on_up(0, true, false, false));
        assert!(!should_retract_on_up(0, false, false, true));
    }

    #[test]
    fn an_open_palette_owns_the_arrow_keys() {
        // The slash / @-mention palettes navigate with Up; retracting out from
        // under them would steal the key mid-selection.
        assert!(!should_retract_on_up(2, true, true, false));
        assert!(!should_retract_on_up(2, true, true, true));
    }
}
