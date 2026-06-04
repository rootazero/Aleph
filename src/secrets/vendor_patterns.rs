//! Shared high-confidence vendor credential patterns.
//!
//! Single source of truth for distinctive vendor API-key / token *prefix*
//! shapes, consumed by BOTH the char-level network egress guard
//! ([`super::leak_detector::LeakDetector`]) and the byte-level sandbox stdout
//! scrubber ([`super::leak_detector::default_patterns_bytes`]). The source
//! strings are regex-flavor-agnostic, so the same catalog compiles to both
//! `regex::Regex` and `regex::bytes::Regex`.
//!
//! These ADD to — they do not replace — the legacy high-confidence patterns
//! already hard-coded in each consumer, so existing redaction/blocking
//! behaviour is byte-identical for inputs the old catalog already matched.
//!
//! Only distinctive, near-zero-false-positive *prefixed* credentials belong
//! here: the network guard hard-*blocks* the entire request on a match, so
//! broad shapes (bare JWTs, `password=` assignments) that the reference
//! agents only ever apply to non-blocking *log* redaction are deliberately
//! excluded to avoid blocking legitimate prompts.

/// `(label, regex_source)` pairs.
///
/// `label` is the human-readable secret class surfaced in leak-block reasons;
/// `regex_source` is the flavor-agnostic pattern body (no anchors so it can be
/// scanned mid-text, identical to the legacy catalogs).
pub const VENDOR_SECRET_PATTERNS: &[(&str, &str)] = &[
    // Slack bot/user/refresh tokens (`xoxb-`, `xoxp-`, …) and app-level tokens.
    ("Slack Token", r"xox[baprs]-[A-Za-z0-9-]{10,}"),
    ("Slack App Token", r"xapp-[0-9]-[A-Z0-9]+-[0-9]+-[a-zA-Z0-9]+"),
    // Stripe secret/restricted keys (publishable `pk_` keys are intentionally
    // omitted — they are not secret).
    ("Stripe Secret Key", r"(?:sk|rk)_(?:live|test)_[0-9a-zA-Z]{20,}"),
    ("SendGrid API Key", r"SG\.[A-Za-z0-9_\-]{16,}\.[A-Za-z0-9_\-]{16,}"),
    ("Hugging Face Token", r"hf_[A-Za-z0-9]{30,}"),
    ("Replicate Token", r"r8_[A-Za-z0-9]{30,}"),
    ("Groq API Key", r"gsk_[A-Za-z0-9]{40,}"),
    ("xAI API Key", r"xai-[A-Za-z0-9\-]{20,}"),
    ("Perplexity API Key", r"pplx-[A-Za-z0-9]{32,}"),
    ("npm Token", r"npm_[A-Za-z0-9]{36}"),
    ("PyPI Token", r"pypi-[A-Za-z0-9_\-]{16,}"),
    ("DigitalOcean Token", r"dop_v1_[a-f0-9]{64}"),
    ("Google OAuth Token", r"ya29\.[A-Za-z0-9_\-]{20,}"),
    // `<bot-id>:<35-char-secret>` — distinctive enough for the blocking path.
    ("Telegram Bot Token", r"\d{8,10}:[A-Za-z0-9_-]{35}"),
    ("Twilio API Key", r"SK[0-9a-fA-F]{32}"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;

    #[test]
    fn all_vendor_patterns_compile() {
        for (label, src) in VENDOR_SECRET_PATTERNS {
            Regex::new(src).unwrap_or_else(|e| panic!("{label} pattern failed to compile: {e}"));
        }
    }

    #[test]
    fn vendor_patterns_compile_as_bytes() {
        for (label, src) in VENDOR_SECRET_PATTERNS {
            regex::bytes::Regex::new(src)
                .unwrap_or_else(|e| panic!("{label} bytes pattern failed to compile: {e}"));
        }
    }

    #[test]
    fn labels_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for (label, _) in VENDOR_SECRET_PATTERNS {
            assert!(seen.insert(*label), "duplicate vendor label: {label}");
        }
    }
}
