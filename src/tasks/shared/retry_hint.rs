//! Error classification for task retry decisions.
//!
//! Ported from openclaw's `src/cron/retry-hint.ts`. Classifies an error
//! message string into one of a small set of transient categories. The
//! caller decides what to do with the classification — typically:
//!
//! - **Transient (any category)**: keep retrying with the existing
//!   [`compute_backoff_ms`] escalation ladder
//!   (`30s → 60s → 5min → 15min → 60min`).
//! - **Non-transient (no match)**: stop scheduling further retries
//!   (clear `next_run_at_ms`) and surface the failure to the user via
//!   the failure-alert path, bypassing the alert cooldown.
//!
//! Classification is purely pattern-based on the error string; it does
//! not interpret status codes or exception types. The patterns are
//! compiled once at first use via [`std::sync::OnceLock`].
//!
//! [`compute_backoff_ms`]: super::schedule::compute_backoff_ms

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// A coarse error category used to drive retry decisions.
///
/// Order matches openclaw's `retry-hint.ts` precedence — first pattern
/// to match wins, so more specific categories are listed earlier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryCategory {
    /// HTTP 429, "rate limit", token-per-day exhaustion, Cloudflare.
    RateLimit,
    /// HTTP 529, "overloaded", high-demand / capacity-exceeded responses.
    Overloaded,
    /// `ECONNRESET`, `ECONNREFUSED`, fetch failures, socket errors.
    Network,
    /// "timeout" / `ETIMEDOUT` text matches.
    Timeout,
    /// Any HTTP 5xx response.
    ServerError,
}

impl RetryCategory {
    /// Stable lowercase wire token used in SQLite/JSON.
    pub fn as_str(self) -> &'static str {
        match self {
            RetryCategory::RateLimit => "rate_limit",
            RetryCategory::Overloaded => "overloaded",
            RetryCategory::Network => "network",
            RetryCategory::Timeout => "timeout",
            RetryCategory::ServerError => "server_error",
        }
    }

    /// Parse a stable wire token back into a `RetryCategory`.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "rate_limit" => Some(RetryCategory::RateLimit),
            "overloaded" => Some(RetryCategory::Overloaded),
            "network" => Some(RetryCategory::Network),
            "timeout" => Some(RetryCategory::Timeout),
            "server_error" => Some(RetryCategory::ServerError),
            _ => None,
        }
    }
}

/// The result of [`classify`].
///
/// `retryable=false` means no transient pattern matched — the failure is
/// likely permanent (auth, validation, programmer error). The caller
/// should stop scheduling retries and notify the user. `retryable=true`
/// means the caller should keep applying its normal backoff schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryHint {
    pub retryable: bool,
    pub category: Option<RetryCategory>,
}

impl RetryHint {
    pub const fn permanent() -> Self {
        RetryHint {
            retryable: false,
            category: None,
        }
    }

    pub const fn transient(category: RetryCategory) -> Self {
        RetryHint {
            retryable: true,
            category: Some(category),
        }
    }
}

impl Default for RetryHint {
    fn default() -> Self {
        Self::permanent()
    }
}

/// Compile patterns once. Order matches openclaw's precedence.
fn patterns() -> &'static [(RetryCategory, Regex)] {
    static CELL: OnceLock<Vec<(RetryCategory, Regex)>> = OnceLock::new();
    CELL.get_or_init(|| {
        vec![
            (
                RetryCategory::RateLimit,
                Regex::new(
                    r"(?i)(rate[_ ]limit|too many requests|429|resource has been exhausted|cloudflare|tokens per day)",
                )
                .expect("rate_limit pattern compiles"),
            ),
            (
                RetryCategory::Overloaded,
                Regex::new(
                    r"(?i)\b529\b|\boverloaded(?:_error)?\b|high demand|temporar(?:ily|y) overloaded|capacity exceeded",
                )
                .expect("overloaded pattern compiles"),
            ),
            (
                RetryCategory::Network,
                Regex::new(
                    r"(?i)(network|fetch failed|socket|econnreset|econnrefused|eai_again|ehostunreach|ehostdown|enetreset|enetunreach|epipe)",
                )
                .expect("network pattern compiles"),
            ),
            (
                RetryCategory::Timeout,
                // Matches "timeout", "timed out", "timing out", and the
                // libc errno `ETIMEDOUT`. The space variant is critical
                // because `ExecutionError::Timeout`'s Display is
                // `"Run timed out"` — without it, every cron timeout
                // would fall through to permanent.
                Regex::new(r"(?i)(timeout|tim(?:ed|ing) out|etimedout)")
                    .expect("timeout pattern compiles"),
            ),
            (
                RetryCategory::ServerError,
                Regex::new(r"\b5\d{2}\b").expect("server_error pattern compiles"),
            ),
        ]
    })
}

/// Classify an error string into a retry hint.
///
/// Empty / blank strings classify as permanent (`retryable=false`).
pub fn classify(error: &str) -> RetryHint {
    let trimmed = error.trim();
    if trimmed.is_empty() {
        return RetryHint::permanent();
    }
    for (cat, re) in patterns() {
        if re.is_match(trimmed) {
            return RetryHint::transient(*cat);
        }
    }
    RetryHint::permanent()
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_rate_limit() {
        let cases = [
            "HTTP 429 Too Many Requests",
            "rate limit exceeded",
            "tokens per day quota reached",
            "Cloudflare blocked the request",
            "RESOURCE HAS BEEN EXHAUSTED",
        ];
        for err in cases {
            let hint = classify(err);
            assert!(hint.retryable, "{err} should be retryable");
            assert_eq!(hint.category, Some(RetryCategory::RateLimit), "{err}");
        }
    }

    #[test]
    fn classifies_overloaded() {
        let cases = [
            "HTTP 529 server overloaded",
            "overloaded_error",
            "Service experiencing high demand",
            "temporarily overloaded",
            "capacity exceeded for this model",
        ];
        for err in cases {
            let hint = classify(err);
            assert!(hint.retryable, "{err} should be retryable");
            assert_eq!(hint.category, Some(RetryCategory::Overloaded), "{err}");
        }
    }

    #[test]
    fn classifies_network() {
        let cases = [
            "ECONNRESET on socket read",
            "fetch failed",
            "Network unreachable",
            "ECONNREFUSED 127.0.0.1:8080",
            "EAI_AGAIN dns lookup",
        ];
        for err in cases {
            let hint = classify(err);
            assert!(hint.retryable, "{err}");
            assert_eq!(hint.category, Some(RetryCategory::Network), "{err}");
        }
    }

    #[test]
    fn classifies_timeout() {
        for err in ["request timeout", "ETIMEDOUT", "Connection Timeout"] {
            let hint = classify(err);
            assert!(hint.retryable);
            assert_eq!(hint.category, Some(RetryCategory::Timeout));
        }
    }

    /// Regression: `ExecutionError::Timeout`'s Display is `"Run timed out"`
    /// (with a space). Before this fix the regex was `(timeout|etimedout)`
    /// and missed the space variant, so cron timeouts were silently
    /// classified as permanent and never retried.
    #[test]
    fn classifies_execution_error_timeout_display() {
        for err in [
            "Run timed out",
            "Execution failed: Run timed out",
            "request is timing out",
            "Job timed out after 30s",
        ] {
            let hint = classify(err);
            assert!(hint.retryable, "{err} should be retryable");
            assert_eq!(hint.category, Some(RetryCategory::Timeout), "{err}");
        }
    }

    #[test]
    fn classifies_server_error() {
        for err in ["upstream returned 502", "503 Service Unavailable", "500"] {
            let hint = classify(err);
            assert!(hint.retryable, "{err}");
            assert_eq!(hint.category, Some(RetryCategory::ServerError), "{err}");
        }
    }

    #[test]
    fn unknown_errors_are_permanent() {
        // Auth, validation, programmer errors — should NOT retry.
        for err in [
            "Invalid API key",
            "ValidationError: required field missing",
            "agent not found: weather-bot",
            "Permission denied",
            "JSON parse error at line 3",
        ] {
            let hint = classify(err);
            assert!(!hint.retryable, "{err} must not retry");
            assert_eq!(hint.category, None, "{err}");
        }
    }

    #[test]
    fn empty_and_whitespace_are_permanent() {
        assert_eq!(classify(""), RetryHint::permanent());
        assert_eq!(classify("   "), RetryHint::permanent());
        assert_eq!(classify("\n\t"), RetryHint::permanent());
    }

    #[test]
    fn precedence_rate_limit_over_server_error() {
        // "429" is both a 4xx code AND a server-error-shaped string; rate_limit
        // pattern is listed first, so it wins.
        let hint = classify("HTTP 429 throttled");
        assert_eq!(hint.category, Some(RetryCategory::RateLimit));
    }

    #[test]
    fn wire_token_roundtrip() {
        for cat in [
            RetryCategory::RateLimit,
            RetryCategory::Overloaded,
            RetryCategory::Network,
            RetryCategory::Timeout,
            RetryCategory::ServerError,
        ] {
            let token = cat.as_str();
            assert_eq!(RetryCategory::from_str(token), Some(cat));
        }
        assert_eq!(RetryCategory::from_str("garbage"), None);
    }
}
