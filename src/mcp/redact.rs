//! Credential/PII redaction for MCP error surfaces.
//!
//! MCP tool failures and transport errors can echo back arguments, URLs, or
//! headers that contain secrets the agent supplied. Before such a string is
//! handed to the LLM — which may persist it in conversation history or a
//! provider's request logs — it is run through the global [`PiiEngine`], the
//! same secret/PII rule set the rest of Aleph uses. MCP therefore needs no
//! bespoke regex of its own (R3 core minimalism).

use crate::pii::PiiEngine;

/// Conservative redaction applied when the global `PiiEngine` is missing.
///
/// This is the fail-closed fallback for `redact_mcp_error`: if the engine
/// has not been installed at boot (e.g. a CLI one-shot that skipped
/// `PiiEngine::init`), the alternative is to hand an unredacted error
/// containing OAuth tokens, API keys, or `Authorization: Bearer …`
/// headers straight to the LLM — a confidentiality bug. We strip the
/// common credential-shaped patterns here so a missing engine cannot
/// silently degrade to identity passthrough.
const CONSERVATIVE_PATTERNS: &[(&str, &str)] = &[
    // Authorization headers and basic-auth in URLs and JSON.
    (
        "authorization: bearer ",
        "authorization: bearer [REDACTED] ",
    ),
    ("authorization: basic ", "authorization: basic [REDACTED] "),
    // Common credential query-string params. Match on `key=value` so we
    // catch `?api_key=…` and `?token=…` but not arbitrary prose that
    // happens to contain the substring "api_key".
    ("api_key=", "api_key=[REDACTED]"),
    ("apikey=", "apikey=[REDACTED]"),
    ("access_token=", "access_token=[REDACTED]"),
    ("refresh_token=", "refresh_token=[REDACTED]"),
    ("client_secret=", "client_secret=[REDACTED]"),
    ("password=", "password=[REDACTED]"),
];

fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    let needle_bytes = needle.as_bytes();
    let haystack_bytes = haystack.as_bytes();
    if needle_bytes.len() > haystack_bytes.len() {
        return None;
    }
    haystack_bytes
        .windows(needle_bytes.len())
        .position(|window| window.eq_ignore_ascii_case(needle_bytes))
}

fn conservative_redact(text: &str) -> String {
    let mut current = text.to_string();
    for (needle, replacement) in CONSERVATIVE_PATTERNS {
        let mut out = String::with_capacity(current.len());
        let mut remaining = current.as_str();
        while let Some(idx) = find_ascii_case_insensitive(remaining, needle) {
            let prefix = remaining.get(..idx).unwrap_or("");
            out.push_str(prefix);
            out.push_str(replacement);
            let after_needle = remaining.get(idx + needle.len()..).unwrap_or("");
            let val_end = if after_needle.starts_with('"') {
                after_needle
                    .get(1..)
                    .and_then(|tail| tail.find('"').map(|end| end + 2))
                    .unwrap_or(after_needle.len())
            } else if after_needle.starts_with('\'') {
                after_needle
                    .get(1..)
                    .and_then(|tail| tail.find('\'').map(|end| end + 2))
                    .unwrap_or(after_needle.len())
            } else {
                after_needle
                    .find(|c: char| c.is_whitespace() || matches!(c, '&' | ',' | '"' | '\''))
                    .unwrap_or(after_needle.len())
            };
            remaining = after_needle.get(val_end..).unwrap_or("");
        }
        out.push_str(remaining);
        current = out;
    }
    current
}

/// Redact secrets and PII from an MCP-originated error string.
///
/// When the global `PiiEngine` is initialised, this delegates to its full
/// rule set (the same one used elsewhere in Aleph). When the engine is
/// absent — typically unit tests, but possibly a boot path that skipped
/// `PiiEngine::init` — we fall back to a conservative pattern-based
/// redaction rather than identity passthrough, so a missing engine cannot
/// silently leak OAuth tokens or API keys into LLM history. The lock is
/// taken poison-safe, matching the idiom used elsewhere in `pii`.
#[must_use]
pub fn redact_mcp_error(text: &str) -> String {
    match PiiEngine::global() {
        Some(engine) => {
            let guard = engine.read().unwrap_or_else(|e| e.into_inner());
            guard.filter(text).text
        }
        None => conservative_redact(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_when_engine_uninitialised_and_no_secrets() {
        // In an isolated unit test the global engine is typically absent;
        // a benign message must come through unchanged (the conservative
        // fallback is no-op on inputs it does not recognise).
        let input = "plain error text with no secrets";
        assert_eq!(redact_mcp_error(input), input);
    }

    #[test]
    fn conservative_redact_strips_bearer_token() {
        let input = "upstream rejected: authorization: bearer abc123.def456";
        let out = conservative_redact(input);
        assert!(!out.contains("abc123.def456"), "got: {out}");
        assert!(out.contains("[REDACTED]"));
    }

    #[test]
    fn conservative_redact_strips_api_key_query_param() {
        let input = "GET /v1/things?api_key=sk_live_zzzz&page=2 failed";
        let out = conservative_redact(input);
        assert!(!out.contains("sk_live_zzzz"), "got: {out}");
        assert!(out.contains("api_key=[REDACTED]"));
        // The non-secret query param must survive.
        assert!(out.contains("page=2"));
    }

    #[test]
    fn conservative_redact_handles_mixed_case() {
        let input = "Authorization: Bearer xyz";
        let out = conservative_redact(input);
        assert!(!out.contains("xyz"), "got: {out}");
    }

    #[test]
    fn conservative_redact_is_case_insensitive_on_substring() {
        // Substring "API_KEY=..." should be scrubbed case-insensitively —
        // the conservative fallback is case-insensitive on the needle
        // itself (so `api_key=`, `API_KEY=`, etc. all match). The pattern
        // intentionally requires the trailing `=` so prose that happens
        // to contain "API_KEY" without a value is NOT a credential leak
        // and stays untouched — see the comment next to the
        // `api_key=` entry in `CONSERVATIVE_PATTERNS`.
        //
        // What the implementation promises: the needle itself is replaced
        // (preserving the original casing for traceability, so `API_KEY`
        // comes back as `api_key=` in the redacted form), AND the value
        // immediately after the needle is consumed up to the next
        // whitespace / delimiter so the credential itself does not
        // survive. Surrounding prose (whitespace, then more words)
        // passes through untouched — that is by design, not a leak.
        let input = "the API_KEY=zzz is leaked";
        let out = conservative_redact(input);
        assert!(
            !out.contains("zzz"),
            "the credential value must be redacted, got: {out}"
        );
        assert!(
            out.contains("[REDACTED]"),
            "the needle must be replaced, got: {out}"
        );
        // Round-trip casing: the original was `API_KEY` (uppercase);
        // the redacted form is `api_key=` (lowercase, the needle's
        // canonical casing). The case-insensitive substring match for
        // either spelling is the property under test.
        assert!(
            out.contains("api_key=") || out.contains("API_KEY="),
            "either casing of the replaced needle must remain, got: {out}"
        );
    }
}
