//! Recognising the server's "this needs operator privilege" refusal, so a
//! surface can explain it instead of guessing about it.
//!
//! # The failure this exists to stop
//!
//! Every one of these calls can come back refused for a member, and each place
//! that handles the refusal was inventing its own meaning for it:
//!
//! | Surface | What it did with the refusal | What the user read |
//! |---|---|---|
//! | Quick Setup checklist | `Err(_) => ready.set(Some(false))` | "PENDING — Configure a chat provider" for a provider that IS configured |
//! | Exec-tier pill | `Err(e) => console::warn` | a popover with one blank-labelled row |
//! | Session-mode pill | `Err(e) => console::warn` | no pill at all |
//! | ~20 settings pages | `format!("Failed to load X: {e}")` | a raw English protocol string |
//!
//! Three of those four are the same mistake in different clothes: **a refused
//! read was read as a VALUE.** "I am not allowed to know" became "the answer is
//! no / empty / zero". The first row is the expensive one, because a confident
//! false statement costs more than a blank.
//!
//! # Why this is not a permission check
//!
//! Nothing here decides anything. The Panel deliberately holds no client-side
//! role predicate — `DashboardState::is_operator()` was deleted on 2026-08-07
//! because a role captured at `connect` is wrong in both directions after
//! `handlers::users::restamp_live_connections` re-stamps a live connection, and
//! `cluster.rs` carries a source-level pin against its return. Authorization is
//! server-side (`method_admin.rs` for RPCs, `event_scope.rs` for topics); a
//! surface a member may not use learns so from the refusal it gets back, and
//! this module is only about saying that out loud.
//!
//! So: render, call, and report what the server said. Do not use
//! [`is_admin_refusal`] to pre-emptively hide a page — that is the same gate
//! under a new name.
//!
//! # Single source
//!
//! The gate refuses with `AUTH_REQUIRED` plus a fixed message, but the Panel's
//! RPC layer keeps only `error.message` (the code is dropped in `context.rs`'s
//! response arm), so the text is the only recognisable part. It is matched
//! through [`ADMIN_REQUIRED_MESSAGE`] — **the same `aleph_protocol` constant
//! the server emits** — so a reword moves the server and every consumer here in
//! one edit, instead of stranding members on the raw English string.

use aleph_protocol::jsonrpc::ADMIN_REQUIRED_MESSAGE;
use leptos_i18n::I18nContext;

use crate::i18n::{t_string, Locale};

/// Whether this error is the gateway's operator-privilege refusal.
///
/// Use it to tell "refused" apart from "the answer is empty". A checklist step,
/// a picker's option list and a page's contents all have a legitimate empty
/// state, and none of them mean the same thing as this.
#[must_use]
pub fn is_admin_refusal(err: &str) -> bool {
    err.contains(ADMIN_REQUIRED_MESSAGE)
}

/// Copy for a failed call: the operator-privilege refusal is replaced by
/// `explanation`, and **every other error passes through verbatim**.
///
/// The pass-through is the important half. A transport failure, a store error
/// and a malformed response are not permission verdicts, and inventing copy for
/// them would put a guess in front of the user instead of the cause — degraded
/// copy beats a wrong claim.
///
/// `explanation` is the caller's, because only the caller knows what was being
/// attempted; a refused fleet READ and a refused node ENROLMENT are the same
/// verdict about two different actions, and one sentence cannot honestly
/// describe both.
#[must_use]
pub fn labeled(err: &str, explanation: &str) -> String {
    if is_admin_refusal(err) {
        explanation.to_string()
    } else {
        err.to_string()
    }
}

/// What a settings page should display when its load failed.
///
/// A refusal becomes the localized explanation; **every other failure keeps the
/// page's own framing** — that is what `frame` is for, and why this takes a
/// closure instead of a pre-formatted string: `"Failed to load MCP servers: …"`
/// is useful context for a store error and an actively misleading preamble for
/// a permission verdict, which did not fail to load anything, it declined.
///
/// This is copy, not a gate. The page still renders, still calls, and still
/// reports what came back — see this module's doc for why it must not start
/// hiding itself instead.
pub fn settings_load_error(
    i18n: I18nContext<Locale>,
    err: &str,
    frame: impl FnOnce(&str) -> String,
) -> String {
    if is_admin_refusal(err) {
        t_string!(i18n, settings.admin_refusal.read_config).to_string()
    } else {
        frame(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fed the SERVER's own refusal — `aleph_protocol`'s constant, which
    /// `gateway::server::handler` emits verbatim — not a local transcription of
    /// it. That is what makes this able to fail: if recognition ever drifts
    /// from the words the server actually sends, the refusal falls through.
    #[test]
    fn the_servers_own_refusal_is_recognised() {
        assert!(is_admin_refusal(ADMIN_REQUIRED_MESSAGE));
        // Handlers that wrap it in their own framing still match.
        assert!(is_admin_refusal(&format!(
            "Failed to load MCP servers: {ADMIN_REQUIRED_MESSAGE}"
        )));
    }

    #[test]
    fn every_other_failure_is_not_a_permission_verdict() {
        for raw in [
            "Invalid response: missing environments",
            "WebSocket disconnected",
            "Internal error: failed to read enrolled node devices",
            "Not connected",
        ] {
            assert!(!is_admin_refusal(raw), "{raw} must not read as a refusal");
            assert_eq!(labeled(raw, "explained"), raw, "{raw} must pass through");
        }
    }

    #[test]
    fn a_refusal_is_explained_in_the_callers_own_terms() {
        assert_eq!(
            labeled(ADMIN_REQUIRED_MESSAGE, "cannot enrol a node"),
            "cannot enrol a node"
        );
        assert_ne!(
            labeled(ADMIN_REQUIRED_MESSAGE, "cannot enrol a node"),
            ADMIN_REQUIRED_MESSAGE,
            "the refusal must be explained, not echoed as a bare protocol string"
        );
    }
}
