//! Byte-level secret scrub for sandbox stdout/stderr.
//!
//! Runs `regex::bytes::Regex` patterns over raw `&[u8]` before any UTF-8
//! conversion, catching secrets surrounded by non-UTF-8 bytes that would
//! otherwise be lossily replaced with `U+FFFD`.

use std::borrow::Cow;

use crate::secrets::injection::InjectedSecret;
use crate::secrets::leak_detector::{default_patterns_bytes, is_block_class_secret};

/// Outcome of a byte-level scrub.
#[derive(Debug, Clone)]
pub struct ScrubResult<'a> {
    /// Possibly modified bytes (borrowed when no hits, owned when redacted).
    pub bytes: Cow<'a, [u8]>,
    /// Pattern names that matched and were redacted.
    pub hits: Vec<&'static str>,
    /// Distinct block-class pattern names that matched (subset of `hits`).
    ///
    /// Non-empty when a catastrophic secret (see
    /// [`crate::secrets::leak_detector::BLOCK_CLASS_SECRETS`]) appeared in the
    /// scanned bytes. The bytes are still redacted as usual; this is an
    /// additional fail-closed signal for callers — the shell-output analogue of
    /// clawshell's `DlpAction::Block` versus the default `Redact`.
    pub blocked: Vec<&'static str>,
}

/// Scan `bytes` for secret patterns; replace matches with `[REDACTED:NAME]`.
/// Matches whose contents hash-match an entry in `injected` are skipped
/// (they were intentionally injected by the placeholder pipeline).
pub fn scrub_secrets_bytes<'a>(bytes: &'a [u8], injected: &[InjectedSecret]) -> ScrubResult<'a> {
    let patterns = default_patterns_bytes();
    let mut hits: Vec<&'static str> = Vec::new();
    let mut buf: Option<Vec<u8>> = None;

    for (name, re) in &patterns {
        let working_slice = buf.as_deref().unwrap_or(bytes);
        if !re.is_match(working_slice) {
            continue;
        }
        let working = buf.get_or_insert_with(|| bytes.to_vec());
        let local_matches: Vec<(usize, usize)> = re
            .find_iter(working)
            .map(|m| (m.start(), m.end()))
            .collect();
        for (start, end) in local_matches.into_iter().rev() {
            if is_whitelisted(&working[start..end], injected) {
                continue;
            }
            let replacement = format!("[REDACTED:{}]", name).into_bytes();
            working.splice(start..end, replacement);
            hits.push(*name);
        }
    }

    // Distinct block-class hits (catastrophic secrets that callers should fail
    // closed on, not merely redact). Derived from `hits` so the redaction logic
    // above stays the single source of truth for what matched.
    let mut blocked: Vec<&'static str> = hits
        .iter()
        .copied()
        .filter(|name| is_block_class_secret(name))
        .collect();
    blocked.sort_unstable();
    blocked.dedup();

    match buf {
        Some(v) => ScrubResult {
            bytes: Cow::Owned(v),
            hits,
            blocked,
        },
        None => ScrubResult {
            bytes: Cow::Borrowed(bytes),
            hits,
            blocked,
        },
    }
}

fn is_whitelisted(slice: &[u8], injected: &[InjectedSecret]) -> bool {
    if injected.is_empty() {
        return false;
    }
    let Ok(s) = std::str::from_utf8(slice) else {
        return false;
    };
    let candidate = InjectedSecret::from_value("__probe__", s);
    injected
        .iter()
        .any(|i| i.value_hash == candidate.value_hash && i.value_len == candidate.value_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_passthrough_when_no_match() {
        let input = b"hello world\n".to_vec();
        let out = scrub_secrets_bytes(&input, &[]);
        assert_eq!(out.bytes.as_ref(), input.as_slice());
        assert!(out.hits.is_empty());
        assert!(matches!(out.bytes, Cow::Borrowed(_)));
    }

    #[test]
    fn scrub_redacts_sk_proj_in_utf8() {
        let mut input = b"key=sk-proj-".to_vec();
        input.extend(std::iter::repeat_n(b'A', 40));
        let out = scrub_secrets_bytes(&input, &[]);
        let s = String::from_utf8_lossy(out.bytes.as_ref());
        assert!(s.contains("[REDACTED:sk_proj]"), "got `{s}`");
        assert!(out.hits.contains(&"sk_proj"));
    }

    #[test]
    fn scrub_finds_sk_around_nonutf8_bytes() {
        let mut input = b"prefix:".to_vec();
        input.extend_from_slice(b"sk-proj-");
        input.extend(std::iter::repeat_n(b'B', 40));
        input.push(0xFF);
        input.extend_from_slice(b":suffix");
        let out = scrub_secrets_bytes(&input, &[]);
        let s = String::from_utf8_lossy(out.bytes.as_ref());
        assert!(s.contains("[REDACTED:sk_proj]"), "got `{s}`");
    }

    #[test]
    fn scrub_redacts_generic_openai_key() {
        // Classic `sk-…` key (not the sk-proj/sk-ant prefixed forms) printed to
        // stdout must be redacted, not returned to the model verbatim.
        let mut input = b"OPENAI_API_KEY=sk-".to_vec();
        input.extend(std::iter::repeat_n(b'a', 40));
        let out = scrub_secrets_bytes(&input, &[]);
        let s = String::from_utf8_lossy(out.bytes.as_ref());
        assert!(s.contains("[REDACTED:openai_sk]"), "got `{s}`");
    }

    #[test]
    fn scrub_redacts_google_api_key() {
        let mut input = b"key: AIza".to_vec();
        input.extend(std::iter::repeat_n(b'X', 35));
        let out = scrub_secrets_bytes(&input, &[]);
        let s = String::from_utf8_lossy(out.bytes.as_ref());
        assert!(s.contains("[REDACTED:google_api]"), "got `{s}`");
    }

    #[test]
    fn scrub_redacts_pem_private_key_header() {
        let input = b"-----BEGIN RSA PRIVATE KEY-----\nMIIE...\n".to_vec();
        let out = scrub_secrets_bytes(&input, &[]);
        let s = String::from_utf8_lossy(out.bytes.as_ref());
        assert!(s.contains("[REDACTED:private_key]"), "got `{s}`");
    }

    #[test]
    fn scrub_skips_whitelisted_injected_secret() {
        let key_str: String = format!("sk-proj-{}", "C".repeat(40));
        let injected = InjectedSecret::from_value("test", &key_str);
        let input = format!("key={key_str}").into_bytes();
        let out = scrub_secrets_bytes(&input, &[injected]);
        let s = String::from_utf8_lossy(out.bytes.as_ref());
        assert!(
            s.contains(&key_str),
            "expected injected key passthrough, got `{s}`"
        );
        assert!(out.hits.is_empty());
    }

    #[test]
    fn scrub_flags_private_key_as_block_class() {
        // A PEM private-key block in command output is catastrophic: redacted
        // AND flagged for fail-closed handling by the caller.
        let input = b"-----BEGIN RSA PRIVATE KEY-----\nMIIE...\n".to_vec();
        let out = scrub_secrets_bytes(&input, &[]);
        let s = String::from_utf8_lossy(out.bytes.as_ref());
        assert!(s.contains("[REDACTED:private_key]"), "got `{s}`");
        assert_eq!(out.blocked, vec!["private_key"]);
    }

    #[test]
    fn scrub_does_not_block_redact_class_api_key() {
        // API-token shapes are redacted but NOT block-class: they can appear in
        // legitimate `env`/config inspection and must not fail the command.
        let mut input = b"OPENAI_API_KEY=sk-".to_vec();
        input.extend(std::iter::repeat_n(b'a', 40));
        let out = scrub_secrets_bytes(&input, &[]);
        assert!(!out.hits.is_empty());
        assert!(out.blocked.is_empty(), "api key must not be block-class");
    }

    #[test]
    fn scrub_clean_output_has_no_block_signal() {
        let out = scrub_secrets_bytes(b"all good here\n", &[]);
        assert!(out.hits.is_empty());
        assert!(out.blocked.is_empty());
    }

    #[test]
    fn scrub_block_class_deduped_across_occurrences() {
        let mut input = b"-----BEGIN RSA PRIVATE KEY-----\n".to_vec();
        input.extend_from_slice(b"-----BEGIN EC PRIVATE KEY-----\n");
        let out = scrub_secrets_bytes(&input, &[]);
        // Two PEM headers matched, but the block signal is deduped by name.
        assert_eq!(out.blocked, vec!["private_key"]);
    }
}
