use crate::cron::config::ErrorReason;

/// Classify an error as transient or permanent for retry decisions.
pub fn classify_error(error: &str) -> ErrorReason {
    let lower = error.to_lowercase();
    let transient_patterns = [
        "rate_limit",
        "rate limit",
        "429",
        "timeout",
        "timed out",
        "overloaded",
        "503",
        "502",
        "500",
        "network",
        "connection",
        "dns",
        "temporarily",
        "retry",
    ];
    if transient_patterns.iter().any(|p| lower.contains(p)) {
        ErrorReason::Transient(error.to_string())
    } else {
        ErrorReason::Permanent(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_transient_errors() {
        let cases = [
            "rate_limit exceeded",
            "rate limit exceeded",
            "HTTP 429 Too Many Requests",
            "request timeout after 30s",
            "connection timed out",
            "server overloaded",
            "HTTP 503 Service Unavailable",
            "HTTP 502 Bad Gateway",
            "HTTP 500 Internal Server Error",
            "network error: could not resolve host",
            "connection refused",
            "DNS resolution failed",
            "temporarily unavailable",
            "please retry later",
        ];
        for case in cases {
            match classify_error(case) {
                ErrorReason::Transient(_) => {}
                ErrorReason::Permanent(_) => {
                    panic!("Expected Transient for: {}", case);
                }
            }
        }
    }

    #[test]
    fn classify_permanent_errors() {
        let cases = [
            "invalid API key",
            "model not found: gpt-5",
            "permission denied",
            "authentication failed",
            "invalid request body",
        ];
        for case in cases {
            match classify_error(case) {
                ErrorReason::Permanent(_) => {}
                ErrorReason::Transient(_) => {
                    panic!("Expected Permanent for: {}", case);
                }
            }
        }
    }
}
