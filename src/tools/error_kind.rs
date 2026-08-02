//! `ToolErrorKind` — stable taxonomy over `ToolError` for prompt rendering.
//!
//! The `ToolError` enum already discriminates the *Aleph-internal* error
//! shapes (Timeout, Transport, `ValidationFailed`, …) but a tool that
//! fails on a network call typically lands in `Execution { cause }`
//! with the upstream status code embedded in the cause string. The LLM
//! sees the raw cause and has to infer "did this fail because of auth?
//! rate-limit? policy block?" before it can pick an alternative method.
//!
//! `ToolErrorKind` is a coarse, stable classification we render INTO
//! the `error` field of `SessionEvent::ToolError` so the LLM gets an
//! explicit routing signal alongside the original message. It mirrors
//! hermes-agent's `FailoverReason` taxonomy but stays harness-neutral —
//! the harness never *acts* on the kind itself (R7 / R9 keep tool
//! selection in the LLM's hands), it only labels the failure.

use super::service::ToolError;

/// Coarse, stable classification of a tool-call failure.
///
/// Layered on top of `ToolError` — the variant gives us most of the
/// answer, and for `Execution` / `Other` we scan the cause string for
/// HTTP status codes and well-known phrases. Order matches priority
/// when multiple signals overlap (e.g. a 401 inside a "timeout"
/// message is still classified as Unauthorized).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolErrorKind {
    /// HTTP 401 / 403 / explicit auth-rejected message.
    Unauthorized,
    /// HTTP 429 / "rate limit" / "quota exceeded" phrases.
    RateLimited,
    /// HTTP 404 / "not found" outside of `ToolError::NotFound`.
    UpstreamNotFound,
    /// HTTP 5xx / "server error" / "bad gateway".
    UpstreamServerError,
    /// CAPTCHA / robots.txt / paywall / WAF / cloudflare challenge.
    BlockedByPolicy,
    /// 200 response with empty body, zero results, or zero matches.
    EmptyResult,
    /// Aleph-internal: tool wall-clock budget exceeded.
    Timeout,
    /// Aleph-internal: transport / IO failure (network, IPC).
    Transport,
    /// Aleph-internal: caller-supplied input violated the tool's schema.
    Validation,
    /// Aleph-internal: permission gate rejected the call.
    Permission,
    /// Aleph-internal: the named tool is not registered.
    ToolNotFound,
    /// Aleph-internal: duplicate registration (developer error).
    Duplicate,
    /// Fallback when no other classifier matched. Treat as opaque —
    /// the LLM should switch methods rather than blindly retry.
    Execution,
}

impl ToolErrorKind {
    /// Short stable label for prompt rendering. Stable: this string is
    /// part of the LLM-facing wire surface, do not reword without
    /// updating prompt-side tests.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unauthorized => "unauthorized",
            Self::RateLimited => "rate_limited",
            Self::UpstreamNotFound => "upstream_not_found",
            Self::UpstreamServerError => "upstream_server_error",
            Self::BlockedByPolicy => "blocked_by_policy",
            Self::EmptyResult => "empty_result",
            Self::Timeout => "timeout",
            Self::Transport => "transport",
            Self::Validation => "validation",
            Self::Permission => "permission",
            Self::ToolNotFound => "tool_not_found",
            Self::Duplicate => "duplicate",
            Self::Execution => "execution",
        }
    }

    /// Whether re-running the SAME call with SAME arguments is likely
    /// to succeed. `false` means "switch tool / source / args before
    /// trying again".
    ///
    /// **Not** the retry gate. `retry::execute_with_one_shot_backoff` asks
    /// `ToolError::is_retryable`, a narrow match on the structural variants
    /// (`Timeout` / `Transport` / `ApprovalExpired`); this predicate is the
    /// wider, message-pattern-derived classification that also covers
    /// `RateLimited` and `UpstreamServerError`. Deliberately keeping the gate
    /// structural: a retry decision driven by substring matching is how a
    /// healthy provider gets locked out because a token count contained
    /// `"401"`.
    ///
    /// What it is for is the relationship between the two, which
    /// `ToolError::kind`'s doc asserts and
    /// `every_retryable_variant_is_also_transient` pins: the kind taxonomy must
    /// stay a superset of the retry gate, so the LLM-facing routing hint never
    /// tells the model to switch approach for an error the tool layer just
    /// silently retried.
    #[must_use]
    pub const fn is_transient(self) -> bool {
        matches!(
            self,
            Self::Timeout | Self::Transport | Self::RateLimited | Self::UpstreamServerError
        )
    }
}

/// Classify a rendered tool-error string. Used when only the
/// post-`to_string()` form is available (e.g. when reading
/// `SessionEvent::ToolError.error` back from the session log).
///
/// The classifier is deliberately pattern-based and conservative: when
/// in doubt it returns `Execution` and the caller decides what to do.
/// All matching is case-insensitive.
#[must_use]
pub fn classify_error_str(s: &str) -> ToolErrorKind {
    let lower = s.to_ascii_lowercase();

    // The Aleph-internal variants render with a recognisable prefix —
    // catch them first so we don't mis-flag a `Timeout` containing the
    // word "server" as `UpstreamServerError`.
    if lower.contains("timed out after") || lower.contains("execution timeout") {
        return ToolErrorKind::Timeout;
    }
    if lower.starts_with("tool not found") {
        return ToolErrorKind::ToolNotFound;
    }
    if lower.contains("permission denied") {
        return ToolErrorKind::Permission;
    }
    if lower.contains("invalid input") {
        return ToolErrorKind::Validation;
    }
    if lower.contains("duplicate tool name") {
        return ToolErrorKind::Duplicate;
    }
    if lower.contains("transport error") {
        return ToolErrorKind::Transport;
    }

    // HTTP status codes — most informative when present.
    if has_status(&lower, 401) || has_status(&lower, 403) || lower.contains("unauthorized") {
        return ToolErrorKind::Unauthorized;
    }
    if has_status(&lower, 429)
        || lower.contains("rate limit")
        || lower.contains("rate-limit")
        || lower.contains("quota exceeded")
        || lower.contains("too many requests")
    {
        return ToolErrorKind::RateLimited;
    }
    if has_status(&lower, 404) || lower.contains("not found") {
        return ToolErrorKind::UpstreamNotFound;
    }
    if has_status(&lower, 500)
        || has_status(&lower, 502)
        || has_status(&lower, 503)
        || has_status(&lower, 504)
        || lower.contains("bad gateway")
        || lower.contains("service unavailable")
        || lower.contains("gateway timeout")
    {
        return ToolErrorKind::UpstreamServerError;
    }

    // Policy / anti-bot signals.
    if lower.contains("captcha")
        || lower.contains("robots.txt")
        || lower.contains("cloudflare")
        || lower.contains("blocked by")
        || lower.contains("access denied")
        || lower.contains("paywall")
    {
        return ToolErrorKind::BlockedByPolicy;
    }

    // Empty-result heuristics — only fire when the message LOOKS like a
    // success-but-empty (e.g. "0 results", "no matches", "empty body").
    if lower.contains("no results")
        || lower.contains("0 results")
        || lower.contains("no matches")
        || lower.contains("empty body")
        || lower.contains("no content")
    {
        return ToolErrorKind::EmptyResult;
    }

    ToolErrorKind::Execution
}

/// True iff the lowercase string mentions the given HTTP status code as
/// a standalone number (not as part of a larger digit run).
fn has_status(lower: &str, code: u16) -> bool {
    let needle = code.to_string();
    let bytes = lower.as_bytes();
    let mut i = 0;
    while let Some(found) = lower[i..].find(&needle) {
        let start = i + found;
        let end = start + needle.len();
        let before_ok = start == 0 || !bytes[start - 1].is_ascii_digit();
        let after_ok = end == bytes.len() || !bytes[end].is_ascii_digit();
        if before_ok && after_ok {
            return true;
        }
        i = end;
    }
    false
}

/// Classify a `ToolError` directly — preferred over `classify_error_str`
/// because the variant gives us the answer without parsing.
#[must_use]
pub fn classify_tool_error(err: &ToolError) -> ToolErrorKind {
    match err {
        // The human timed out, not the tool — but for the model's purposes the
        // shape is the same: transient, not a verdict, may resolve on its own.
        // It must stay transient to keep `kind()` a superset of `is_retryable`.
        ToolError::Timeout { .. } | ToolError::ApprovalExpired { .. } => ToolErrorKind::Timeout,
        ToolError::Transport { .. } => ToolErrorKind::Transport,
        ToolError::ValidationFailed { .. } => ToolErrorKind::Validation,
        ToolError::PermissionDenied { .. } => ToolErrorKind::Permission,
        ToolError::NotFound { .. } => ToolErrorKind::ToolNotFound,
        ToolError::Duplicate { .. } => ToolErrorKind::Duplicate,
        // For Execution / Other, fall through to string-based scan: the
        // upstream HTTP status or "rate limit" / "captcha" / empty-result
        // phrasing lives in the cause string we built when wrapping the
        // failure.
        ToolError::Execution { cause, .. } | ToolError::Other(cause) => classify_error_str(cause),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_is_stable_lowercase_snakecase() {
        // Every variant must produce a non-empty, non-uppercase label.
        for k in [
            ToolErrorKind::Unauthorized,
            ToolErrorKind::RateLimited,
            ToolErrorKind::UpstreamNotFound,
            ToolErrorKind::UpstreamServerError,
            ToolErrorKind::BlockedByPolicy,
            ToolErrorKind::EmptyResult,
            ToolErrorKind::Timeout,
            ToolErrorKind::Transport,
            ToolErrorKind::Validation,
            ToolErrorKind::Permission,
            ToolErrorKind::ToolNotFound,
            ToolErrorKind::Duplicate,
            ToolErrorKind::Execution,
        ] {
            let s = k.label();
            assert!(!s.is_empty());
            assert!(s.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
        }
    }

    #[test]
    fn is_transient_matches_known_set() {
        assert!(ToolErrorKind::Timeout.is_transient());
        assert!(ToolErrorKind::Transport.is_transient());
        assert!(ToolErrorKind::RateLimited.is_transient());
        assert!(ToolErrorKind::UpstreamServerError.is_transient());
        // Switching, not retrying, is the right move for these:
        assert!(!ToolErrorKind::Unauthorized.is_transient());
        assert!(!ToolErrorKind::BlockedByPolicy.is_transient());
        assert!(!ToolErrorKind::EmptyResult.is_transient());
        assert!(!ToolErrorKind::Validation.is_transient());
        assert!(!ToolErrorKind::Permission.is_transient());
        assert!(!ToolErrorKind::ToolNotFound.is_transient());
        assert!(!ToolErrorKind::UpstreamNotFound.is_transient());
        assert!(!ToolErrorKind::Duplicate.is_transient());
        assert!(!ToolErrorKind::Execution.is_transient());
    }

    /// The superset relationship `ToolError::kind`'s doc asserts, made
    /// checkable.
    ///
    /// It matters because the two classifications feed opposite advice at the
    /// same moment: the tool layer silently respins anything `is_retryable`,
    /// while the routing hint built from `kind()` tells the model whether to
    /// try again or switch approach. If a variant were retryable but not
    /// transient, the model would be told "switch tool / source / args" about a
    /// call the layer beneath it had just retried on its behalf.
    #[test]
    fn every_retryable_variant_is_also_transient() {
        use crate::tools::service::ToolError;

        let variants = [
            ToolError::NotFound { name: "t".into() },
            ToolError::PermissionDenied {
                name: "t".into(),
                reason: "no".into(),
            },
            ToolError::ValidationFailed {
                name: "t".into(),
                cause: "bad".into(),
            },
            ToolError::Execution {
                name: "t".into(),
                cause: "boom".into(),
            },
            ToolError::Timeout {
                name: "t".into(),
                elapsed_ms: 1,
            },
            ToolError::ApprovalExpired {
                name: "t".into(),
                waited_ms: 1,
            },
            ToolError::Transport {
                name: "t".into(),
                cause: "reset".into(),
            },
            ToolError::Duplicate { name: "t".into() },
            ToolError::Other("opaque".into()),
        ];

        for e in &variants {
            if e.is_retryable() {
                assert!(
                    e.kind().is_transient(),
                    "{e} is retried by the tool layer, so its kind must not tell \
                     the model to switch approach (kind = {:?})",
                    e.kind()
                );
            }
        }
    }

    #[test]
    fn classify_str_recognises_http_status_codes() {
        assert_eq!(
            classify_error_str("HTTP 401 from reuters.com"),
            ToolErrorKind::Unauthorized
        );
        assert_eq!(
            classify_error_str("HTTP 403 Forbidden"),
            ToolErrorKind::Unauthorized
        );
        assert_eq!(
            classify_error_str("got 429 from search api"),
            ToolErrorKind::RateLimited
        );
        assert_eq!(
            classify_error_str("HTTP 404 page not found"),
            ToolErrorKind::UpstreamNotFound
        );
        assert_eq!(
            classify_error_str("upstream returned 503 Service Unavailable"),
            ToolErrorKind::UpstreamServerError
        );
    }

    #[test]
    fn classify_str_recognises_phrases_without_codes() {
        assert_eq!(
            classify_error_str("Cloudflare challenge detected"),
            ToolErrorKind::BlockedByPolicy
        );
        assert_eq!(
            classify_error_str("Rate limit exceeded, please retry"),
            ToolErrorKind::RateLimited
        );
        assert_eq!(
            classify_error_str("Too Many Requests"),
            ToolErrorKind::RateLimited
        );
        assert_eq!(
            classify_error_str("returned 0 results"),
            ToolErrorKind::EmptyResult
        );
        assert_eq!(
            classify_error_str("paywall blocking access"),
            ToolErrorKind::BlockedByPolicy
        );
    }

    #[test]
    fn classify_str_prefers_aleph_internal_prefixes() {
        // "tool foo timed out after 5000ms" must beat any HTTP heuristic.
        assert_eq!(
            classify_error_str("tool foo timed out after 5000ms"),
            ToolErrorKind::Timeout
        );
        assert_eq!(
            classify_error_str("permission denied for tool exec_code: workspace gate"),
            ToolErrorKind::Permission
        );
        assert_eq!(
            classify_error_str("invalid input for tool web_fetch: url required"),
            ToolErrorKind::Validation
        );
        assert_eq!(
            classify_error_str("tool not found: nonsense"),
            ToolErrorKind::ToolNotFound
        );
    }

    #[test]
    fn classify_str_falls_through_to_execution() {
        // Bare cause with no signal we recognise — must NOT be confused
        // with a known kind. Better to say "execution" than mis-flag.
        assert_eq!(
            classify_error_str("subprocess exited with code 7"),
            ToolErrorKind::Execution
        );
        assert_eq!(
            classify_error_str("unexpected JSON shape"),
            ToolErrorKind::Execution
        );
    }

    #[test]
    fn has_status_isolates_codes_from_digit_runs() {
        // "404" inside "1404" must not match — `has_status` guards
        // against substring collisions inside larger numeric tokens.
        assert!(!super::has_status("error 1404 occurred", 404));
        assert!(super::has_status("error 404 occurred", 404));
        assert!(super::has_status("error: 404", 404));
        assert!(super::has_status("404", 404));
    }

    #[test]
    fn classify_tool_error_uses_variant_first() {
        let err = ToolError::Timeout {
            name: "search".into(),
            elapsed_ms: 5000,
        };
        assert_eq!(classify_tool_error(&err), ToolErrorKind::Timeout);

        let err = ToolError::Transport {
            name: "web_fetch".into(),
            cause: "connection reset".into(),
        };
        assert_eq!(classify_tool_error(&err), ToolErrorKind::Transport);
    }

    #[test]
    fn classify_tool_error_falls_through_to_str_for_execution() {
        let err = ToolError::Execution {
            name: "web_fetch".into(),
            cause: "HTTP 401 unauthorized".into(),
        };
        assert_eq!(classify_tool_error(&err), ToolErrorKind::Unauthorized);

        let err = ToolError::Other("got 429 too many requests".into());
        assert_eq!(classify_tool_error(&err), ToolErrorKind::RateLimited);
    }
}
