//! Shared secret detection patterns for `LeakDetector` and `SecretMasker`.
//!
//! All secret patterns are defined here in one place to prevent drift
//! between the two consumers.
//!
//! **This file is NOT the repo's vendor catalog.** `crate::secrets::vendor_patterns`
//! knows Slack `xapp-`, Stripe, SendGrid, HuggingFace, Replicate, Groq, xAI,
//! Perplexity, npm, PyPI, DigitalOcean, Google OAuth, Telegram, Twilio and
//! GitLab credentials that this catalog does not: the two overlap only
//! partially. The gap is deliberate for now — which of the two is the single
//! source of truth is an owner decision, and copying one into the other
//! would make that decision silently instead of explicitly.

use regex::Regex;
use std::borrow::Cow;
use std::sync::OnceLock;

const PRIVATE_KEY_BEGIN: &str = r"-----BEGIN[A-Z ]*PRIVATE KEY-----";
const PRIVATE_KEY_END: &str = r"-----END[A-Z ]*PRIVATE KEY-----";

/// Redact a truncated PEM tail before complete blocks receive their replacement.
pub(crate) fn mask_incomplete_private_key(text: &str) -> Cow<'_, str> {
    static MARKERS: OnceLock<(Regex, Regex)> = OnceLock::new();
    let markers = MARKERS.get_or_init(|| {
        (
            Regex::new(PRIVATE_KEY_BEGIN).unwrap(),
            Regex::new(PRIVATE_KEY_END).unwrap(),
        )
    });
    let mut cursor = 0;
    while let Some(begin) = markers.0.find_at(text, cursor) {
        if let Some(end) = markers.1.find_at(text, begin.end()) {
            cursor = end.end();
            continue;
        }
        return Cow::Owned(format!(
            "{}-----BEGIN PRIVATE KEY----- ***REDACTED***",
            &text[..begin.start()]
        ));
    }
    Cow::Borrowed(text)
}

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
                    // The key may be quoted, and in JSON the key's closing
                    // quote sits BETWEEN the name and the separator — so a
                    // rule that wants the separator right after the name can
                    // never fire on a JSON document and the credential ships
                    // in clear.
                    //
                    // The prefix is CAPTURED rather than rebuilt because only
                    // the value is the secret: the key text, the separator and
                    // its spacing, and the quotes are structure the consumer
                    // downstream still has to parse. Re-emitting the captured
                    // prefix is what keeps the redacted document valid JSON.
                    regex: Regex::new(r#"(?i)(aws_secret_access_key|secret_access_key)(\s*["']?\s*[=:]\s*["']?)([a-zA-Z0-9/+=]{40,})(["']?)"#).unwrap(),
                    replacement: "${1}${2}***REDACTED***${4}",
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
                    // Same quoted-key gap as the AWS rule above, and the value
                    // alphabet was too narrow for a real credential: standard
                    // base64 carries `+`, `/` and `=` padding, so
                    // `token=abc+def/…` sailed straight past the old class.
                    //
                    // Two shapes, because the separator decides the shape: the
                    // header form (`key: value`) may carry an auth scheme
                    // word before the blob (`Authorization: Basic <base64>`)
                    // — one word plus one space — while the `key=value` form
                    // is a single unbroken run. The floor of 20 characters
                    // applies to the final run in both shapes, so a short
                    // `token=x` is still left alone.
                    //
                    // `api_key` joins the key names here: it is a credential
                    // key this catalog had no rule for at all, and the JSON
                    // leg could not reach it.
                    regex: Regex::new(r#"(?i)(bearer|token|authorization|api_key)(\s*["']?\s*[=:]\s*["']?)((?:[a-zA-Z0-9\-_.+/=]+\s+)?[a-zA-Z0-9\-_.+/=]{20,})(["']?)"#).unwrap(),
                    replacement: "${1}${2}***REDACTED***${4}",
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
                    regex: Regex::new(&format!("{PRIVATE_KEY_BEGIN}[\\s\\S]*?{PRIVATE_KEY_END}")).unwrap(),
                    replacement: "-----BEGIN PRIVATE KEY-----\n***REDACTED***\n-----END PRIVATE KEY-----",
                },
                SecretPattern {
                    // A key whose END marker is not in the text is the
                    // REACHABLE case, not the exotic one: `mask_file_change`
                    // masks the hunk lines, so an edit that touches only the
                    // first lines of an existing key file ships the header
                    // plus the first base64 lines of the body. A key body
                    // continues past the text, so the only safe reading is
                    // "from the BEGIN marker to the end of the text".
                    //
                    // What keeps this rule off a COMPLETE block is the rule
                    // above: it already replaced every one of those, and
                    // what it leaves behind starts its body line with the
                    // redaction marker `***REDACTED***` — a `*`, which a
                    // base64 key body never starts with. So the first
                    // character of the body is what separates "a key body
                    // follows" from "a redacted block was already handled
                    // here". The `*` is also what keeps this rule from
                    // eating the rest of the document after that frame.
                    //
                    // That test is a character class and not a negative
                    // condition because the `regex` crate has NO look-around:
                    // a pattern naming it as a negative condition does not
                    // COMPILE, and since this catalog is compiled lazily the
                    // failure is a panic in the first `mask()` call, not a
                    // build error — `cargo check` cannot see it.
                    //
                    // The body may open with `\r\n` (a key file written on
                    // Windows), so the newline is matched with its optional
                    // carriage return. The second alternative is the key body
                    // on the SAME line as the header; it requires a base64
                    // character so that this rule's own replacement — which
                    // puts a space after the header — is not matched again.
                    //
                    // `mask_incomplete_private_key` above already truncates a
                    // key whose END marker is missing, so this rule is the
                    // SECOND line of defense: it catches the case where a
                    // later END marker belongs to a DIFFERENT key and the
                    // helper therefore let the first key through.
                    //
                    // The replacement is one line on purpose: a multi-line
                    // replacement written back into a single hunk line would
                    // make the diff's line counts lie.
                    regex: Regex::new(r"-----BEGIN[A-Z ]*PRIVATE KEY-----(?:\r?\n[^*][\s\S]*|[A-Za-z0-9+/=][\s\S]*)").unwrap(),
                    replacement: "-----BEGIN PRIVATE KEY----- ***REDACTED***",
                },
                SecretPattern {
                    // A URL credential cannot contain whitespace, so the
                    // value class stops at it: the old `[^@]+` ran across
                    // spaces and ate the port, the path and the following
                    // prose of a URL that had no credential at all.
                    //
                    // The password may contain `@` itself, so the class is
                    // greedy over the whole non-whitespace run and the match
                    // settles on its LAST `@` — stopping at the first one
                    // left the tail of the password in clear.
                    regex: Regex::new(r"://([^\s:/@]+):(\S*)@").unwrap(),
                    replacement: "://${1}:***REDACTED***@",
                },
                SecretPattern {
                    // Same quoted-key gap as the AWS and token rules, and the
                    // same reason for capturing the prefix: `db_password = "…"`
                    // must keep its spacing and its quotes, because only the
                    // value is the secret. The 8-character floor stays.
                    regex: Regex::new(r#"(?i)(password|passwd|pwd|secret)(\s*["']?\s*[=:]\s*["']?)([^\s'"]{8,})(["']?)"#).unwrap(),
                    replacement: "${1}${2}***REDACTED***${4}",
                },
                SecretPattern {
                    regex: Regex::new(r"xox[baprs]-[a-zA-Z0-9\-]{10,}").unwrap(),
                    replacement: "xox*-***REDACTED***",
                },
                SecretPattern {
                    // Discord bot tokens: <id>.<timestamp>.<hmac>, three dot-separated
                    // segments. The leading `[MN]` plus 24-char body is the user-id
                    // segment, the middle is the timestamp base64, the trailing is the
                    // HMAC. Word-boundary anchored on both sides so an in-string
                    // coincidence (`"id: Mxxxxxxxxxx.xxxxxx.xxxxxx"` mid-base64) is not
                    // over-redacted — the masker cost on a false match is just
                    // "this word is gone" but the leak path Block arm refuses the LLM.
                    regex: Regex::new(r"\b[MN][A-Za-z\d]{23,}\.[\w-]{6,7}\.[\w-]{27,}\b").unwrap(),
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
                    regex: Regex::new(PRIVATE_KEY_BEGIN).unwrap(),
                    action: super::leak_detector::LeakAction::Block,
                },
                LeakPatternDef {
                    name: "discord_bot_token",
                    regex: Regex::new(r"\b[MN][A-Za-z\d]{23,}\.[\w-]{6,7}\.[\w-]{27,}\b").unwrap(),
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

    /// Every pattern in this catalog must COMPILE, and the only place that
    /// can be asserted is a test: the `regex` crate rejects a pattern it
    /// cannot express, and this catalog is built inside a `OnceLock`, so a
    /// pattern that does not compile is not a build error — it is a PANIC in
    /// the first `mask()` call, on whatever live path masks first.
    /// `cargo check` cannot see it.
    ///
    /// The look-around assertion is the specific regression: the `regex`
    /// crate has NO look-around, so a pattern naming it as a negative
    /// condition (`(?!…)`) does not compile. One did, in the truncated-PEM
    /// rule, and the catalog panicked on first use.
    #[test]
    fn every_catalog_pattern_compiles_without_look_around() {
        // `Regex::new` already panicked on an inexpressible pattern, so
        // reaching this line means every pattern compiled.
        let masker = secret_masker_patterns();
        let leak_assets = leak_detector_assets();
        assert!(!masker.is_empty(), "the masker catalog must not be empty");
        assert!(
            !leak_assets.patterns.is_empty(),
            "the leak detector catalog must not be empty"
        );

        // The `regex` crate supports `(?i)`, `(?s)`, `(?m)`, `(?x)` and
        // `(?:`, and supports NO look-around: not `(?=`, not `(?!`, not
        // `(?<`.
        for source in masker
            .iter()
            .map(|p| p.regex.as_str())
            .chain(leak_assets.patterns.iter().map(|p| p.regex.as_str()))
        {
            for look_around in ["(?=", "(?!", "(?<"] {
                assert!(
                    !source.contains(look_around),
                    "the `regex` crate has no look-around, and this pattern \
                     names one ({look_around}): {source:?}"
                );
            }
        }
    }
}
