//! Shared secret detection patterns for `LeakDetector` and `SecretMasker`.
//!
//! All secret patterns are defined here in one place to prevent drift
//! between the two consumers.

use regex::Regex;
use std::sync::OnceLock;

#[derive(Clone)]
pub(crate) struct SecretPattern {
    pub regex: Regex,
    pub replacement: &'static str,
}

#[derive(Clone)]
pub(crate) struct LeakPatternDef {
    pub name: &'static str,
    pub regex: Regex,
    pub action: super::leak_detector::LeakAction,
}

#[derive(Clone)]
pub(crate) struct LeakDetectorAssets {
    pub patterns: Vec<LeakPatternDef>,
}

#[must_use]
pub(crate) fn secret_masker_patterns() -> Vec<SecretPattern> {
    static PATTERNS: OnceLock<Vec<SecretPattern>> = OnceLock::new();
    PATTERNS
        .get_or_init(|| {
            vec![
                SecretPattern {
                    regex: Regex::new(r"\bsk-[a-zA-Z0-9]{20,}").unwrap(),
                    replacement: "sk-***REDACTED***",
                },
                SecretPattern {
                    regex: Regex::new(r"\bsk-ant-[a-zA-Z0-9\-]{20,}").unwrap(),
                    replacement: "sk-ant-***REDACTED***",
                },
                SecretPattern {
                    // `\b` anchors the start so an in-string occurrence of
                    // `AIza` (e.g. embedded in a longer base64) is not
                    // matched. The body (`[a-zA-Z0-9_-]{35}`) already
                    // requires 35 chars after the prefix, so the false-
                    // positive surface is the asymmetric edge cases (a
                    // password manager URL with `AIza` mid-string), and
                    // the cost of an over-match in redaction is just
                    // "this word is gone" — a sentence the model can
                    // always ask the user about.
                    regex: Regex::new(r"\bAIza[a-zA-Z0-9_\-]{35}").unwrap(),
                    replacement: "AIza***REDACTED***",
                },
                SecretPattern {
                    regex: Regex::new(r"AKIA[A-Z0-9]{16}").unwrap(),
                    replacement: "AKIA***REDACTED***",
                },
                SecretPattern {
                    regex: Regex::new(r#"(?i)(aws_secret_access_key|secret_access_key)\s*[=:]\s*['"]?([a-zA-Z0-9/+=]{40})['"]?"#).unwrap(),
                    replacement: "$1=***REDACTED***",
                },
                SecretPattern {
                    regex: Regex::new(r"gh[pousr]_[a-zA-Z0-9]{36,}").unwrap(),
                    replacement: "gh*_***REDACTED***",
                },
                SecretPattern {
                    regex: Regex::new(r"github_pat_[A-Za-z0-9_]{50,}").unwrap(),
                    replacement: "github_pat_***REDACTED***",
                },
                SecretPattern {
                    regex: Regex::new(r#"(?i)(bearer|token|authorization)\s*[=:]\s*['"]?([a-zA-Z0-9\-_.]{20,})['"]?"#).unwrap(),
                    replacement: "$1=***REDACTED***",
                },
                SecretPattern {
                    regex: Regex::new(r"(?i)bearer\s+[a-zA-Z0-9\-._~+/]+=*").unwrap(),
                    replacement: "Bearer ***REDACTED***",
                },
                SecretPattern {
                    regex: Regex::new(r"(?i)(-u|--user)\s+[^\s:]+:[^\s]+").unwrap(),
                    replacement: "$1 ***REDACTED***",
                },
                SecretPattern {
                    regex: Regex::new(r"-----BEGIN[A-Z ]*PRIVATE KEY-----[\s\S]*?-----END[A-Z ]*PRIVATE KEY-----").unwrap(),
                    replacement: "-----BEGIN PRIVATE KEY-----\n***REDACTED***\n-----END PRIVATE KEY-----",
                },
                SecretPattern {
                    regex: Regex::new(r"://([^:]+):([^@]+)@").unwrap(),
                    replacement: "://$1:***REDACTED***@",
                },
                SecretPattern {
                    regex: Regex::new(r#"(?i)(password|passwd|pwd|secret)\s*[=:]\s*['"]?([^\s'"]{8,})['"]?"#).unwrap(),
                    replacement: "$1=***REDACTED***",
                },
                SecretPattern {
                    regex: Regex::new(r"xox[baprs]-[a-zA-Z0-9\-]{10,}").unwrap(),
                    replacement: "xox*-***REDACTED***",
                },
                SecretPattern {
                    regex: Regex::new(r"[MN][A-Za-z\d]{23,}\.[\w-]{6}\.[\w-]{27}").unwrap(),
                    replacement: "***DISCORD_TOKEN_REDACTED***",
                },
            ]
        })
        .clone()
}

#[must_use]
pub(crate) fn leak_detector_assets() -> LeakDetectorAssets {
    static ASSETS: OnceLock<LeakDetectorAssets> = OnceLock::new();
    ASSETS
        .get_or_init(|| {
            let patterns = vec![
                LeakPatternDef {
                    name: "openai_key",
                    regex: Regex::new(r"\bsk-[a-zA-Z0-9]{20,}").unwrap(),
                    action: super::leak_detector::LeakAction::Block,
                },
                LeakPatternDef {
                    name: "anthropic_key",
                    regex: Regex::new(r"\bsk-ant-[a-zA-Z0-9\-]{20,}").unwrap(),
                    action: super::leak_detector::LeakAction::Block,
                },
                LeakPatternDef {
                    name: "google_api_key",
                    // `\b` anchor — see the masker entry for the same body.
                    // Without it, the pattern matches `AIza…` mid-string and
                    // gives every leak detector a "google_api_key" finding
                    // for an in-string coincidence. False positives in the
                    // leak path are worse than in the masker path: a
                    // `Block` finding is refused from the LLM.
                    regex: Regex::new(r"\bAIza[a-zA-Z0-9_\-]{35}").unwrap(),
                    action: super::leak_detector::LeakAction::Block,
                },
                LeakPatternDef {
                    name: "aws_access_key",
                    regex: Regex::new(r"AKIA[A-Z0-9]{16}").unwrap(),
                    action: super::leak_detector::LeakAction::Block,
                },
                LeakPatternDef {
                    name: "github_token",
                    regex: Regex::new(r"gh[pousr]_[a-zA-Z0-9]{36,}").unwrap(),
                    action: super::leak_detector::LeakAction::Block,
                },
                LeakPatternDef {
                    name: "github_fine_grained_pat",
                    regex: Regex::new(r"github_pat_[A-Za-z0-9_]{50,}").unwrap(),
                    action: super::leak_detector::LeakAction::Block,
                },
                LeakPatternDef {
                    name: "slack_token",
                    regex: Regex::new(r"xox[baprs]-[a-zA-Z0-9\-]{10,}").unwrap(),
                    action: super::leak_detector::LeakAction::Block,
                },
                LeakPatternDef {
                    name: "private_key",
                    regex: Regex::new(r"-----BEGIN[A-Z ]*PRIVATE KEY-----").unwrap(),
                    action: super::leak_detector::LeakAction::Block,
                },
                LeakPatternDef {
                    name: "bearer_token",
                    regex: Regex::new(r"(?i)bearer\s+[a-zA-Z0-9\-._~+/]+=*").unwrap(),
                    action: super::leak_detector::LeakAction::Redact,
                },
            ];

            LeakDetectorAssets { patterns }
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masker_patterns_detect_openai_key() {
        for p in secret_masker_patterns() {
            if p.regex
                .is_match("sk-abcdefghijklmnopqrstuvwxyz123456789012345678")
            {
                return;
            }
        }
        panic!("OpenAI key pattern should exist in masker patterns");
    }

    #[test]
    fn openai_pattern_in_both() {
        let masker = secret_masker_patterns();
        let leak_assets = leak_detector_assets();
        let openai = leak_assets
            .patterns
            .iter()
            .find(|p| p.name == "openai_key")
            .unwrap();
        let found = masker
            .iter()
            .any(|mp| mp.regex.as_str() == openai.regex.as_str());
        assert!(found, "openai_key regex should be identical in both");
    }

    #[test]
    fn openai_key_ignores_word_internal_sk() {
        // Regression: "task-<uuid>" / "elon-musk-..." contain "sk-" mid-word and
        // must NOT be treated as an OpenAI key by either the masker or the leak
        // detector (the latter's openai_key action is Block). Real keys at a
        // boundary still match.
        // The exec patterns use a hyphen-free body (`sk-[a-zA-Z0-9]{20,}`), so a
        // false positive needs "·sk-" followed by 20+ CONTIGUOUS alnum — e.g. a
        // URL slug "elon-musk-<run>" or "disk-<run>".
        let benign =
            "see elon-musk-teslarobotaxiupdate2025q3report and disk-cleanuputility1234567890";
        let real = "OPENAI_API_KEY=sk-abcdefghijklmnopqrstuvwxyz1234";

        let leak = leak_detector_assets();
        let openai = leak
            .patterns
            .iter()
            .find(|p| p.name == "openai_key")
            .unwrap();
        assert!(
            !openai.regex.is_match(benign),
            "leak openai_key false-matched a word-internal sk-"
        );
        assert!(
            openai.regex.is_match(real),
            "leak openai_key missed a real boundary key"
        );

        let masker_openai = secret_masker_patterns()
            .into_iter()
            .find(|p| p.replacement == "sk-***REDACTED***")
            .unwrap();
        assert!(
            !masker_openai.regex.is_match(benign),
            "masker openai pattern false-matched a word-internal sk-"
        );
        assert!(
            masker_openai.regex.is_match(real),
            "masker openai pattern missed a real boundary key"
        );
    }

    #[test]
    fn github_token_pattern_in_both() {
        let masker = secret_masker_patterns();
        let leak_assets = leak_detector_assets();
        let github = leak_assets
            .patterns
            .iter()
            .find(|p| p.name == "github_token")
            .unwrap();
        let found = masker
            .iter()
            .any(|mp| mp.regex.as_str() == github.regex.as_str());
        assert!(found, "github_token regex should be identical in both");
    }
}
