//! Bidirectional secret leak detection.
//!
//! Scans outbound requests and inbound responses for leaked secret values.
//! Uses two detection strategies:
//! 1. Pattern rules - known secret formats (sk-ant-*, AKIA*, etc.)
//! 2. Injected-value detection — substring match of recently injected secrets,
//!    performed over `(hash, length)` fingerprints so no plaintext secret is
//!    ever retained by the detector (see [`LeakDetector::scan_inbound`]).

use std::hash::{Hash, Hasher};

use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::{BTreeSet, HashSet};

use super::injection::{InjectedSecret, INJECTED_HASH_KEY0, INJECTED_HASH_KEY1};

const REDACTED_LEAK: &str = "***LEAKED_REDACTED***";
const REDACTED_INJECTED: &str = "***INJECTED_REDACTED***";

/// Shortest secret tracked for injected-value substring matching. Below this a
/// window match is more likely to be a coincidence in prose than a leak, and
/// short values are already covered by the pattern rules.
const MIN_INJECTED_MATCH_LEN: usize = 8;

/// Result of a leak scan.
#[derive(Debug, Clone)]
pub enum LeakDecision {
    /// Content is safe to proceed.
    Allow,
    /// Content contains a leaked secret and must be blocked.
    Block {
        reason: String,
        redacted_content: String,
    },
}

impl LeakDecision {
    #[must_use]
    pub const fn is_blocked(&self) -> bool {
        matches!(self, Self::Block { .. })
    }
}

/// Source-of-truth secret regex strings for the byte-level sandbox scrubber.
///
/// NOTE: This list is intentionally independent from the `LEAK_PATTERNS`
/// below. The str-side detector pre-dates this list and uses different pattern
/// boundaries. Refactoring `LEAK_PATTERNS` to share this list would change
/// existing redaction behavior and break tests — kept separate by design.
///
/// Ordering matters: the specific prefixes (`sk_proj`/`sk_ant`) run before the
/// generic `openai_sk` catch-all so they win the named redaction tag; once a
/// match is redacted to `[REDACTED:NAME]` the generic rule no longer matches
/// it. The trailing three entries (generic `OpenAI` key, Google API key, PEM
/// private-key header) mirror the high-confidence `LEAK_PATTERNS` so that
/// sandbox stdout/stderr is scrubbed of the same secrets the str-side detector
/// blocks on the network path — closing a previously open leak path where a
/// command printing a classic `sk-…`, `AIza…`, or PEM key to stdout was
/// returned to the model un-redacted.
pub const SECRET_PATTERN_SOURCES: &[(&str, &str)] = &[
    // `\b` left word-boundary: the `sk-` family must not match inside ordinary
    // words that merely contain the substring "sk-" (e.g. "elon-mu`sk-`tesla…",
    // "ta`sk-`<uuid>"). Mirrors pii/rules/api_key.rs.
    ("sk_proj", r"\bsk-proj-[A-Za-z0-9_\-]{20,}"),
    ("sk_ant", r"\bsk-ant-[A-Za-z0-9_\-]{20,}"),
    ("aws_akia", r"AKIA[0-9A-Z]{16}"),
    ("github_pat", r"ghp_[A-Za-z0-9]{20,}"),
    ("gitlab_pat", r"glpat-[A-Za-z0-9_\-]{20,}"),
    ("openai_sk", r"\bsk-[a-zA-Z0-9\-]{20,}"),
    ("google_api", r"AIza[a-zA-Z0-9_\-]{35}"),
    // `[A-Z ]*` (zero-or-more, no forced algorithm word) so the canonical PKCS#8
    // header `-----BEGIN PRIVATE KEY-----` matches alongside the algorithm-tagged
    // `-----BEGIN RSA PRIVATE KEY-----`. Mirrors exec/secret_patterns.rs so the
    // two catalogs' private-key floors cannot diverge (a `[A-Z ]+` form silently
    // let bare PKCS#8 keys slip the block-class gate below).
    ("private_key", r"-----BEGIN[A-Z ]*PRIVATE KEY-----"),
];

/// Catastrophic ("block-class") secret pattern names — a curated subset of
/// [`SECRET_PATTERN_SOURCES`] whose appearance in **sandboxed command output**
/// is treated as fail-closed: the output is refused rather than merely redacted.
///
/// This is the shell-output analogue of clawshell's `DlpAction::Block` (versus
/// the default `Redact`). It is intentionally minimal — limited to categories
/// that have essentially no legitimate reason to be echoed to the model and a
/// near-zero false-positive rate. A PEM `PRIVATE KEY` block in command stdout
/// means a key file is being dumped; redacting the literal bytes still returns
/// the surrounding context to the model, so the worst class fails closed.
///
/// API-token shapes (`sk-…`, `ghp_…`, `AKIA…`) are deliberately **excluded** —
/// they can legitimately surface in `env`/config inspection and are handled by
/// redaction so as not to break ordinary workflows. Like the
/// `command_policy` hardline floor, this is a frozen hard-filter, not a config knob.
pub const BLOCK_CLASS_SECRETS: &[&str] = &["private_key"];

/// Whether a named secret pattern (from [`SECRET_PATTERN_SOURCES`]) is
/// block-class. Cheap linear scan over the tiny frozen [`BLOCK_CLASS_SECRETS`].
#[must_use]
pub fn is_block_class_secret(name: &str) -> bool {
    BLOCK_CLASS_SECRETS.contains(&name)
}

/// Produce bytes-flavored regexes matching the same patterns as
/// `SECRET_PATTERN_SOURCES`, plus the shared high-confidence vendor catalog in
/// [`super::vendor_patterns::VENDOR_SECRET_PATTERNS`]. Used by `sandbox::scrub`
/// to redact raw stdout/stderr before any UTF-8 conversion.
///
/// The legacy `SECRET_PATTERN_SOURCES` entries run first so their named
/// redaction tags win for inputs they already matched (byte-identical scrub for
/// pre-existing patterns); the vendor catalog only widens coverage.
#[must_use]
pub fn default_patterns_bytes() -> &'static [(&'static str, regex::bytes::Regex)] {
    use std::sync::LazyLock;
    static DEFAULT_BYTE_PATTERNS: LazyLock<Vec<(&'static str, regex::bytes::Regex)>> =
        LazyLock::new(|| {
            SECRET_PATTERN_SOURCES
                .iter()
                .chain(super::vendor_patterns::VENDOR_SECRET_PATTERNS.iter())
                .map(|(name, src)| {
                    (
                        *name,
                        regex::bytes::Regex::new(src).expect("static pattern compiles"),
                    )
                })
                .collect()
        });
    &DEFAULT_BYTE_PATTERNS
}

/// Known secret format patterns.
///
/// The legacy high-confidence entries below run first; the shared vendor
/// catalog ([`super::vendor_patterns::VENDOR_SECRET_PATTERNS`]) is appended so
/// the network egress guard blocks the same distinctive vendor credentials the
/// byte-level sandbox scrubber redacts. Inputs the legacy entries already
/// matched keep byte-identical labels (first match wins the redaction tag).
static LEAK_PATTERNS: Lazy<Vec<(&str, Regex)>> = Lazy::new(|| {
    let mut patterns = vec![
        (
            "Anthropic API Key",
            // `\b` left-anchor — see SECRET_PATTERN_SOURCES note above.
            Regex::new(r"\bsk-ant-[a-zA-Z0-9\-]{20,}").expect("static Anthropic pattern compiles"),
        ),
        (
            "OpenAI API Key",
            Regex::new(r"\bsk-[a-zA-Z0-9\-]{20,}").expect("static OpenAI pattern compiles"),
        ),
        (
            "Google API Key",
            Regex::new(r"AIza[a-zA-Z0-9_\-]{35}").expect("static Google pattern compiles"),
        ),
        (
            "AWS Access Key",
            Regex::new(r"AKIA[A-Z0-9]{16}").expect("static AWS pattern compiles"),
        ),
        (
            "GitHub Token",
            Regex::new(r"gh[pousr]_[a-zA-Z0-9]{36,}").expect("static GitHub pattern compiles"),
        ),
        (
            // GitLab personal/group/project access tokens — mirrors SECRET_PATTERN_SOURCES entry
            "GitLab Token",
            Regex::new(r"glpat-[A-Za-z0-9_\-]{20,}").expect("static GitLab pattern compiles"),
        ),
        (
            "Private Key Block",
            // `[A-Z ]*` — see SECRET_PATTERN_SOURCES note: bare PKCS#8
            // `-----BEGIN PRIVATE KEY-----` must match, not only algorithm-tagged
            // variants. Kept identical to the bytes-side catalog above.
            Regex::new(r"-----BEGIN[A-Z ]*PRIVATE KEY-----")
                .expect("static private key pattern compiles"),
        ),
    ];
    patterns.extend(
        super::vendor_patterns::VENDOR_SECRET_PATTERNS
            .iter()
            .map(|(label, src)| {
                (
                    *label,
                    // rust-doctor-disable-next-line unwrap-in-production
                    Regex::new(src).expect("static vendor pattern compiles"),
                )
            }),
    );
    patterns
});

/// Compiled custom leak pattern for runtime use.
struct CompiledCustomPattern {
    name: String,
    regex: Regex,
}

/// Bidirectional leak detector for secret values.
pub struct LeakDetector {
    /// Fingerprints of registered secrets with bounded capacity. The previous
    /// unbounded `HashSet<u64>` grew for the lifetime of the process on every
    /// `register_injected` call (Aleph Server is a long-running daemon), so a
    /// workload that injected many distinct secrets would inflate memory and
    /// broaden false-positive matches over time. The LRU cap evicts the
    /// oldest fingerprints first; once evicted, an old secret value that
    /// re-appears in inbound content will no longer be flagged here (pattern
    /// rules still cover the leak case — they just won't tag it as
    /// "previously injected").
    injected_hashes: lru::LruCache<u64, ()>,
    /// Byte lengths of the registered secrets — the window sizes
    /// [`Self::scan_inbound`] slides over inbound content. Kept as a sorted
    /// set because a handful of distinct lengths is the norm and the scan
    /// cost is linear in their sum. Lengths are also LRU-bounded for the same
    /// reason as the hash set.
    injected_lens: lru::LruCache<usize, ()>,
    custom_patterns: Vec<CompiledCustomPattern>,
}

/// Cap on tracked fingerprints. Long enough to cover a multi-turn tool
/// resolution + a follow-up reflection prompt (typical: < 50 entries); small
/// enough that memory and scan cost stay bounded under adversarial workloads
/// (e.g. a host that cycles through unique tokens).
const INJECTED_LRU_CAP: usize = 1024;

impl LeakDetector {
    #[must_use]
    pub fn new() -> Self {
        Self {
            injected_hashes: lru::LruCache::new(
                std::num::NonZeroUsize::new(INJECTED_LRU_CAP).expect("cap is non-zero"),
            ),
            injected_lens: lru::LruCache::new(
                std::num::NonZeroUsize::new(INJECTED_LRU_CAP).expect("cap is non-zero"),
            ),
            custom_patterns: Vec::new(),
        }
    }

    /// Create a leak detector with custom patterns from config.
    ///
    /// Built-in patterns are always active. Custom patterns are additive.
    /// Invalid regex patterns are logged and skipped.
    pub fn with_custom_patterns(custom: &[crate::config::types::CustomLeakPattern]) -> Self {
        let mut detector = Self::new();
        for pattern in custom {
            match crate::security::safe_regex::bounded_builder(&pattern.pattern).build() {
                Ok(regex) => detector.custom_patterns.push(CompiledCustomPattern {
                    name: pattern.name.clone(),
                    regex,
                }),
                Err(e) => {
                    tracing::warn!(
                        name = %pattern.name,
                        pattern = %pattern.pattern,
                        error = %e,
                        "Skipping invalid custom leak pattern"
                    );
                }
            }
        }
        detector
    }

    /// Register secrets that were injected in the current request.
    ///
    /// Only the `(siphash, byte length)` fingerprint is stored — never the
    /// plaintext, matching the contract of
    /// [`crate::secrets::injection::render_with_secrets`] ("with hashes, never
    /// plaintext"). Secrets shorter than [`MIN_INJECTED_MATCH_LEN`] are not
    /// tracked for substring matching: a 7-byte window over prose produces
    /// false positives, and short values are covered by the pattern rules.
    pub fn register_injected(&mut self, secrets: &[InjectedSecret]) {
        for secret in secrets {
            if secret.value_len < MIN_INJECTED_MATCH_LEN {
                continue;
            }
            // `put` evicts the least-recently-used entry when the cache is at
            // capacity. `contains` below promotes on hit (LRU semantics).
            self.injected_hashes.put(secret.value_hash, ());
            self.injected_lens.put(secret.value_len, ());
        }
    }

    /// Scan content for known secret patterns (built-in + custom).
    ///
    /// Returns `(found_labels, redacted_content)`. Labels are borrowed from
    /// `LEAK_PATTERNS` (static) or `self.custom_patterns` (owned), hence the
    /// mixed lifetime `Vec<&str>`.
    fn scan_patterns<'a>(&'a self, content: &str) -> (Vec<&'a str>, String) {
        let mut redacted = content.to_string();
        let mut found_labels = Vec::new();

        // Check built-in patterns first
        for (label, pattern) in LEAK_PATTERNS.iter() {
            if pattern.is_match(&redacted) {
                found_labels.push(*label);
                redacted = pattern.replace_all(&redacted, REDACTED_LEAK).to_string();
            }
        }

        // Check custom patterns (additive)
        for pattern in &self.custom_patterns {
            if pattern.regex.is_match(&redacted) {
                found_labels.push(&pattern.name);
                redacted = pattern
                    .regex
                    .replace_all(&redacted, REDACTED_LEAK)
                    .to_string();
            }
        }

        (found_labels, redacted)
    }

    /// Scan outbound content for known secret patterns.
    #[must_use]
    pub fn scan_outbound(&self, content: &str) -> LeakDecision {
        let (found_labels, redacted) = self.scan_patterns(content);

        if found_labels.is_empty() {
            LeakDecision::Allow
        } else {
            LeakDecision::Block {
                reason: format!("Outbound leak detected: {}", found_labels.join(", ")),
                redacted_content: redacted,
            }
        }
    }

    /// Scan inbound content for echoed secret values.
    ///
    /// After the pattern rules, every registered secret is looked for as a
    /// **substring** — by hashing each window of the content whose length
    /// matches a registered secret's length and testing the fingerprint set.
    /// This is the same `(hash, len)` identification `crate::sandbox::scrub`
    /// uses, and it is what makes the check independent of the surrounding
    /// text: the previous version hashed whitespace-split words verbatim, so
    /// the single most likely echo — `"Your API key is <SECRET>, stored."` —
    /// hashed `"<SECRET>,"` (trailing comma) and did not match.
    #[must_use]
    pub fn scan_inbound(&self, content: &str) -> LeakDecision {
        let (found_labels, redacted) = self.scan_patterns(content);

        if !found_labels.is_empty() {
            return LeakDecision::Block {
                reason: format!("Inbound leak detected: {}", found_labels.join(", ")),
                redacted_content: redacted,
            };
        }

        if let Some(matched) = self.find_injected_substring(content) {
            return LeakDecision::Block {
                reason: "Inbound response echoed an injected secret value".to_string(),
                redacted_content: content.replace(matched, REDACTED_INJECTED),
            };
        }

        LeakDecision::Allow
    }

    /// The first substring of `content` whose `(siphash, byte length)` matches a
    /// registered secret, or `None`.
    ///
    /// Cost is `O(content.len() * sum(injected_lens))` hashing, and the common
    /// case — no secret was injected — exits on the first check. Windows are
    /// taken over `char_indices` so a multi-byte boundary can never panic; a
    /// window is skipped when the end offset is not a char boundary (a secret is
    /// matched at its own byte length, so its real occurrence always is one).
    fn find_injected_substring<'c>(&self, content: &'c str) -> Option<&'c str> {
        if self.injected_lens.is_empty() {
            return None;
        }
        for (&len, _) in self.injected_lens.iter() {
            if len > content.len() {
                continue;
            }
            for (start, _) in content.char_indices() {
                let end = start + len;
                if end > content.len() || !content.is_char_boundary(end) {
                    continue;
                }
                let window = &content[start..end];
                let mut hasher = siphasher::sip::SipHasher::new_with_keys(
                    INJECTED_HASH_KEY0,
                    INJECTED_HASH_KEY1,
                );
                window.hash(&mut hasher);
                if self.injected_hashes.contains(&hasher.finish()) {
                    return Some(window);
                }
            }
        }
        None
    }
}

impl Default for LeakDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_outbound_blocks_known_api_key() {
        let detector = LeakDetector::new();
        let content = "Use this key: sk-ant-api03-abcdefghijklmnopqrstuvwxyz";
        let decision = detector.scan_outbound(content);
        assert!(decision.is_blocked());
        if let LeakDecision::Block { reason, .. } = decision {
            assert!(reason.contains("Anthropic API Key"));
        }
    }

    #[test]
    fn test_outbound_allows_normal_content() {
        let detector = LeakDetector::new();
        let content = "Please search for 'rust async programming'";
        let decision = detector.scan_outbound(content);
        assert!(!decision.is_blocked());
    }

    #[test]
    fn test_outbound_allows_word_internal_sk_substring() {
        // Regression (real production trigger): a CNBC "elon-musk-..." article
        // URL and a "task-<uuid>" coordination id both contain "sk-" mid-word.
        // The OpenAI-key pattern must be left-anchored (`\b`) so these are NOT
        // flagged as API keys — otherwise the ENTIRE outbound LLM request is
        // blocked. Mirrors the existing fix in pii/rules/api_key.rs
        // (test_no_match_sk_inside_larger_token).
        let detector = LeakDetector::new();
        let content = "See https://www.cnbc.com/2025/12/08/elon-musk-tesla-robotaxi.html \
                       and coordination id task-d537438a-f1a7-46ec-9e6f-4b1fb20d8233";
        assert!(
            !detector.scan_outbound(content).is_blocked(),
            "word-internal 'sk-' (musk-/task-) must not be flagged as an API key"
        );
    }

    #[test]
    fn test_byte_patterns_allow_word_internal_sk_substring() {
        // Same regression for the byte-level sandbox scrubber sources
        // (SECRET_PATTERN_SOURCES → default_patterns_bytes): "musk-"/"task-" in
        // raw command stdout must not be redacted as a secret, while a real key
        // at a boundary still is.
        let pats = default_patterns_bytes();
        let sk_family: Vec<_> = pats
            .iter()
            .filter(|(n, _)| n.starts_with("sk") || *n == "openai_sk")
            .collect();
        assert!(!sk_family.is_empty(), "sk-family byte patterns must exist");
        let benign = b"elon-musk-tesla-robotaxi-update and task-d537438a-f1a7-46ec-9e6f";
        for (name, re) in &sk_family {
            assert!(
                !re.is_match(benign),
                "byte pattern {name} false-matched a word-internal 'sk-' substring"
            );
        }
        let real = b"OPENAI_API_KEY=sk-abcdefghijklmnopqrstuvwxyz1234";
        assert!(
            sk_family.iter().any(|(_, re)| re.is_match(real)),
            "a real sk- key at a boundary must still be detected"
        );
    }

    #[test]
    fn test_inbound_blocks_echoed_injected_value() {
        let mut detector = LeakDetector::new();
        // Deliberately NOT a vendor-recognisable format: this must be blocked by
        // the injected-fingerprint path, not by a pattern rule.
        let secret = "Kf83-quiet-brook-91xz";
        detector.register_injected(&[InjectedSecret::from_value("my_key", secret)]);

        // The comma is the point: the previous whitespace-word hashing hashed
        // "<SECRET>," and let the most likely echo of all through.
        let response = format!("Your API key is {secret}, stored.");
        let decision = detector.scan_inbound(&response);
        assert!(decision.is_blocked(), "echoed injected secret must block");
        if let LeakDecision::Block {
            redacted_content, ..
        } = decision
        {
            assert!(!redacted_content.contains(secret));
            assert!(redacted_content.contains(REDACTED_INJECTED));
        }
    }

    #[test]
    fn test_inbound_blocks_echoed_injected_value_without_word_boundaries() {
        let mut detector = LeakDetector::new();
        let secret = "Kf83-quiet-brook-91xz";
        detector.register_injected(&[InjectedSecret::from_value("my_key", secret)]);

        // Glued to surrounding text with no whitespace at all — a JSON body or a
        // URL query is the realistic shape.
        let response = format!("{{\"token\":\"{secret}\"}}");
        assert!(detector.scan_inbound(&response).is_blocked());
    }

    #[test]
    fn test_inbound_scan_handles_multibyte_content() {
        let mut detector = LeakDetector::new();
        let secret = "Kf83-quiet-brook-91xz";
        detector.register_injected(&[InjectedSecret::from_value("my_key", secret)]);

        // Windows are taken over char boundaries; CJK text must neither panic
        // nor false-positive.
        assert!(!detector.scan_inbound("密钥已保存，请勿外泄。").is_blocked());
        assert!(detector
            .scan_inbound(&format!("密钥是{secret}，已保存"))
            .is_blocked());
    }

    #[test]
    fn test_short_injected_values_not_tracked() {
        let mut detector = LeakDetector::new();
        detector.register_injected(&[InjectedSecret::from_value("k", "short")]);
        // Below MIN_INJECTED_MATCH_LEN: not tracked, so echoing it is allowed by
        // this path (pattern rules still apply).
        assert!(!detector.scan_inbound("the value is short").is_blocked());
    }

    #[test]
    fn test_inbound_allows_safe_response() {
        let mut detector = LeakDetector::new();
        detector.register_injected(&[InjectedSecret::from_value(
            "key",
            "some-long-secret-value-here",
        )]);

        let response = "Request processed successfully. Status: 200 OK.";
        let decision = detector.scan_inbound(response);
        assert!(!decision.is_blocked());
    }

    #[test]
    fn test_inbound_blocks_known_pattern_even_without_injection() {
        let detector = LeakDetector::new();
        let response = "Here's a token: sk-proj-abcdefghijklmnopqrstuvwxyz12345678";
        let decision = detector.scan_inbound(response);
        assert!(decision.is_blocked());
    }

    #[test]
    fn bare_pkcs8_private_key_header_is_matched_as_block_class() {
        // Canonical PKCS#8 headers carry no algorithm word
        // (`-----BEGIN PRIVATE KEY-----`). The previous `[A-Z ]+` form required
        // one, so bare PKCS#8 keys slipped the block-class secret floor entirely
        // — neither redacted nor refused. Pin the bytes-side catalog: the header
        // must match under the `private_key` name, which is block-class.
        let matched = default_patterns_bytes()
            .iter()
            .find(|(_, re)| re.is_match(b"-----BEGIN PRIVATE KEY-----"))
            .map(|(name, _)| *name);
        assert_eq!(
            matched,
            Some("private_key"),
            "bare PKCS#8 header must match"
        );
        assert!(is_block_class_secret("private_key"));
        // Algorithm-tagged variants must still match — no regression.
        assert!(default_patterns_bytes()
            .iter()
            .any(|(name, re)| *name == "private_key"
                && re.is_match(b"-----BEGIN RSA PRIVATE KEY-----")));
    }

    #[test]
    fn test_registering_stores_fingerprint_only() {
        let mut detector = LeakDetector::new();
        let secret = "abcdefghij-fingerprint-only";
        detector.register_injected(&[InjectedSecret::from_value("k", secret)]);
        assert!(!detector.injected_hashes.is_empty());
        assert_eq!(
            detector
                .injected_lens
                .iter()
                .map(|(k, _)| *k)
                .collect::<Vec<_>>(),
            vec![secret.len()],
            "only the length is kept alongside the hash"
        );
    }

    #[test]
    fn test_redacted_content_in_block_decision() {
        let detector = LeakDetector::new();
        let content = "Key: sk-abcdefghijklmnopqrstuvwxyz123456789012345678";
        if let LeakDecision::Block {
            redacted_content, ..
        } = detector.scan_outbound(content)
        {
            assert!(redacted_content.contains(REDACTED_LEAK));
            assert!(!redacted_content.contains("abcdefgh"));
        } else {
            panic!("Expected Block");
        }
    }

    #[test]
    fn test_custom_pattern_blocks_outbound() {
        use crate::config::types::CustomLeakPattern;

        let custom = vec![CustomLeakPattern {
            name: "Internal Token".to_string(),
            pattern: r"internal-[a-z0-9]{8}".to_string(),
        }];
        let detector = LeakDetector::with_custom_patterns(&custom);

        let decision = detector.scan_outbound("Token: internal-abc12345");
        assert!(decision.is_blocked());
        if let LeakDecision::Block { reason, .. } = decision {
            assert!(reason.contains("Internal Token"));
        }
    }

    #[test]
    fn test_custom_pattern_blocks_inbound() {
        use crate::config::types::CustomLeakPattern;

        let custom = vec![CustomLeakPattern {
            name: "Service Key".to_string(),
            pattern: r"svc-[A-Z]{6}".to_string(),
        }];
        let detector = LeakDetector::with_custom_patterns(&custom);

        let decision = detector.scan_inbound("Key: svc-ABCDEF");
        assert!(decision.is_blocked());
    }

    #[test]
    fn test_outbound_blocks_vendor_slack_token() {
        let detector = LeakDetector::new();
        let decision = detector.scan_outbound("SLACK_TOKEN=xoxb-1234567890-abcdefghijklmnop");
        assert!(
            decision.is_blocked(),
            "slack bot token should block on egress"
        );
        if let LeakDecision::Block { reason, .. } = decision {
            assert!(reason.contains("Slack Token"));
        }
    }

    #[test]
    fn test_outbound_blocks_vendor_huggingface_token() {
        let detector = LeakDetector::new();
        let decision = detector.scan_outbound("export HF=hf_abcdefghijklmnopqrstuvwxyz0123456789");
        assert!(
            decision.is_blocked(),
            "hugging face token should block on egress"
        );
    }

    #[test]
    fn test_outbound_blocks_vendor_stripe_key() {
        let detector = LeakDetector::new();
        let decision = detector.scan_outbound("key: sk_live_abcdefghijklmnopqrstuvwx1234");
        assert!(
            decision.is_blocked(),
            "stripe secret key should block on egress"
        );
    }

    #[test]
    fn test_vendor_patterns_redact_in_byte_scrubber() {
        // The shared vendor catalog must also feed the byte-level scrubber so
        // sandbox stdout is scrubbed of the same secrets the egress guard blocks.
        let patterns = default_patterns_bytes();
        let has_groq = patterns.iter().any(|(name, _)| *name == "Groq API Key");
        assert!(
            has_groq,
            "vendor catalog should be wired into byte scrubber"
        );
        let groq = patterns
            .iter()
            .find(|(name, _)| *name == "Groq API Key")
            .map(|(_, re)| re)
            .unwrap();
        assert!(groq.is_match(b"gsk_abcdefghijklmnopqrstuvwxyz0123456789ABCD"));
    }

    #[test]
    fn test_outbound_allows_non_vendor_prefixed_text() {
        // Distinctive prefixes must not over-block ordinary prose / identifiers.
        let detector = LeakDetector::new();
        let decision =
            detector.scan_outbound("The sky was clear and the report listed 42 findings.");
        assert!(!decision.is_blocked());
    }

    #[test]
    fn test_custom_pattern_invalid_regex_skipped() {
        use crate::config::types::CustomLeakPattern;

        let custom = vec![
            CustomLeakPattern {
                name: "Invalid".to_string(),
                pattern: "[invalid".to_string(),
            },
            CustomLeakPattern {
                name: "Valid".to_string(),
                pattern: r"test-\d{4}".to_string(),
            },
        ];
        let detector = LeakDetector::with_custom_patterns(&custom);

        // Invalid pattern is skipped, but valid one works
        let decision = detector.scan_outbound("Code: test-1234");
        assert!(decision.is_blocked());
        if let LeakDecision::Block { reason, .. } = decision {
            assert!(reason.contains("Valid"));
        }
    }
}
