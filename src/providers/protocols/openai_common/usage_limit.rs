//! Quota / spending / usage-limit detection for OpenAI-compatible HTTP error bodies.
//!
//! Shared by the Chat Completions and Responses adapters so both non-2xx
//! paths surface the same typed, non-retryable usage-limit error.

/// Detect a quota / spending / usage-limit error from an OpenAI-compatible
/// provider's (non-429) HTTP error body.
///
/// Several providers report exhausted credits with a non-429 status and a
/// descriptive body instead of a clean 429 — e.g. xAI (Grok) returns HTTP 403
/// "used all available credits or reached its monthly spending limit", and
/// OpenAI uses `insufficient_quota`. Matching these lets the caller surface a
/// typed, non-retryable usage-limit error rather than a generic provider fault.
pub(crate) fn is_usage_limit_body(body: &str) -> bool {
    let b = body.to_ascii_lowercase();
    const MARKERS: &[&str] = &[
        "spending limit",
        "available credits",
        "insufficient_quota",
        "insufficient quota",
        "quota exceeded",
        "exceeded your current quota",
        "usage limit",
        "out of credits",
    ];
    MARKERS.iter().any(|m| b.contains(m))
}

#[cfg(test)]
mod tests {
    use super::is_usage_limit_body;

    #[test]
    fn detects_xai_spending_limit_403_body() {
        let body = r#"{"code":"...","error":"Your team ... has either used all available credits or reached its monthly spending limit ..."}"#;
        assert!(is_usage_limit_body(body));
    }

    #[test]
    fn detects_openai_insufficient_quota() {
        let body = r#"{"error":{"message":"You exceeded your current quota","type":"insufficient_quota"}}"#;
        assert!(is_usage_limit_body(body));
    }

    #[test]
    fn case_insensitive() {
        assert!(is_usage_limit_body("Monthly SPENDING LIMIT reached"));
    }

    #[test]
    fn ignores_unrelated_errors() {
        assert!(!is_usage_limit_body(
            r#"{"error":{"message":"invalid model: gpt-9","type":"invalid_request_error"}}"#
        ));
        assert!(!is_usage_limit_body(
            "400 Bad Request: missing field 'messages'"
        ));
        assert!(!is_usage_limit_body(""));
    }
}
