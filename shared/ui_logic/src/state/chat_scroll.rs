//! Pure decision logic for the chat transcript's auto-scroll.
//!
//! The message list follows the bottom of the stream while the user is reading
//! there, and stops following the moment they scroll up to re-read something —
//! offering a "↓ new messages" pill instead. The only interesting question is
//! what is allowed to *override* that pause and yank the viewport back down.
//!
//! # Why this is not "the message count grew"
//!
//! The rule Aleph ported from hermes-desktop's `useChatScroll` is "the user
//! sending re-arms stickiness": sending is an unambiguous *I am done reading
//! back*. hermes detects it as `messages.length > prev && last.role === "user"`,
//! which is sound in a single-user desktop app because the only thing that can
//! append a user row is the person at the keyboard.
//!
//! Aleph generalised that to "the number of `role == "user"` rows grew", and
//! that generalisation is false here, because Aleph has surfaces hermes does
//! not:
//!
//! - **A project-room peer's message is a `role == "user"` row.** It arrives
//!   over `run.session_user_message` and is appended by
//!   `ChatState::push_peer_user_message`. Under the count rule, a teammate
//!   typing yanks *my* viewport to the bottom while I am reading history —
//!   attributing their send to me.
//! - **Switching conversations replaces the whole vector.** Move to a
//!   conversation with more user rows and the count "grows", which reads as a
//!   send; move to one with fewer and it shrinks, which reads as *new content
//!   arrived while scrolled up* — a "↓ new messages" pill on a conversation the
//!   user just opened, with the viewport left wherever the previous transcript
//!   had it.
//!
//! So the predicate is stated directly instead of proxied: [`ListCursor::sends`]
//! is a counter this viewer's own send path owns, and conversation identity is
//! carried explicitly rather than inferred from the transcript shape.

/// What the message list should do with its scroll position after an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollAction {
    /// Jump to the bottom and resume following it.
    PinToBottom,
    /// Leave the viewport alone and raise the "↓ new messages" affordance:
    /// rows landed below where the user is reading.
    MarkUnseen,
    /// Leave the viewport alone and raise nothing — the transcript changed
    /// without gaining a row (a tool row settling, a bubble's text being
    /// rewritten by `finalize_answer`).
    Leave,
}

/// Everything the scroll decision is allowed to know about one observation of
/// the transcript.
///
/// Deliberately three scalars rather than the message vector: the decision is
/// about *edges between observations*, and taking the rows themselves would
/// invite exactly the "look at the last row's role" reasoning that misreads a
/// peer's message as the viewer's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ListCursor {
    /// Which conversation the list is showing. `None` before the first
    /// activation. Any change is a fresh view, not an update to the old one.
    pub conv: Option<u64>,
    /// How many messages **this viewer** has sent in this conversation —
    /// `ChatState::sends`, bumped only by `push_user_message`. A peer's row, a
    /// replayed history row and an archived plan capsule all leave it alone.
    pub sends: u64,
    /// Total rows in the transcript, so "gained a row" is distinguishable from
    /// "an existing row changed".
    pub rows: usize,
}

/// Decide what to do with the scroll position, given the previous observation,
/// the current one, and whether the user is still parked at the bottom.
///
/// Order matters and encodes the precedence: opening a conversation beats
/// everything (the user asked to look at it), the viewer's own send beats a
/// paused follow (they asked for the answer), and a paused follow beats new
/// content (they asked to keep their place).
#[must_use]
pub fn scroll_action(prev: ListCursor, next: ListCursor, stuck_to_bottom: bool) -> ScrollAction {
    // A different conversation is a different transcript; nothing about the
    // previous one's scroll state or unseen-row count carries over.
    if prev.conv != next.conv {
        return ScrollAction::PinToBottom;
    }
    // This viewer sent something. Monotone by construction, but compared with
    // `>` rather than `!=` so a reset (`clear_session`, a snapshot restoring an
    // older count) reads as "no send" instead of as one.
    if next.sends > prev.sends {
        return ScrollAction::PinToBottom;
    }
    if stuck_to_bottom {
        return ScrollAction::PinToBottom;
    }
    if next.rows > prev.rows {
        return ScrollAction::MarkUnseen;
    }
    ScrollAction::Leave
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cursor(conv: u64, sends: u64, rows: usize) -> ListCursor {
        ListCursor {
            conv: Some(conv),
            sends,
            rows,
        }
    }

    #[test]
    fn following_the_bottom_keeps_following() {
        assert_eq!(
            scroll_action(cursor(1, 0, 3), cursor(1, 0, 4), true),
            ScrollAction::PinToBottom
        );
    }

    #[test]
    fn my_own_send_re_arms_a_paused_follow() {
        // Scrolled up to re-read, then sent: the reply must not land off-screen.
        assert_eq!(
            scroll_action(cursor(1, 2, 5), cursor(1, 3, 6), false),
            ScrollAction::PinToBottom
        );
    }

    #[test]
    fn a_peers_message_does_not_yank_my_viewport() {
        // The regression this module exists for: a room peer's row is
        // `role == "user"` and grows the transcript, but it is not my send —
        // `sends` is untouched, so reading back is not interrupted.
        assert_eq!(
            scroll_action(cursor(1, 2, 5), cursor(1, 2, 6), false),
            ScrollAction::MarkUnseen
        );
    }

    #[test]
    fn opening_a_conversation_always_lands_at_the_bottom() {
        // Whichever direction the row/send counts move across the switch.
        assert_eq!(
            scroll_action(cursor(1, 7, 20), cursor(2, 1, 3), false),
            ScrollAction::PinToBottom
        );
        assert_eq!(
            scroll_action(cursor(2, 1, 3), cursor(1, 7, 20), false),
            ScrollAction::PinToBottom
        );
    }

    #[test]
    fn the_first_conversation_of_the_session_lands_at_the_bottom() {
        assert_eq!(
            scroll_action(ListCursor::default(), cursor(1, 0, 4), false),
            ScrollAction::PinToBottom
        );
    }

    #[test]
    fn a_shrinking_send_count_is_not_a_send() {
        // `clear_session` / a restored snapshot can lower the counter. Under
        // `!=` that would read as a send and steal the viewport.
        assert_eq!(
            scroll_action(cursor(1, 9, 20), cursor(1, 0, 0), false),
            ScrollAction::Leave
        );
    }

    #[test]
    fn an_in_place_change_raises_nothing() {
        // A tool row settling to ✓, or `finalize_answer` rewriting the trailing
        // bubble: the transcript changed, but nothing new landed below the
        // reader, so the pill must not appear.
        assert_eq!(
            scroll_action(cursor(1, 2, 6), cursor(1, 2, 6), false),
            ScrollAction::Leave
        );
    }
}
