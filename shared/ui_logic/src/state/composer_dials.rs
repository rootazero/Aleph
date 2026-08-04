//! What a composer send carries for the two per-session dials.
//!
//! The exec tier (Ask / Auto / Full) and the session mode (chat / work / code)
//! are both picked in the composer and both stored on the session — but they
//! ride a send under *different* rules, and getting the difference wrong fails
//! silently in opposite directions:
//!
//! * Drop the **tier** on a send and the very first turn of a new conversation
//!   runs under the global tier, not the one the user just armed. There is no
//!   session row yet to have been patched, so the pill's value can only reach
//!   the run by riding the message.
//! * Keep carrying the **mode** after a session exists and the pill's cached
//!   value out-ranks the store — silently reverting a `session_set_mode` the
//!   model made mid-conversation. Once a session row exists it is authoritative.
//!
//! Lives here because two composers (wide and phone) have to agree on it and
//! neither can be host-tested through Leptos. The phone composer used to carry
//! neither dial at all — it passed `None` for both — so a phone user could not
//! arm a tier or a mode for the one turn the pickers exist for.

/// The `(exec_tier, session_mode)` pair a send should carry, given the pills'
/// current values and whether the conversation already has a session row.
///
/// `session_exists` is `session_key.is_some()`.
#[must_use]
pub fn session_dials_for_send(
    session_exists: bool,
    pill_tier: Option<String>,
    pill_mode: Option<String>,
) -> (Option<String>, Option<String>) {
    let mode = if session_exists { None } else { pill_mode };
    (pill_tier, mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> Option<String> {
        Some(v.to_string())
    }

    #[test]
    fn first_send_carries_both_dials() {
        // No session row yet: this is the only chance either pill has to
        // govern the turn it was armed for.
        let (tier, mode) = session_dials_for_send(false, s("full"), s("code"));
        assert_eq!(tier.as_deref(), Some("full"));
        assert_eq!(mode.as_deref(), Some("code"));
    }

    #[test]
    fn a_live_session_still_carries_the_tier() {
        // The tier is re-armed per send; a session row does not out-rank it.
        let (tier, _) = session_dials_for_send(true, s("ask"), None);
        assert_eq!(tier.as_deref(), Some("ask"));
    }

    #[test]
    fn a_live_session_drops_the_mode() {
        // The store is authoritative once it exists — re-sending the pill's
        // cached mode would revert a mid-conversation `session_set_mode`.
        let (_, mode) = session_dials_for_send(true, None, s("chat"));
        assert_eq!(mode, None);
    }

    #[test]
    fn follow_global_carries_nothing() {
        // Both pills on "follow global" = no override to carry, on either
        // side of the session boundary.
        assert_eq!(session_dials_for_send(false, None, None), (None, None));
        assert_eq!(session_dials_for_send(true, None, None), (None, None));
    }
}
