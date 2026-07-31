//! Secret redaction for diagnostic text.
//!
//! Probe and repair errors can embed credentials (a provider echoing its
//! `Authorization` header in an error body, a config snippet in an io
//! error). Findings are rendered to the CLI, serialized to `--json`, and
//! shipped to the LLM as tool output — so any error/detail text flowing
//! into a [`Finding`](super::Finding) passes through [`redact_secrets`]
//! first. Best-effort pattern matching, not a guarantee: known credential
//! *shapes* are masked, unknown ones are not.

use std::sync::OnceLock;

use regex::Regex;

/// Credential shapes to mask. Order matters inside the alternation:
/// `apikey=` must precede `key=` so `apikey=abc` is masked whole instead of
/// leaving an `api` stump behind.
fn patterns() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)(sk-[A-Za-z0-9_-]{8,}|bearer\s+\S+|basic\s+\S+|eyJ[A-Za-z0-9_-]*(?:\.[A-Za-z0-9_-]+){2,4}|apikey=\S+|token=\S+|key=\S+)",
        )
        .expect("redaction regex is a compile-time constant")
    })
}

/// Replace common credential shapes in `input` with `***`.
#[must_use]
pub fn redact_secrets(input: &str) -> String {
    patterns().replace_all(input, "***").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_openai_style_keys() {
        let out = redact_secrets("auth failed for sk-abcdefgh12345678");
        assert_eq!(out, "auth failed for ***");
    }

    #[test]
    fn redacts_bearer_headers() {
        let out = redact_secrets("header Authorization: Bearer eyJhbGciOi was rejected");
        assert_eq!(out, "header Authorization: *** was rejected");
    }

    #[test]
    fn redacts_basic_auth_headers() {
        let out = redact_secrets("header Authorization: Basic dXNlcjpwYXNzd29yZA== was rejected");
        assert_eq!(out, "header Authorization: *** was rejected");
    }

    #[test]
    fn redacts_standalone_jwts() {
        let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        let out = redact_secrets(&format!("token {jwt} was rejected"));
        assert_eq!(out, "token *** was rejected");
    }

    #[test]
    fn redacts_query_param_shapes_case_insensitively() {
        assert_eq!(redact_secrets("?key=abc123"), "?***");
        assert_eq!(redact_secrets("?TOKEN=abc123"), "?***");
        assert_eq!(redact_secrets("?ApiKey=abc123"), "?***");
        // `apikey=` is masked whole, not reduced to an `api` stump.
        assert_eq!(redact_secrets("?apikey=abc123"), "?***");
    }

    #[test]
    fn leaves_ordinary_text_untouched() {
        let plain = "connection refused (os error 111)";
        assert_eq!(redact_secrets(plain), plain);
        // A bare `key` without `=` is not a credential shape.
        assert_eq!(redact_secrets("the key insight"), "the key insight");
        // Short sk- fragments are not key-shaped.
        assert_eq!(redact_secrets("sk-short"), "sk-short");
    }

    #[test]
    fn redacts_multiple_secrets_in_one_string() {
        let out = redact_secrets("sk-abcdefgh1234 and Bearer xyz both leaked");
        assert_eq!(out, "*** and *** both leaked");
    }
}
