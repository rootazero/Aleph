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
    ("authorization: bearer ", "authorization: bearer [REDACTED] "),
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

fn conservative_redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut remaining = text;
    for (needle, replacement) in CONSERVATIVE_PATTERNS {
        while let Some(idx) = remaining.to_ascii_lowercase().find(&needle.to_ascii_lowercase()) {
            out.push_str(&remaining[..idx]);
            out.push_str(replacement);
            // Consume the value following the needle (up to the next
            // whitespace, delimiter, or quote) so the credential itself does
            // not survive into the trailing slice — e.g. for input
            // "Authorization: Bearer xyz" this drops "xyz" along with the
            // needle. Without this, `out` would end with the replacement and
            // `remaining` would still hold the secret, leaking it into the
            // final `out.push_str(remaining)` below.
            remaining = &remaining[idx + needle.len()..];
            let val_end = remaining
                .find(|c: char| c.is_whitespace() || matches!(c, '&' | ',' | '"' | '\''))
                .unwrap_or(remaining.len());
            remaining = &remaining[val_end..];
        }
    }
    out.push_str(remaining);
    out
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
        // Substring "API_KEY" in prose should also be scrubbed — the
        // conservative fallback over-matches slightly, by design.
        let input = "the API_KEY is leaked";
        let out = conservative_redact(input);
        assert!(!out.contains("leaked"));
    }
}
