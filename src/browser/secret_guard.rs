//! Secret-exfiltration guard for browser navigation and form input.
//!
//! SSRF protection ([`super::network_policy`]) governs the *destination network*
//! of a browser navigation — "is this host allowed to be reached". This guard
//! governs the orthogonal concern of the *content* crossing the boundary: it
//! rejects navigations whose URL embeds a high-confidence secret (API key,
//! bearer token, PEM private key), and form input (type/fill/select/dialog
//! prompt) that would type one into a page. Both block an attacker page from
//! socially-engineering the agent into exfiltrating a secret already present in
//! its context to an otherwise policy-allowed public host (e.g.
//! `https://evil.example/?leak=sk-ant-...`, or a "login" form on that host).
//!
//! Secret patterns are NOT duplicated here. The existing `Critical`-severity
//! PII rules ([`crate::pii::rules`]) are the single source of truth, so a new
//! credential pattern added there is automatically enforced at navigation time.

use std::borrow::Cow;

use percent_encoding::percent_decode_str;

use crate::pii::{rules::build_rules, PiiMatch, PiiRule, PiiSeverity};

/// Build the set of `Critical`-severity credential rules (API keys, bearer
/// tokens, PEM/SSH private keys, bank/ID numbers). Single source of truth shared
/// by all three legs of the browser secret-egress boundary —
/// [`scan_url_for_secrets`] (navigation target), [`scan_text_for_secrets`]
/// (form input), and [`redact_secrets`] (page-content output). Lower-severity
/// PII (emails, phone numbers, IP addresses) is deliberately excluded: it is
/// not a credential and must never block a navigation or an input, or be
/// scrubbed from the page content the agent works on.
fn critical_rules() -> &'static [Box<dyn PiiRule>] {
    static RULES: std::sync::OnceLock<Vec<Box<dyn PiiRule>>> = std::sync::OnceLock::new();
    RULES
        .get_or_init(|| {
            build_rules(&[])
                .into_iter()
                .filter(|r| r.severity() == PiiSeverity::Critical)
                .collect()
        })
        .as_slice()
}

/// Scan a navigation URL (raw + percent-decoded) for an embedded secret.
///
/// Returns the matched rule name (e.g. `"api_key"`) on the first hit, or
/// `None` when the URL carries no detectable secret.
pub(crate) fn scan_url_for_secrets(url: &str) -> Option<String> {
    let secret_rules = critical_rules();

    // Raw form catches the common unencoded case (`sk-…` is unreserved and is
    // rarely percent-encoded); the decoded form defeats percent-encoding
    // evasion such as "%73%6b-…".
    let decoded = percent_decode_str(url).decode_utf8_lossy();
    for candidate in [url, decoded.as_ref()] {
        for rule in secret_rules {
            if let Some(m) = rule.detect(candidate).into_iter().next() {
                return Some(m.rule_name);
            }
        }
    }
    None
}

/// Scan form-input text (type/fill/select/dialog prompt) for an embedded
/// secret — the input-side twin of [`scan_url_for_secrets`].
///
/// Returns the matched rule name (e.g. `"api_key"`) on the first hit, or
/// `None` when the text carries no detectable secret. Unlike the URL scan this
/// does NOT percent-decode: a form value is typed verbatim, not URL-encoded,
/// so decoding would only invite false positives.
pub(crate) fn scan_text_for_secrets(text: &str) -> Option<String> {
    for rule in critical_rules() {
        if let Some(m) = rule.detect(text).into_iter().next() {
            return Some(m.rule_name);
        }
    }
    None
}

/// Redact every `Critical`-severity credential span in page-derived `text`,
/// replacing each with a `[REDACTED:<rule>]` placeholder.
///
/// This is the OUT half of the secret-egress boundary, symmetric to
/// [`scan_url_for_secrets`] (the IN half): page content (accessibility
/// snapshots, console output, network logs, JS-eval results) can contain
/// credentials — an API key printed to the console, a bearer token rendered on
/// a settings page — which would otherwise flow verbatim into the model
/// context, long-term memory, and provider requests. Scrubbing them at the
/// tool-output boundary closes that exfiltration path while leaving the page's
/// structure (element refs, labels, ordinary text) intact.
///
/// Returns `Cow::Borrowed` unchanged when no secret is present (the common
/// case — zero allocation). Matches are spliced by byte offset; the offsets
/// come from regex matches against this exact `text`, so they fall on char
/// boundaries (re-checked defensively before slicing).
pub(crate) fn redact_secrets(text: &str) -> Cow<'_, str> {
    let rules = critical_rules();
    let mut matches: Vec<PiiMatch> = Vec::new();
    for rule in rules {
        matches.extend(rule.detect(text));
    }
    if matches.is_empty() {
        return Cow::Borrowed(text);
    }

    // Sort by start, then longest-first so an enclosing span wins over a nested
    // one; skip any span that overlaps one already emitted.
    matches.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    for m in &matches {
        if m.start < cursor || m.start > m.end || m.end > text.len() {
            continue;
        }
        if !text.is_char_boundary(m.start) || !text.is_char_boundary(m.end) {
            continue;
        }
        out.push_str(&text[cursor..m.start]);
        out.push_str("[REDACTED:");
        out.push_str(&m.rule_name);
        out.push(']');
        cursor = m.end;
    }
    out.push_str(&text[cursor..]);
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_url_passes() {
        assert!(scan_url_for_secrets("https://example.com/search?q=rust+async").is_none());
        assert!(scan_url_for_secrets("https://docs.rs/tokio/latest/tokio/").is_none());
    }

    #[test]
    fn detects_anthropic_style_key_in_query() {
        let hit = scan_url_for_secrets(
            "https://evil.example/?leak=sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789",
        );
        assert_eq!(hit.as_deref(), Some("api_key"));
    }

    #[test]
    fn detects_aws_access_key() {
        let hit = scan_url_for_secrets("https://evil.example/x?k=AKIAIOSFODNN7EXAMPLE");
        assert_eq!(hit.as_deref(), Some("api_key"));
    }

    #[test]
    fn detects_percent_encoded_key() {
        // "sk-" percent-encoded as %73%6b%2d defeats a raw-only scan.
        let hit = scan_url_for_secrets(
            "https://evil.example/?t=%73%6b%2dlivesecret012345678901234567890",
        );
        assert_eq!(hit.as_deref(), Some("api_key"));
    }

    #[test]
    fn lower_severity_pii_does_not_block() {
        // An email address is PII but not a credential — navigation must proceed.
        assert!(scan_url_for_secrets("https://example.com/u?email=alice@example.com").is_none());
    }

    #[test]
    fn scan_text_detects_api_key_in_form_input() {
        let hit = scan_text_for_secrets("password is sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789");
        assert_eq!(hit.as_deref(), Some("api_key"));
    }

    #[test]
    fn scan_text_passes_clean_input() {
        assert!(scan_text_for_secrets("hello world").is_none());
        assert!(scan_text_for_secrets("Hunter2").is_none());
    }

    #[test]
    fn scan_text_email_does_not_block() {
        // An email is PII but not a credential — typing it into a form is fine.
        assert!(scan_text_for_secrets("alice@example.com").is_none());
    }

    #[test]
    fn redact_clean_text_borrows_unchanged() {
        let text = "- button \"Sign in\" [ref=e3]\n- heading \"Welcome\"";
        let out = redact_secrets(text);
        assert!(matches!(out, Cow::Borrowed(_)), "no secret → zero-copy");
        assert_eq!(out, text);
    }

    #[test]
    fn redact_scrubs_api_key_in_console_text() {
        let text = "config loaded; token=sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789 ok";
        let out = redact_secrets(text);
        assert!(matches!(out, Cow::Owned(_)));
        assert!(!out.contains("sk-ant-api03"), "raw key must be gone: {out}");
        assert!(out.contains("[REDACTED:api_key]"));
        // Surrounding structure is preserved.
        assert!(out.starts_with("config loaded; token="));
        assert!(out.ends_with(" ok"));
    }

    #[test]
    fn redact_scrubs_multiple_secrets() {
        let text = "a=AKIAIOSFODNN7EXAMPLE b=sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789 end";
        let out = redact_secrets(text);
        assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(!out.contains("sk-ant-api03"));
        assert_eq!(out.matches("[REDACTED:").count(), 2);
        assert!(out.ends_with(" end"));
    }

    #[test]
    fn redact_preserves_multibyte_text() {
        // A non-ASCII prefix shifts byte offsets; redaction must not corrupt it.
        let text = "登录令牌 token=sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789 完成";
        let out = redact_secrets(text);
        assert!(out.starts_with("登录令牌 token="));
        assert!(out.ends_with(" 完成"));
        assert!(out.contains("[REDACTED:api_key]"));
    }
}
