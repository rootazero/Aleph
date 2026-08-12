// Classifying a failed gateway call, so a surface can say what happened rather
// than answer on the server's behalf.
//
// The `providers.*` family is admin-gated, so every call in it can come back
// refused on a member connection. A refusal is NOT an empty answer, and folding
// one into the other makes this client state something the server never said —
// the launch caption did exactly that, degrading a refused `providers.list` to
// the literal "unknown", which reads as "your gateway has no model configured".
// Only `Ok` may assert anything about the thing being read; every flavour of
// `Err` can say no more than "I do not know".

use aleph_client::CliError;
use aleph_protocol::jsonrpc::ADMIN_REQUIRED_MESSAGE;

/// Why a gateway call produced no answer.
///
/// Two variants, not three: the operator gate is the one refusal a client can
/// act on (link a device with more privilege), and everything else — a dropped
/// socket, a deadline, a malformed reply, some other server error — leaves us
/// equally ignorant and gets the server's own words passed through instead of
/// copy we invented for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CallFailure {
    /// The gateway refused: this connection is not an operator.
    Restricted,
    /// No usable answer, for any other reason.
    Unavailable,
}

impl CallFailure {
    /// One word for a status-bar cell, where a sentence does not fit.
    ///
    /// Neither word is a value the server reported — both say "I could not
    /// ask", which is the distinction the caption exists to keep.
    pub(super) const fn caption(self) -> &'static str {
        match self {
            Self::Restricted => "restricted",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Recognise the gateway's operator-privilege refusal.
///
/// Matched through [`ADMIN_REQUIRED_MESSAGE`] — the constant the **server**
/// emits, from the crate both sides depend on — so a reword moves the gate and
/// this classifier in one edit instead of stranding members on raw English.
pub(super) fn classify(err: &CliError) -> CallFailure {
    match err {
        CliError::Rpc { message, .. } if message.contains(ADMIN_REQUIRED_MESSAGE) => {
            CallFailure::Restricted
        }
        _ => CallFailure::Unavailable,
    }
}

/// One line naming what could not be read, and why.
///
/// `subject` is the caller's words, because only the caller knows what was
/// being attempted: the same refusal answers a catalogue read and a fleet
/// enrolment, and one sentence cannot honestly describe both. Every non-refusal
/// passes through verbatim — a timeout is not a permission verdict, and
/// inventing copy for it would put a guess in front of the cause.
pub(super) fn explain(err: &CliError, subject: &str) -> String {
    match classify(err) {
        CallFailure::Restricted => format!(
            "Could not read {subject}: this connection is not an operator, and this is an \
             operator-only method. Nothing is being claimed about what is configured."
        ),
        CallFailure::Unavailable => format!("Could not read {subject}: {err}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::jsonrpc::AUTH_REQUIRED;

    /// The refusal is recognised from the server's own constant, not from a
    /// sentence copied into this crate.
    #[test]
    fn the_operator_gate_is_restricted_and_everything_else_is_not() {
        let refusal = CliError::Rpc {
            code: AUTH_REQUIRED,
            message: ADMIN_REQUIRED_MESSAGE.to_string(),
        };
        assert_eq!(classify(&refusal), CallFailure::Restricted);

        for other in [
            CliError::Timeout("read timeout".into()),
            CliError::Disconnected("socket closed".into()),
            CliError::Rpc {
                code: -32602,
                message: "invalid params: view".into(),
            },
        ] {
            assert_eq!(
                classify(&other),
                CallFailure::Unavailable,
                "{other} is not a permission verdict"
            );
        }
    }

    /// A refusal never borrows the server's wording (there is none to borrow
    /// that a user can act on); anything else is quoted rather than paraphrased.
    #[test]
    fn a_non_refusal_reaches_the_user_verbatim() {
        let err = CliError::Timeout("no response in 30s".into());
        let line = explain(&err, "the provider catalogue");
        assert!(line.contains("the provider catalogue"));
        assert!(line.contains("no response in 30s"));

        let refusal = CliError::Rpc {
            code: AUTH_REQUIRED,
            message: ADMIN_REQUIRED_MESSAGE.to_string(),
        };
        let line = explain(&refusal, "the provider catalogue");
        assert!(line.contains("not an operator"));
        // The caption and the sentence must not disagree about which case this
        // is — they are read side by side.
        assert_eq!(classify(&refusal).caption(), "restricted");
    }
}
