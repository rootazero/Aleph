//! Stable wire codes for user-facing failure receipts.
//!
//! One source for the classification both halves of the system already need:
//! the server picks a bucket (`gateway::i18n::ReceiptKind`) and puts its code
//! on `StreamEvent::RunError.error_code`; the Panel has to render that bucket.
//!
//! Before this module existed the Panel did not read the code at all — it
//! lower-cased the message and keyword-matched its way to a *second*, smaller
//! taxonomy with no bucket for `CANCELLED`, `AGENT_BUSY`, `AUTH`, or
//! `SPEND_EXHAUSTED`. That is the same defect the server deleted from
//! `inbound_router::executor` (re-classifying an already-typed error from its
//! string), one crate over.
//!
//! Living here rather than in either side is what makes a rename a compile
//! error instead of a silent reclassification: `aleph-protocol` is depended on
//! by both `alephcore` and `aleph-panel`.

use serde::{Deserialize, Serialize};

/// A user-facing failure bucket, as carried on the wire.
///
/// The `as_wire` strings are API. New variants may be added; **existing
/// spellings may never change** — a shipped client switches on them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReceiptCode {
    /// The run exceeded its wall-clock budget.
    Timeout,
    /// The user (or an `Interrupt`-mode message) stopped the run.
    Cancelled,
    /// The session was busy and the message could not be steered or queued —
    /// including a queued message rejected at the lane cap or timed out
    /// waiting.
    AgentBusy,
    /// Every provider in the chain reported rate limiting.
    RateLimited,
    /// Credential rejected (401 / invalid API key). Retrying will not help,
    /// so a surface that renders this as "try again" is actively misleading.
    Auth,
    /// Network / upstream outage across the whole provider chain.
    ProvidersUnreachable,
    /// Anything else. Deliberately opaque — the raw chain stays in the server
    /// log and never reaches the wire.
    Failed,
    /// A spend ceiling was reached.
    SpendExhausted,
}

impl ReceiptCode {
    /// Every variant. Both sides derive their reconciliation expectations from
    /// this rather than restating a literal list, so a new bucket cannot be
    /// added on one side only.
    pub const ALL: &'static [Self] = &[
        Self::Timeout,
        Self::Cancelled,
        Self::AgentBusy,
        Self::RateLimited,
        Self::Auth,
        Self::ProvidersUnreachable,
        Self::Failed,
        Self::SpendExhausted,
    ];

    /// The stable wire spelling. **Never rename an existing one.**
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Timeout => "TIMEOUT",
            Self::Cancelled => "CANCELLED",
            Self::AgentBusy => "AGENT_BUSY",
            Self::RateLimited => "RATE_LIMITED",
            Self::Auth => "AUTH",
            Self::ProvidersUnreachable => "PROVIDERS_UNREACHABLE",
            Self::Failed => "FAILED",
            Self::SpendExhausted => "SPEND_EXHAUSTED",
        }
    }

    /// Parse a wire code. `None` for anything this build does not know —
    /// a newer core may send a bucket an older client has never heard of, and
    /// guessing is what this module exists to stop.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.as_wire() == s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire strings are API — a client switches on them. Renaming one
    /// silently reclassifies every error of that kind on every surface that
    /// has already shipped.
    #[test]
    fn wire_spellings_are_frozen() {
        assert_eq!(ReceiptCode::Timeout.as_wire(), "TIMEOUT");
        assert_eq!(ReceiptCode::Cancelled.as_wire(), "CANCELLED");
        assert_eq!(ReceiptCode::AgentBusy.as_wire(), "AGENT_BUSY");
        assert_eq!(ReceiptCode::RateLimited.as_wire(), "RATE_LIMITED");
        assert_eq!(ReceiptCode::Auth.as_wire(), "AUTH");
        // NOTE the asymmetry: the variant is `ProvidersUnreachable` and the
        // wire code is `PROVIDERS_UNREACHABLE`, but the server-side variant it
        // mirrors is named `Unreachable`. The wire string is the contract.
        assert_eq!(
            ReceiptCode::ProvidersUnreachable.as_wire(),
            "PROVIDERS_UNREACHABLE"
        );
        assert_eq!(ReceiptCode::Failed.as_wire(), "FAILED");
        assert_eq!(ReceiptCode::SpendExhausted.as_wire(), "SPEND_EXHAUSTED");
    }

    /// `ALL` is what both sides derive their expectations from, so a variant
    /// missing from it is a variant no reconciliation test can see.
    #[test]
    fn all_covers_every_variant_and_round_trips() {
        assert_eq!(ReceiptCode::ALL.len(), 8);
        for code in ReceiptCode::ALL {
            assert_eq!(
                ReceiptCode::from_wire(code.as_wire()),
                Some(*code),
                "{} does not round-trip",
                code.as_wire()
            );
        }
        assert_eq!(ReceiptCode::from_wire("NOT_A_CODE"), None);
    }
}
