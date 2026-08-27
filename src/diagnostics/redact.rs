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
/// longer-prefix vendor shapes (AWS, Anthropic, GitHub, Slack, Stripe,
/// Google) must precede `sk-[A-Za-z0-9_-]{8,}`; `apikey=` and the X-…
/// header family must precede `key=\S+`; and query-param shapes use
/// `[^\s&=#?]+` instead of `\S+` so a URL fragment like
/// `?token=abc&next=https://other/?key=xyz` is masked *to the boundary of
/// the parameter* rather than swallowing the whole redirect tail.
fn patterns() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            concat!(
                r"(?ix)", // case-insensitive + allow comments + whitespace
                r"(?:
                    # Vendor prefixes (longer first, so the alternation never
                    # stops at a shorter prefix that happens to be a substring).
                    sk-ant-[A-Za-z0-9_-]{8,}               |
                    sk_live_[A-Za-z0-9]+                    |
                    AKIA[0-9A-Z]{16}                        |
                    AIza[0-9A-Za-z_-]{35}                   |
                    gh[psoru]_[A-Za-z0-9]{36,}               |
                    xox[baprs]-[A-Za-z0-9-]+                |
                    # OpenAI-style keys.
                    sk-[A-Za-z0-9_-]{8,}                    |
                    # Authorization headers (whitespace-terminated).
                    bearer\s+\S+                             |
                    basic\s+\S+                              |
                    # Standalone JWTs.
                    eyJ[A-Za-z0-9_-]*(?:\.[A-Za-z0-9_-]+){2,4} |
                    # `X-Api-Key:` / `X-Auth-Token:` style vendor headers
                    # (colon or equals as separator).
                    x-(?:api[-_]?key|auth[-_]?token|secret|token|access[-_]?token)\s*[:=]\s*\S+ |
                    # Generic config dump shapes — `password=foo`,
                    # `secret=bar`, `client_secret=baz`, `private_key=qux`.
                    (?:password|secret|client_secret|private_key)\s*=\s*[^\s,;'\"&]+ |
                    # Query-parameter shapes — bounded so a trailing URL
                    # fragment (`&next=…`, `#…`) is not consumed.
                    apikey=[^\s&=#?]+                       |
                    token=[^\s&=#?]+                        |
                    key=[^\s&=#?]+                          |
                )"
            ),
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
    fn redacts_anthropic_style_keys() {
        let out = redact_secrets("using sk-ant-api03-abcdefgh12345678abcdefgh12");
        assert_eq!(out, "using ***");
    }

    #[test]
    fn redacts_stripe_live_keys() {
        let out = redact_secrets("paid via sk_live_abcdefgh12345678");
        assert_eq!(out, "paid via ***");
    }

    #[test]
    fn redacts_aws_access_keys() {
        let out = redact_secrets("creds: AKIAIOSFODNN7EXAMPLE leaked");
        assert_eq!(out, "creds: *** leaked");
    }

    #[test]
    fn redacts_github_personal_access_tokens() {
        let out = redact_secrets("oauth: ghp_abcdefghijklmnopqrstuvwxyz0123456789");
        assert_eq!(out, "oauth: ***");
    }

    #[test]
    fn redacts_google_api_keys() {
        let out = redact_secrets("key=AIzaSyA-aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456");
        assert_eq!(out, "key=***");
    }

    #[test]
    fn redacts_slack_tokens() {
        let slack_token = concat!("xoxb-", "1234567890-", "abcdefghijklmnopqrstuvwx");
        let out = redact_secrets(&format!("webhook {slack_token} leaked"));
        assert_eq!(out, "webhook *** leaked");
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
    fn redacts_x_api_key_headers() {
        assert_eq!(
            redact_secrets("X-API-Key: abcdefgh12345678 sent"),
            "*** sent"
        );
        assert_eq!(
            redact_secrets("X-Auth-Token=abcdefgh12345678 sent"),
            "*** sent"
        );
        assert_eq!(
            redact_secrets("x-secret: abcdefgh12345678 sent"),
            "*** sent"
        );
    }

    #[test]
    fn redacts_generic_password_secret_shapes() {
        assert_eq!(
            redact_secrets("config dump: password=hunter2 leaked"),
            "config dump: *** leaked"
        );
        assert_eq!(
            redact_secrets("body: secret=topsecret123 in the clear"),
            "body: *** in the clear"
        );
        assert_eq!(
            redact_secrets("oauth body=client_secret=topsecret123 json"),
            "oauth body=*** json"
        );
        assert_eq!(
            redact_secrets("loaded private_key=-----BEGIN PRIVATE KEY-----ABCDEFGHIJKLMNOPQRSTUV"),
            "loaded ***"
        );
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
    fn query_param_redaction_does_not_swallow_url_fragments() {
        // Greedy `\S+` would consume `&next=https://other/?key=xyz` and
        // leave the operator unable to inspect the `next=` redirect. The
        // bounded `[^\s&=#?]+` stops at the parameter boundary instead.
        let out = redact_secrets(
            "https://api.example.com/?token=abc123&next=https://other.example/?key=xyz",
        );
        assert!(
            out.contains("&next=https://other.example/"),
            "the next= redirect must survive redaction; got {out}"
        );
        assert!(!out.contains("token=abc123"), "token must be masked: {out}");
        assert!(!out.contains("?key=xyz"), "trailing key= must be masked: {out}");
    }

    #[test]
    fn leaves_ordinary_text_untouched() {
        let plain = "connection refused (os error 111)";
        assert_eq!(redact_secrets(plain), plain);
        // A bare `key` without `=` is not a credential shape.
        assert_eq!(redact_secrets("the key insight"), "the key insight");
        // Short sk- fragments are not key-shaped.
        assert_eq!(redact_secrets("sk-short"), "sk-short");
        // AKIA-shaped strings under the 16-char suffix are not key-shaped.
        assert_eq!(redact_secrets("AKIA-short"), "AKIA-short");
    }

    #[test]
    fn redacts_multiple_secrets_in_one_string() {
        let out = redact_secrets("sk-abcdefgh1234 and Bearer xyz both leaked");
        assert_eq!(out, "*** and *** both leaked");
    }
}
