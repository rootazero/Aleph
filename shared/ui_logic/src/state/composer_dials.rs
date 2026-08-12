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
//! * The **plan phase** follows the mode's rule and needs it more than the mode
//!   does. An approved plan handoff writes `building` onto the session from the
//!   *server* side, mid-conversation, with no request of the client's involved —
//!   so a composer that re-asserted its cached `planning` on the next message
//!   would undo the approval it had just watched the user give, and the read-only
//!   floor would snap back on with the work half done. Live sessions therefore
//!   carry nothing and read the phase back from the session row.
//!
//! Lives here because two composers (wide and phone) have to agree on it and
//! neither can be host-tested through Leptos. The phone composer used to carry
//! neither dial at all — it passed `None` for both — so a phone user could not
//! arm a tier or a mode for the one turn the pickers exist for.

/// What a send carries for the three per-session dials, given the pills'
/// current values and whether the conversation already has a session row.
///
/// A struct rather than a tuple from three onwards: `(Option<String>,
/// Option<String>, Option<String>)` is three identical types in a row, and the
/// call sites that destructure it are in two crates.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SendDials {
    /// Re-armed on every send: a session row does not out-rank the pill.
    pub exec_tier: Option<String>,
    /// First send only — the store is authoritative once it exists.
    pub session_mode: Option<String>,
    /// First send only, for a stronger reason than the mode's. See the module
    /// doc.
    pub plan_phase: Option<String>,
}

/// The dials a send should carry. `session_exists` is `session_key.is_some()`.
#[must_use]
pub fn session_dials_for_send(
    session_exists: bool,
    pill_tier: Option<String>,
    pill_mode: Option<String>,
    pill_plan_phase: Option<String>,
) -> SendDials {
    SendDials {
        exec_tier: pill_tier,
        session_mode: if session_exists { None } else { pill_mode },
        plan_phase: if session_exists {
            None
        } else {
            pill_plan_phase
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> Option<String> {
        Some(v.to_string())
    }

    #[test]
    fn first_send_carries_every_dial() {
        // No session row yet: this is the only chance any pill has to govern
        // the turn it was armed for.
        let d = session_dials_for_send(false, s("full"), s("code"), s("planning"));
        assert_eq!(d.exec_tier.as_deref(), Some("full"));
        assert_eq!(d.session_mode.as_deref(), Some("code"));
        assert_eq!(d.plan_phase.as_deref(), Some("planning"));
    }

    #[test]
    fn a_live_session_still_carries_the_tier() {
        // The tier is re-armed per send; a session row does not out-rank it.
        let d = session_dials_for_send(true, s("ask"), None, None);
        assert_eq!(d.exec_tier.as_deref(), Some("ask"));
    }

    #[test]
    fn a_live_session_drops_the_mode() {
        // The store is authoritative once it exists — re-sending the pill's
        // cached mode would revert a mid-conversation `session_set_mode`.
        let d = session_dials_for_send(true, None, s("chat"), None);
        assert_eq!(d.session_mode, None);
    }

    #[test]
    fn a_live_session_drops_the_plan_phase() {
        // The one that matters most: an approved handoff writes `building` on
        // the server. A composer re-asserting its cached `planning` here would
        // undo the approval the user just gave and re-engage the read-only
        // floor with the work half done.
        let d = session_dials_for_send(true, None, None, s("planning"));
        assert_eq!(d.plan_phase, None);
    }

    #[test]
    fn follow_global_carries_nothing() {
        // Every pill on "follow global" = no override to carry, on either side
        // of the session boundary.
        assert_eq!(
            session_dials_for_send(false, None, None, None),
            SendDials::default()
        );
        assert_eq!(
            session_dials_for_send(true, None, None, None),
            SendDials::default()
        );
    }
}
