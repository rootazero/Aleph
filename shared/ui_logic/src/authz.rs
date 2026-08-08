//! Reading a server refusal for what it is.
//!
//! ## The shape this exists to stop
//!
//! A client that collapses "you may not" into "there is none" does not merely
//! render badly — it makes a confident false statement, and it makes it to the
//! person least able to check it. Every instance found in the Panel on
//! 2026-08-08 had the same three lines:
//!
//! ```ignore
//! match SomeApi::list(&state).await {
//!     Ok(list) => ready.set(Some(!list.is_empty())),
//!     Err(_)   => ready.set(Some(false)),   // ← "denied" read as "unconfigured"
//! }
//! ```
//!
//! - The Quick Setup checklist told a member `PENDING Configure a chat provider`
//!   on a server with providers configured, and invited them into a settings
//!   page they cannot use.
//! - The composer's tier and mode pills silently lost every option but "follow
//!   global", with a `console.warn` as the only trace.
//! - The cluster page reused its *read* failure copy for an *enroll* failure,
//!   telling the user it could not read a topology they had not asked it to read.
//!
//! None of these is a leak — the server refused correctly in every case. They
//! are honesty bugs, and they are worth fixing precisely because the standing
//! architectural ruling is that the Panel must NOT gate on role
//! (a role is latched at `connect`, `restamp_live_connections` changes it
//! without telling the client, so a UI gate can never be an enforcement point).
//! A client that will not gate must instead **report accurately**, and that
//! means telling a refusal apart from an empty answer.
//!
//! ## Why a shared predicate and not `err.contains(...)` at each site
//!
//! The recognisable part of the refusal is its message text: the Panel's RPC
//! layer keeps `error.message` and drops the code. Matching that text against
//! [`ADMIN_REQUIRED_MESSAGE`] — the same `aleph_protocol` constant the server
//! emits, never a copy — means a reword moves both sides in one edit. That
//! discipline already existed at exactly one site (`fleet_error_label` in the
//! cluster page) and nowhere else; this module is that site's predicate, hoisted
//! so the other callers stop inventing their own answer.

use aleph_protocol::jsonrpc::ADMIN_REQUIRED_MESSAGE;

/// Did this RPC fail because the caller lacks operator privileges?
///
/// `false` for every other failure — a transport drop, a malformed response, a
/// genuine server error. Callers must keep treating those as errors: conflating
/// "not permitted" with "broken" is the same class of mistake in the other
/// direction.
#[must_use]
pub fn is_admin_required(err: &str) -> bool {
    err.contains(ADMIN_REQUIRED_MESSAGE)
}

/// What a client knows about a value it tried to read.
///
/// Three states, because two are not enough: a checklist row that can only say
/// "ready" or "pending" has no way to say "not yours to configure", so it says
/// "pending" and lies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readable<T> {
    /// Not asked yet, or the socket is down.
    Unknown,
    /// The server answered.
    Known(T),
    /// The server refused: this caller is not an operator.
    Restricted,
}

impl<T> Readable<T> {
    /// Classify an RPC result into the three states.
    ///
    /// A non-authorization error stays [`Self::Unknown`] rather than becoming a
    /// value: "we could not find out" is the honest reading of a failed read,
    /// and it is what leaves the door open for a retry on reconnect.
    pub fn from_result<E: AsRef<str>>(result: Result<T, E>) -> Self {
        match result {
            Ok(v) => Self::Known(v),
            Err(e) if is_admin_required(e.as_ref()) => Self::Restricted,
            Err(_) => Self::Unknown,
        }
    }

    /// `true` only when the server actually refused on authorization grounds.
    #[must_use]
    pub const fn is_restricted(&self) -> bool {
        matches!(self, Self::Restricted)
    }

    /// The value, if the server answered.
    pub const fn known(&self) -> Option<&T> {
        match self {
            Self::Known(v) => Some(v),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_servers_own_refusal_is_recognised() {
        assert!(is_admin_required(ADMIN_REQUIRED_MESSAGE));
        // The Panel's RPC layer prefixes/wraps the message in places; a
        // substring match is what `fleet_error_label` has always done.
        assert!(is_admin_required(&format!(
            "RPC error: {ADMIN_REQUIRED_MESSAGE}"
        )));
    }

    /// The half that matters most. If an ordinary failure were classified as a
    /// refusal, every transport blip would render as "you lack permission" —
    /// the same false-confidence bug pointing the other way.
    #[test]
    fn an_ordinary_failure_is_not_a_refusal() {
        for err in [
            "Not connected",
            "timeout",
            "invalid params",
            "Session not found",
            "",
        ] {
            assert!(!is_admin_required(err), "{err:?} is not an authz refusal");
        }
    }

    #[test]
    fn a_failed_read_is_unknown_not_a_value() {
        let denied: Readable<bool> =
            Readable::from_result(Err::<bool, &str>(ADMIN_REQUIRED_MESSAGE));
        assert_eq!(denied, Readable::Restricted);
        assert!(denied.is_restricted());
        assert_eq!(denied.known(), None);

        let dropped: Readable<bool> = Readable::from_result(Err::<bool, &str>("Not connected"));
        assert_eq!(
            dropped,
            Readable::Unknown,
            "a transport failure must not become a value — that is exactly how \
             `Err(_) => Some(false)` told a member their provider was unconfigured"
        );

        assert_eq!(
            Readable::from_result(Ok::<bool, &str>(true)),
            Readable::Known(true)
        );
    }
}
