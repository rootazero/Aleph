//! The one place a Slack API rejection becomes a [`ChannelError`].
//!
//! Slack answers every Web API call with `200 {"ok": false, "error": "..."}`
//! *or*, for tier limits, `429` plus that same body — so "is this a rate
//! limit?" has **two carriers**, and reading only one of them silently loses
//! the one class the delivery queue is built to survive. Before this module
//! the two faces of this adapter disagreed: `directory.rs` mapped
//! `"ratelimited"` to [`ChannelError::RateLimited`] while `message_ops::api`
//! folded the same string into [`ChannelError::SendFailed`] — and
//! `SendFailed` is refused by both `ChannelRegistry::send_attempt`'s
//! retry-after loop and `delivery_queue::should_enqueue`, so a rate-limited
//! reply was neither retried, nor queued, nor dead-lettered. It vanished.
//!
//! Only the *fallback* differs between the two faces (a failed read is
//! `ReceiveFailed`, a failed send is `SendFailed`); the classes that carry
//! delivery semantics are decided here, once.

use crate::gateway::channel::ChannelError;

/// How long to wait when Slack rate-limits us without saying for how long.
/// Slack's documented tier-3 window; formerly a per-face literal.
const DEFAULT_RETRY_AFTER_SECS: u64 = 30;

/// Which error a caller wants for the classes this module has no opinion on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlackFallback {
    /// A write path (`chat.postMessage`, `reactions.add`, uploads).
    Send,
    /// A read path (`conversations.list`, `users.list`).
    Receive,
}

impl SlackFallback {
    fn wrap(self, msg: String) -> ChannelError {
        match self {
            Self::Send => ChannelError::SendFailed(msg),
            Self::Receive => ChannelError::ReceiveFailed(msg),
        }
    }
}

/// Read `Retry-After` (seconds) from a Slack response, if present.
///
/// Slack sends it alongside the `429`; honouring the server's own number is
/// the difference between backing off correctly and guessing.
pub(crate) fn retry_after_secs(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
}

/// Classify one `{"ok": false}` Slack response.
///
/// `status` is the HTTP status when the caller kept it (`None` when it did
/// not); `err` is the body's `error` string. The rate-limit predicate is
/// **either carrier**, deliberately: Slack ships tier limits on `429` and, on
/// older endpoints, on a response whose body alone says `ratelimited`.
/// Requiring both would reintroduce exactly the loss this module exists to
/// stop.
pub(crate) fn classify_slack_error(
    ctx: &str,
    status: Option<reqwest::StatusCode>,
    err: &str,
    retry_after: Option<u64>,
    fallback: SlackFallback,
) -> ChannelError {
    if status == Some(reqwest::StatusCode::TOO_MANY_REQUESTS) || err == "ratelimited" {
        return ChannelError::RateLimited {
            retry_after_secs: retry_after.unwrap_or(DEFAULT_RETRY_AFTER_SECS),
        };
    }

    // Slack's error strings are the actionable part — a `missing_scope` here
    // is a workspace-admin fix, not a retry.
    match err {
        "invalid_auth" | "not_authed" | "account_inactive" => {
            ChannelError::AuthFailed(format!("Slack {ctx}: {err}"))
        }
        _ => fallback.wrap(format!("Slack {ctx} failed: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The defect this module was written for: the send face folded
    /// `"ratelimited"` into `SendFailed`, which `should_enqueue` refuses — so
    /// the reply was dropped instead of retried. Both faces now answer the
    /// same string with the same variant, and the queue agrees it is
    /// replayable.
    #[test]
    fn ratelimited_is_the_same_answer_on_both_faces_and_the_queue_accepts_it() {
        for fallback in [SlackFallback::Send, SlackFallback::Receive] {
            let err = classify_slack_error("chat.postMessage", None, "ratelimited", None, fallback);
            assert!(
                matches!(err, ChannelError::RateLimited { retry_after_secs } if retry_after_secs == DEFAULT_RETRY_AFTER_SECS),
                "{fallback:?} face must classify \"ratelimited\" as RateLimited, got {err:?}"
            );
            assert!(
                crate::gateway::delivery_queue::should_enqueue(&err),
                "a rate-limited Slack rejection must be durable-queue eligible"
            );
        }
    }

    /// The other carrier: a `429` whose body string is something else is still
    /// a rate limit, and the server's own `Retry-After` wins over the default.
    #[test]
    fn a_429_is_a_rate_limit_even_when_the_body_string_is_not() {
        let err = classify_slack_error(
            "chat.postMessage",
            Some(reqwest::StatusCode::TOO_MANY_REQUESTS),
            "some_other_error",
            Some(7),
            SlackFallback::Send,
        );
        assert!(
            matches!(
                err,
                ChannelError::RateLimited {
                    retry_after_secs: 7
                }
            ),
            "429 + Retry-After must be honoured verbatim, got {err:?}"
        );
    }

    /// A plain `4xx` must NOT be read as a rate limit — that would retry
    /// malformed calls until the budget is gone.
    #[test]
    fn an_ordinary_rejection_keeps_its_faces_fallback() {
        let send = classify_slack_error(
            "chat.postMessage",
            Some(reqwest::StatusCode::BAD_REQUEST),
            "channel_not_found",
            None,
            SlackFallback::Send,
        );
        assert!(matches!(send, ChannelError::SendFailed(_)), "{send:?}");

        let recv = classify_slack_error(
            "users.list",
            None,
            "missing_scope",
            None,
            SlackFallback::Receive,
        );
        assert!(matches!(recv, ChannelError::ReceiveFailed(_)), "{recv:?}");

        let auth = classify_slack_error(
            "users.list",
            None,
            "invalid_auth",
            None,
            SlackFallback::Receive,
        );
        assert!(matches!(auth, ChannelError::AuthFailed(_)), "{auth:?}");
    }
}
