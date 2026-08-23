//! The Panel's user-facing failure taxonomy.
//!
//! Populated from the server's `error_code` (`aleph_protocol::receipt`), with
//! the keyword classifier kept only for inputs that genuinely carry no code.

use serde::{Deserialize, Serialize};

/// Stable, machine-readable code for a chat send / delivery failure.
///
/// Mirrors openhuman's `chatSendError.ts` taxonomy so analytics and tests
/// can branch on a small fixed set instead of substring-matching messages.
/// New variants only — never rename or repurpose existing ones (wire-stable).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatSendErrorCode {
    /// WebSocket dropped or never established.
    SocketDisconnected,
    /// Cloud provider rejected the send (HTTP error, rate limit, etc.).
    CloudSendFailed,
    /// Server-side safety pipeline blocked the prompt.
    PromptBlocked,
    /// Server flagged the prompt for review (soft warning).
    PromptReview,
    /// Usage limit / quota reached.
    UsageLimitReached,
    /// Run aborted due to a safety timeout.
    SafetyTimeout,
    /// The composer refused the send before it left the client — the input is
    /// not supported on this surface (e.g. attachments in team group chat).
    /// Distinct from the server-side codes above: nothing was transmitted, and
    /// the user can fix it and retry immediately.
    Unsupported,
    /// Catch-all for unmapped errors. Use the message field for context.
    Unknown,
    /// The user stopped the run — `CANCELLED`. Not a failure; surfaces should
    /// not raise an error banner for it (the stopped bubble already says so).
    Cancelled,
    /// The session was busy and the message never ran — `AGENT_BUSY`.
    /// Includes a queued message rejected at the lane cap or timed out.
    AgentBusy,
    /// Credential rejected — `AUTH`. Retrying will not help; the user must fix
    /// the key, so this must never be worded as "try again".
    Auth,
    /// A spend ceiling was reached — `SPEND_EXHAUSTED`.
    SpendExhausted,
}

/// Structured chat send error — preferred over the legacy bare
/// `error_message` string. Both are populated in lock-step so existing
/// readers keep working unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatSendError {
    pub code: ChatSendErrorCode,
    pub message: String,
}

impl ChatSendError {
    pub fn new(code: ChatSendErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// Build from the server's classification, falling back to the keyword
    /// classifier only when there is genuinely no code to read.
    ///
    /// # Why the code wins
    ///
    /// The server already picked a bucket (`gateway::i18n::ReceiptKind`) from
    /// a *typed* error and put its stable spelling on the wire. Re-deriving a
    /// bucket from the rendered message is the same defect the server deleted
    /// from `inbound_router::executor`, one crate over — and it is why Stop, a
    /// rejected queued message, and an expired API key all rendered as an
    /// UNKNOWN banner: those buckets had no keyword branch at all.
    ///
    /// An unrecognized spelling maps to [`ChatSendErrorCode::Unknown`] rather
    /// than falling through to the classifier. A newer core naming a bucket
    /// this build has not heard of has still *answered*; guessing a different
    /// answer from its prose is the behaviour being removed.
    #[must_use]
    pub fn from_wire_code(code: Option<&str>, message: impl Into<String>) -> Self {
        let message = message.into();
        let Some(code) = code else {
            return Self::classify(message);
        };
        let mapped = match aleph_protocol::receipt::ReceiptCode::from_wire(code) {
            Some(c) => Self::from_receipt_code(c),
            None => ChatSendErrorCode::Unknown,
        };
        Self {
            code: mapped,
            message,
        }
    }

    /// Total map from the shared protocol bucket. Exhaustive on purpose: a
    /// bucket added to `ReceiptCode` is a compile error here, not a silent
    /// `Unknown`.
    #[must_use]
    const fn from_receipt_code(code: aleph_protocol::receipt::ReceiptCode) -> ChatSendErrorCode {
        use aleph_protocol::receipt::ReceiptCode as C;
        match code {
            C::Timeout => ChatSendErrorCode::SafetyTimeout,
            C::Cancelled => ChatSendErrorCode::Cancelled,
            C::AgentBusy => ChatSendErrorCode::AgentBusy,
            C::RateLimited => ChatSendErrorCode::UsageLimitReached,
            C::Auth => ChatSendErrorCode::Auth,
            C::ProvidersUnreachable => ChatSendErrorCode::CloudSendFailed,
            C::Failed => ChatSendErrorCode::CloudSendFailed,
            C::SpendExhausted => ChatSendErrorCode::SpendExhausted,
        }
    }

    /// **Fallback only** — prefer [`Self::from_wire_code`]. Reachable for a
    /// core that predates `error_code` and for transport-layer failures raised
    /// before any run exists. Its keyword table cannot see `CANCELLED`,
    /// `AGENT_BUSY`, `AUTH`, or `SPEND_EXHAUSTED`; that is why it is not the
    /// first classifier any more.
    ///
    /// Heuristic classifier — maps an opaque error string to a code so the
    /// existing `ChatApi::send` error path can produce structured errors
    /// without a wire-format change. Order matters (most specific first).
    pub fn classify(msg: impl Into<String>) -> Self {
        let message = msg.into();
        let l = message.to_lowercase();
        let code =
            if l.contains("disconnect") || l.contains("not connected") || l.contains("websocket") {
                ChatSendErrorCode::SocketDisconnected
            } else if l.contains("prompt_blocked") || l.contains("prompt injection") {
                ChatSendErrorCode::PromptBlocked
            } else if l.contains("prompt_review") {
                ChatSendErrorCode::PromptReview
            } else if l.contains("usage limit") || l.contains("quota") || l.contains("rate limit") {
                ChatSendErrorCode::UsageLimitReached
            } else if l.contains("safety timeout")
                || l.contains("turn timeout")
                || l.contains("stalled after")
            {
                // Harness-side watchdogs (TerminateReason::TurnTimeout /
                // StallTimeout humanized text) — the run itself was killed.
                ChatSendErrorCode::SafetyTimeout
            } else if l.contains("timed out")
                || l.contains("cloud")
                || l.contains("http")
                || l.contains("provider")
            {
                // "Request timed out" comes from the provider transport
                // (connect/TTFB/stream-idle), not the harness — an upstream
                // delivery failure, so it belongs with CloudSendFailed.
                ChatSendErrorCode::CloudSendFailed
            } else {
                ChatSendErrorCode::Unknown
            };
        Self { code, message }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The server already classified this failure and put a stable code on the
    /// wire. Re-deriving it from the message is how the Panel ended up with a
    /// taxonomy that had no bucket for Stop, for a rejected queued message, or
    /// for an expired API key — all three rendered as an UNKNOWN banner.
    #[test]
    fn the_wire_code_wins_over_the_message_text() {
        // A message whose text would keyword-match CloudSendFailed.
        let e = ChatSendError::from_wire_code(Some("CANCELLED"), "http provider stopped");
        assert_eq!(e.code, ChatSendErrorCode::Cancelled);

        let e = ChatSendError::from_wire_code(Some("AGENT_BUSY"), "session is occupied");
        assert_eq!(e.code, ChatSendErrorCode::AgentBusy);

        let e = ChatSendError::from_wire_code(Some("AUTH"), "http 401 from provider");
        assert_eq!(
            e.code,
            ChatSendErrorCode::Auth,
            "an expired key must never render as a retryable cloud failure"
        );
    }

    /// Every bucket the server can send must land somewhere real. Derived from
    /// `ReceiptCode::ALL` rather than a literal list, so a bucket added
    /// server-side fails here instead of silently becoming Unknown.
    #[test]
    fn every_server_bucket_maps_to_something_other_than_unknown() {
        for code in aleph_protocol::receipt::ReceiptCode::ALL {
            let mapped = ChatSendError::from_wire_code(Some(code.as_wire()), "msg").code;
            assert_ne!(
                mapped,
                ChatSendErrorCode::Unknown,
                "{} has no Panel bucket — it would render as an UNKNOWN banner",
                code.as_wire()
            );
        }
    }

    /// `classify` is the fallback, not the classifier: a core that predates
    /// `error_code`, and the transport-layer errors `ChatApi::send` raises
    /// before any run exists, both arrive without one.
    #[test]
    fn a_missing_code_falls_back_to_the_keyword_classifier() {
        let e = ChatSendError::from_wire_code(None, "websocket disconnected");
        assert_eq!(e.code, ChatSendErrorCode::SocketDisconnected);
        assert_eq!(e.message, "websocket disconnected");
    }

    /// An unknown spelling is not a licence to guess. A newer core sending a
    /// bucket this build has never heard of must not be re-derived from prose.
    #[test]
    fn an_unrecognized_code_is_unknown_not_a_keyword_guess() {
        let e = ChatSendError::from_wire_code(Some("BRAND_NEW_BUCKET"), "rate limit exceeded");
        assert_eq!(
            e.code,
            ChatSendErrorCode::Unknown,
            "the server named a bucket; guessing a different one from the text \
             is exactly the defect this replaces"
        );
    }
}
