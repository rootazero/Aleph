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

use super::service::{RefusedBy, ToolError};
use crate::thinker::nudges::CROSS_BATCH_REFUSED_CAUSE;

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
    /// Aleph-internal: the call was refused — a permission gate
    /// (`ToolError::PermissionDenied`), or a hook, a person, or nobody being
    /// there to ask (`ToolError::Refused`). A verdict, not a failure: no ladder.
    Permission,
    /// Aleph-internal: the named tool is not registered.
    ToolNotFound,
    /// Aleph-internal: duplicate registration (developer error).
    Duplicate,
    /// Aleph-internal: the run was stopped while the call was in flight.
    /// Says nothing about the call — the user did not judge it.
    Cancelled,
    /// The harness refused an identical repeat of a call that already failed
    /// with a non-retryable error this run (`CROSS_BATCH_REFUSED_CAUSE`); the
    /// call did not run. The earlier failure's kind is the one that means
    /// something, and this error does not carry it — the per-call hint cannot
    /// see it (no ladder here: the earlier error already had whatever hint its
    /// own kind admitted), the run summary looks it up in the event log.
    Repeated,
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
            Self::Cancelled => "cancelled",
            Self::Repeated => "repeated",
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
            Self::Timeout
                | Self::Transport
                | Self::RateLimited
                | Self::UpstreamServerError
                | Self::Cancelled
        )
    }

    /// Whether the model may be pointed at another route after a failure of
    /// this kind — the one question both ladder surfaces ask:
    /// `fallback_registry::render_persistence_hint` (per call) and
    /// `attempt_summary::aggregate_failures` (per run).
    ///
    /// `false` for a call nobody judged (`Cancelled`), for a call that was
    /// refused (`Permission`) and for a refused repeat whose own kind is
    /// unknown here (`Repeated`): suggesting a way around a refusal is steering
    /// the model around the guard (a path-scoped `Read` hook plus "`file_ops
    /// copy`, then read the copy" is the same read).
    #[must_use]
    pub const fn admits_ladder(self) -> bool {
        !matches!(self, Self::Cancelled | Self::Permission | Self::Repeated)
    }
}

/// The kind a rendering states by its fixed head — the part of `Display` the
/// variant writes, before any content (a hook's prose, a user's words, a
/// cause) it carries. `None` when the content is all there is to go on
/// (`Execution`, `Other`). Checked before any content scan, so the read-back
/// face agrees with `classify_tool_error` (判据 §12).
fn kind_from_head(lower: &str) -> Option<ToolErrorKind> {
    const HEADS: [(&str, ToolErrorKind); 9] = [
        ("permission denied for tool ", ToolErrorKind::Permission),
        ("invalid input for tool ", ToolErrorKind::Validation),
        ("tool not found: ", ToolErrorKind::ToolNotFound),
        ("duplicate tool name: ", ToolErrorKind::Duplicate),
        ("approval for tool ", ToolErrorKind::Timeout),
        ("invalid tool descriptor for ", ToolErrorKind::Validation),
        ("descriptor mismatch for tool ", ToolErrorKind::Validation),
        ("registry closed: ", ToolErrorKind::Execution),
        ("unknown registration revision ", ToolErrorKind::Execution),
    ];
    if let Some((_, kind)) = HEADS.iter().find(|(head, _)| lower.starts_with(head)) {
        return Some(*kind);
    }
    // The rest render as `tool <name> <tail>`.
    let (_, tail) = lower.strip_prefix("tool ")?.split_once(' ')?;
    if let Some(cause) = tail.strip_prefix("execution failed: ") {
        return cause
            .starts_with(CROSS_BATCH_REFUSED_CAUSE)
            .then_some(ToolErrorKind::Repeated);
    }
    if RefusedBy::ALL.iter().any(|by| {
        tail.strip_prefix(by.head())
            .is_some_and(|r| r.starts_with(':'))
    }) {
        return Some(ToolErrorKind::Permission);
    }
    [
        ("timed out after ", ToolErrorKind::Timeout),
        ("transport error: ", ToolErrorKind::Transport),
        ("was cancelled — ", ToolErrorKind::Cancelled),
    ]
    .into_iter()
    .find_map(|(head, kind)| tail.starts_with(head).then_some(kind))
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

    // First, the variant's own head: what follows it is a hook's prose, a
    // user's words or a tool's cause, and whatever that says ("not found",
    // "timed out after", "429") is not the kind of a refusal.
    if let Some(kind) = kind_from_head(&lower) {
        return kind;
    }

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
    // Word-boundary anchored matches for rate-limit phrases. A bare
    // `lower.contains("rate limit")` would false-positive on prose like
    // "the response includes the new rate limit header but the call
    // succeeded" and reclassify a successful run as transient — the
    // model-facing routing hint (switch tool / source / args) then
    // pushes the model off a working path. Word boundaries keep the
    // match load-bearing on the actual signal.
    if has_status(&lower, 429)
        || contains_word(&lower, "rate limit")
        || contains_word(&lower, "rate-limit")
        || contains_word(&lower, "quota exceeded")
        || contains_word(&lower, "too many requests")
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

/// True iff `haystack` contains `needle` with non-alphanumeric ASCII on
/// each side. Reduces substring-match false positives like
/// "rate limited by design" firing `RateLimited` when the prose was
/// describing a configuration choice, not an upstream rejection.
///
/// Hyphens, slashes, dots, and underscores are accepted on either side
/// (so "rate-limit" and "rate_limit" still match "rate limit") but a
/// letter/digit adjacent to the needle is not — "rating-limit" will not
/// match "rate limit".
fn contains_word(haystack: &str, needle: &str) -> bool {
    let bytes = haystack.as_bytes();
    let needle_bytes = needle.as_bytes();
    let mut i = 0;
    while let Some(found) = haystack[i..].find(needle) {
        let start = i + found;
        let end = start + needle_bytes.len();
        let before_ok = start == 0 || !is_word_char(bytes[start - 1]);
        let after_ok = end == bytes.len() || !is_word_char(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        i = end;
    }
    false
}

fn is_word_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
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
        // A refusal is a verdict. Never the string scan: its reason is a hook
        // author's prose or a user's own words, and the same refusal would
        // otherwise be labelled `timeout` or `upstream_not_found` by how it
        // was worded.
        ToolError::PermissionDenied { .. } | ToolError::Refused { .. } => ToolErrorKind::Permission,
        // The harness's refusal of an identical repeat, recognised by
        // equality with the constant it is built from — not by a substring.
        ToolError::Execution { cause, .. } if cause == CROSS_BATCH_REFUSED_CAUSE => {
            ToolErrorKind::Repeated
        }
        ToolError::NotFound { .. } => ToolErrorKind::ToolNotFound,
        ToolError::Duplicate { .. } => ToolErrorKind::Duplicate,
        // Registry admission failures are developer/contract errors, not a
        // verdict on a tool that ran. `Validation` marks the contract refusal;
        // a closed registry or a stale revision is an Execution-layer refusal.
        ToolError::InvalidDescriptor { .. } | ToolError::DescriptorMismatch { .. } => {
            ToolErrorKind::Validation
        }
        ToolError::RegistryClosed { .. } | ToolError::UnknownRevision { .. } => {
            ToolErrorKind::Execution
        }
        // Never falls through to the string scan: the cause of a cancellation
        // is whatever the tool happened to be saying when the run stopped, and
        // classifying *that* would hand the model a routing hint about a
        // failure that never happened.
        ToolError::Cancelled { .. } => ToolErrorKind::Cancelled,
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
            ToolErrorKind::Cancelled,
            ToolErrorKind::Repeated,
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

    /// Pressing stop must not ban the call that was stopped.
    ///
    /// The harness's cross-batch failure memo enters a call whose error is not
    /// retryable (`agent/act.rs`: "Only a NON-retryable failure enters the
    /// cross-batch memo"), and an identical re-issue is then refused for the
    /// rest of the run. A cancellation used to arrive as
    /// `ToolError::Execution { cause: "tool X cancelled" }`, so one `/stop`
    /// permanently banned that exact call — and the model was handed a
    /// persistence hint telling it to climb the tool ladder, about a failure
    /// that never happened. Aleph already made this argument for
    /// `ApprovalExpired` ("nobody said no — nobody said anything"); a
    /// cancellation says even less about the call.
    #[test]
    fn a_cancelled_call_is_not_a_verdict_on_the_call() {
        use crate::tools::service::ToolError;

        let cancelled = ToolError::Cancelled {
            name: "bash".into(),
        };
        assert!(
            cancelled.is_retryable(),
            "the cross-batch memo keys off !is_retryable — a cancelled call must \
             not be banned for the rest of the run"
        );
        assert_eq!(cancelled.kind(), ToolErrorKind::Cancelled);
        assert!(cancelled.kind().is_transient());
        assert_eq!(
            crate::tools::fallback_registry::render_persistence_hint(&cancelled, "bash"),
            "",
            "a stopped call is not a rung of any ladder"
        );
        // …while a genuine execution failure still is.
        let failed = ToolError::Execution {
            name: "bash".into(),
            cause: "exit 1".into(),
        };
        assert!(!failed.is_retryable());
        assert!(
            !crate::tools::fallback_registry::render_persistence_hint(&failed, "bash").is_empty()
        );
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
            ToolError::Refused {
                name: "t".into(),
                by: crate::tools::service::RefusedBy::Hook,
                reason: "timed out after 5ms".into(),
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
            ToolError::Cancelled { name: "t".into() },
            ToolError::Duplicate { name: "t".into() },
            ToolError::InvalidDescriptor {
                name: "t".into(),
                reason: "empty name".into(),
            },
            ToolError::DescriptorMismatch {
                name: "t".into(),
                reason: "source differs".into(),
            },
            ToolError::RegistryClosed { name: "t".into() },
            ToolError::UnknownRevision {
                name: "t".into(),
                revision: 3,
            },
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

    /// M-2: the read-back face (`classify_error_str` over a persisted error)
    /// agrees with the live face (`kind()`) for every rendering, even when the
    /// content inside it — a hook's prose, a user's words — reads like another
    /// kind. A variant with a fixed head is recognised by its head, before any
    /// content is scanned.
    #[test]
    fn every_rendering_reads_back_as_its_live_kind() {
        use crate::tools::service::{RefusedBy, ToolError};

        let prose = "HTTP 404 not found; timed out after 5ms; permission denied; 429";
        let t = || "file_read".to_string();
        let mut errors = vec![
            ToolError::NotFound { name: t() },
            ToolError::PermissionDenied {
                name: t(),
                reason: prose.into(),
            },
            ToolError::ValidationFailed {
                name: t(),
                cause: prose.into(),
            },
            ToolError::Execution {
                name: t(),
                cause: prose.into(),
            },
            // The harness's refusal of an identical repeat.
            ToolError::Execution {
                name: t(),
                cause: CROSS_BATCH_REFUSED_CAUSE.into(),
            },
            ToolError::Timeout {
                name: t(),
                elapsed_ms: 5,
            },
            ToolError::ApprovalExpired {
                name: t(),
                waited_ms: 5,
            },
            ToolError::Transport {
                name: t(),
                cause: prose.into(),
            },
            ToolError::Cancelled { name: t() },
            ToolError::Duplicate { name: t() },
            ToolError::InvalidDescriptor {
                name: t(),
                reason: prose.into(),
            },
            ToolError::DescriptorMismatch {
                name: t(),
                reason: prose.into(),
            },
            ToolError::RegistryClosed { name: t() },
            ToolError::UnknownRevision {
                name: t(),
                revision: 7,
            },
            ToolError::Other(prose.into()),
        ];
        errors.extend(RefusedBy::ALL.map(|by| ToolError::Refused {
            name: t(),
            by,
            reason: prose.into(),
        }));
        for e in &errors {
            // A new variant fails to compile here until it is listed above.
            match e {
                ToolError::NotFound { .. }
                | ToolError::PermissionDenied { .. }
                | ToolError::ValidationFailed { .. }
                | ToolError::Execution { .. }
                | ToolError::Refused { .. }
                | ToolError::Timeout { .. }
                | ToolError::ApprovalExpired { .. }
                | ToolError::Transport { .. }
                | ToolError::Cancelled { .. }
                | ToolError::Duplicate { .. }
                | ToolError::InvalidDescriptor { .. }
                | ToolError::DescriptorMismatch { .. }
                | ToolError::RegistryClosed { .. }
                | ToolError::UnknownRevision { .. }
                | ToolError::Other(_) => {}
            }
            assert_eq!(classify_error_str(&e.to_string()), e.kind(), "{e}");
        }
        // Every refusal and the repeat are `Permission` / `Repeated` on both
        // faces, whatever their content says.
        assert!(errors
            .iter()
            .filter(|e| matches!(e, ToolError::Refused { .. }))
            .all(|e| e.kind() == ToolErrorKind::Permission));
        assert_eq!(errors[4].kind(), ToolErrorKind::Repeated);
        // Only the head counts: a refusal's head quoted inside another error's
        // cause is that cause's prose.
        let quoted = ToolError::Execution {
            name: "web_fetch".into(),
            cause: "page said: tool x was refused by a policy hook: HTTP 404".into(),
        };
        assert_eq!(
            classify_error_str(&quoted.to_string()),
            ToolErrorKind::UpstreamNotFound
        );
        // …and the repeat is recognised by the whole constant, not a fragment.
        let fragment = ToolError::Execution {
            name: t(),
            cause: "this exact call already failed earlier in the run".into(),
        };
        assert_eq!(fragment.kind(), ToolErrorKind::Execution);
        assert_eq!(
            classify_error_str(&fragment.to_string()),
            ToolErrorKind::Execution
        );
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
