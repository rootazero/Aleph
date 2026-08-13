//! API key and token detection
//!
//! Uses prefix-based matching only to avoid URL slug false positives.
//! Patterns cover major API providers with known prefixes.

use crate::pii::engine::{PiiMatch, PiiSeverity};
use crate::pii::rules::PiiRule;
use regex::Regex;
use std::sync::OnceLock;

static API_KEY_RE: OnceLock<Regex> = OnceLock::new();

fn api_key_regex() -> &'static Regex {
    API_KEY_RE.get_or_init(|| {
        Regex::new(
            r"(?x)
            \b                                    # anchor at a word boundary so
                                                  # prefixes (sk-, gho_, ...) are
                                                  # not matched inside larger
                                                  # tokens (e.g. ta`sk-`...)
            (?:
                sk-[a-zA-Z0-9\-_]{20,}           # OpenAI / Anthropic style
                | ghp_[a-zA-Z0-9]{36,}            # GitHub Personal Access Token
                | gho_[a-zA-Z0-9]{36,}            # GitHub OAuth
                | github_pat_[a-zA-Z0-9_]{40,}    # GitHub Fine-grained PAT
                | glpat-[a-zA-Z0-9\-_]{20,}       # GitLab Personal Access Token
                | AKIA[A-Z0-9]{16}                # AWS Access Key ID
                | AIza[0-9A-Za-z\-_]{35}          # Google API key
                | sk_live_[0-9a-zA-Z]{16,}        # Stripe live secret key
                | hf_[A-Za-z0-9]{20,}             # HuggingFace token
                | sk-or-v1-[A-Za-z0-9]{20,}       # OpenRouter
                | pplx-[A-Za-z0-9]{20,}           # Perplexity
                | xox[abprse]-[a-zA-Z0-9\-]{10,}  # Slack tokens (b/p/a/r/s/e)
                | tvly-[a-zA-Z0-9\-_]{20,}        # Tavily
                | (?i:Bearer)\s+[a-zA-Z0-9._\-]{20,}  # RFC 7235 says case-insensitive
            )
            ",
        )
        // rust-doctor-disable-next-line unwrap-in-production
        .expect("static API key regex compiles")
    })
}

pub struct ApiKeyRule;

impl ApiKeyRule {
    pub const fn new() -> Self {
        Self
    }
}

impl PiiRule for ApiKeyRule {
    fn name(&self) -> &str {
        "api_key"
    }
    fn severity(&self) -> PiiSeverity {
        PiiSeverity::Critical
    }
    fn placeholder(&self) -> &str {
        "[REDACTED]"
    }

    fn detect(&self, text: &str) -> Vec<PiiMatch> {
        let re = api_key_regex();
        let mut results = Vec::new();

        for m in re.find_iter(text) {
            results.push(PiiMatch {
                rule_name: self.name().to_string(),
                start: m.start(),
                end: m.end(),
                matched_text: m.as_str().to_string(),
                severity: self.severity(),
                placeholder: self.placeholder().to_string(),
            });
        }

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule() -> ApiKeyRule {
        ApiKeyRule::new()
    }

    #[test]
    fn test_detect_openai_key() {
        let matches = rule().detect("key: sk-1234567890abcdefghijklmnopqrstuvwxyz123456");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_detect_anthropic_key() {
        let matches = rule().detect(
            "ANTHROPIC_API_KEY=sk-ant-api03-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        );
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_detect_github_pat() {
        let matches = rule().detect("token: ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_detect_aws_key() {
        let matches = rule().detect("AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_no_match_short_sk() {
        // sk- prefix but too short
        let matches = rule().detect("sk-short");
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_detect_bearer_token() {
        let matches = rule().detect("Authorization: Bearer eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_no_match_sk_inside_larger_token() {
        // "task-..." contains the substring "sk-" but must NOT be redacted:
        // the leading word boundary prevents matching prefixes mid-token.
        let matches = rule().detect("task-1234567890abcdefghij12");
        assert_eq!(
            matches.len(),
            0,
            "prefix inside a larger word token must not match"
        );
    }

    #[test]
    fn test_detect_sk_at_word_boundary() {
        // Preceded by '=' (non-word) the boundary still allows a real key.
        let matches = rule().detect("OPENAI_API_KEY=sk-1234567890abcdefghijklmnop");
        assert_eq!(matches.len(), 1);
    }

    // === Case-fold (B3-H1) ===

    #[test]
    fn test_detect_bearer_lowercase() {
        // RFC 7235: the Bearer scheme is case-insensitive. `bearer`
        // (lowercase) was previously missed.
        let matches = rule().detect("Authorization: bearer eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCIsInRp");
        assert_eq!(matches.len(), 1, "lowercase bearer must be detected");
    }

    #[test]
    fn test_detect_bearer_uppercase() {
        let matches = rule().detect("Authorization: BEARER eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCIsInRp");
        assert_eq!(matches.len(), 1, "uppercase bearer must be detected");
    }

    // === Additional API-key families (B3-H2) ===

    #[test]
    fn test_detect_google_api_key() {
        let matches = rule().detect("AIzaSyA-abcdefghijklmnopqrstuvwxyz012345");
        assert_eq!(matches.len(), 1, "Google API key (AIza) must be detected");
    }

    #[test]
    fn test_detect_gitlab_pat() {
        let matches = rule().detect("glpat-abcdefghijklmnopqrstuv");
        assert_eq!(matches.len(), 1, "GitLab PAT (glpat-) must be detected");
    }

    #[test]
    fn test_detect_stripe_live_key() {
        let matches = rule().detect("sk_live_abcdefghijklmnopqrstuvwx");
        assert_eq!(matches.len(), 1, "Stripe live key must be detected");
    }

    #[test]
    fn test_detect_huggingface_token() {
        let matches = rule().detect("hf_abcdefghijklmnopqrstuv");
        assert_eq!(matches.len(), 1, "HuggingFace token must be detected");
    }

    #[test]
    fn test_detect_openrouter_key() {
        let matches = rule().detect("sk-or-v1-abcdefghijklmnopqrstuvwxyz0123456789");
        assert_eq!(
            matches.len(),
            1,
            "OpenRouter key (sk-or-v1-) must be detected"
        );
    }

    // === Slack Enterprise (B3-H3) ===

    #[test]
    fn test_detect_slack_enterprise_token() {
        // xoxe- is the Slack Enterprise token family; previously missed
        // because the character class was [bpras].
        let matches = rule().detect("xoxe-abcdefghij1234567890");
        assert_eq!(matches.len(), 1, "Slack Enterprise token must be detected");
    }

    #[test]
    fn test_no_match_short_ai_in_word() {
        // Anti-false-positive guard: `AIza` must not match inside a larger
        // word like `Aiza` (no real Google key).
        let matches = rule().detect("Aiza1234567890123456789012345678901234");
        assert_eq!(
            matches.len(),
            0,
            "AIza inside a non-Google token must not match (word boundary anchor)"
        );
    }
}
