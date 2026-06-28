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
}
