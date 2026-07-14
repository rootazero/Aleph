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
#[must_use]
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
            let replacement = format!("[REDACTED:{name}]").into_bytes();
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

/// Invisible / bidirectional Unicode control characters that have no place in
/// command output and are classic prompt- and terminal-spoofing vectors:
/// zero-width injection (text the human never sees but the model does) and
/// right-to-left / isolate overrides (visually reordering text to disguise its
/// real content). Each entry is the exact UTF-8 encoding of one code point.
///
/// This is the redline-safe half of prompt-injection defense — a deterministic
/// hard filter on a fixed set of control characters, like stripping ANSI
/// escapes. It maps `OpenSquilla`'s `injection_guard` "invisible character" class
/// onto Aleph WITHOUT porting its semantic regex classifier (prompt-override /
/// role-hijack / exfiltration heuristics), which would be content scoring and
/// violate the no-content-review redline (R7 / R10).
const UNSAFE_INVISIBLE_SEQUENCES: &[&[u8]] = &[
    &[0xE2, 0x80, 0x8B], // U+200B ZERO WIDTH SPACE
    &[0xE2, 0x80, 0x8C], // U+200C ZERO WIDTH NON-JOINER
    &[0xE2, 0x80, 0x8D], // U+200D ZERO WIDTH JOINER
    &[0xE2, 0x80, 0xAA], // U+202A LEFT-TO-RIGHT EMBEDDING
    &[0xE2, 0x80, 0xAB], // U+202B RIGHT-TO-LEFT EMBEDDING
    &[0xE2, 0x80, 0xAC], // U+202C POP DIRECTIONAL FORMATTING
    &[0xE2, 0x80, 0xAD], // U+202D LEFT-TO-RIGHT OVERRIDE
    &[0xE2, 0x80, 0xAE], // U+202E RIGHT-TO-LEFT OVERRIDE
    &[0xE2, 0x81, 0xA6], // U+2066 LEFT-TO-RIGHT ISOLATE
    &[0xE2, 0x81, 0xA7], // U+2067 RIGHT-TO-LEFT ISOLATE
    &[0xE2, 0x81, 0xA8], // U+2068 FIRST STRONG ISOLATE
    &[0xE2, 0x81, 0xA9], // U+2069 POP DIRECTIONAL ISOLATE
    &[0xEF, 0xBB, 0xBF], // U+FEFF ZERO WIDTH NO-BREAK SPACE (BOM)
];

/// Strip every [`UNSAFE_INVISIBLE_SEQUENCES`] occurrence from `bytes`,
/// returning the cleaned bytes (borrowed when none present) and the number of
/// sequences removed.
///
/// Operating on raw bytes is safe even amid non-UTF-8 data: UTF-8 is
/// self-synchronizing, so each three-byte target sequence can only appear as
/// the complete encoding of its code point — never spanning a character
/// boundary — because its lead byte (`0xE2` / `0xEF`) always starts a fresh
/// character and its trailing bytes are continuation bytes that never start one.
#[must_use]
pub fn strip_unsafe_invisible(bytes: &[u8]) -> (Cow<'_, [u8]>, usize) {
    // Every target shares one of two lead bytes — cheap reject for clean output.
    if !bytes.iter().any(|&b| b == 0xE2 || b == 0xEF) {
        return (Cow::Borrowed(bytes), 0);
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut removed = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        let rest = &bytes[i..];
        if let Some(seq) = UNSAFE_INVISIBLE_SEQUENCES
            .iter()
            .find(|seq| rest.starts_with(seq))
        {
            removed += 1;
            i += seq.len();
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    if removed == 0 {
        // Lead byte present but no full target matched: nothing changed, so
        // hand back the borrow and drop the scratch buffer.
        return (Cow::Borrowed(bytes), 0);
    }
    (Cow::Owned(out), removed)
}

/// Apply the full command-output content floor to a finished command's
/// stdout/stderr, in place: redact secret material, neutralize invisible/bidi
/// control characters, and return the deduped set of **block-class** secret
/// names detected (empty = safe). A non-empty return means catastrophic secret
/// material was present and the caller MUST fail closed — redaction alone still
/// returns the surrounding context to the model.
///
/// This is the single source of truth for the output floor, shared by every
/// [`Sandbox`](crate::sandbox::Sandbox) implementation (`WorkspaceSandbox`,
/// `WorktreeSandbox`). Duplicating it per execution path is exactly how a floor
/// silently diverges — the same defect class as a bare PKCS#8 key slipping one
/// of two secret catalogs, or a worktree subagent running with no floor at all.
///
/// Direct sandbox callers carry no `SecurityContext`, so the injected-secret
/// whitelist is empty — the safe default (scrub everything).
#[must_use]
pub fn scrub_and_gate_output(out: &mut crate::sandbox::SandboxOutput) -> Vec<&'static str> {
    let injected: &[InjectedSecret] = &[];
    let stdout_scrub = scrub_secrets_bytes(&out.stdout, injected);
    let stderr_scrub = scrub_secrets_bytes(&out.stderr, injected);
    if !stdout_scrub.hits.is_empty() || !stderr_scrub.hits.is_empty() {
        tracing::warn!(
            stdout_hits = ?stdout_scrub.hits,
            stderr_hits = ?stderr_scrub.hits,
            "sandbox bytes-scrub redacted secrets in command output"
        );
    }
    let mut blocked: Vec<&'static str> = Vec::new();
    blocked.extend(stdout_scrub.blocked);
    blocked.extend(stderr_scrub.blocked);
    out.stdout = stdout_scrub.bytes.into_owned();
    out.stderr = stderr_scrub.bytes.into_owned();

    // Neutralize invisible / bidirectional Unicode control characters
    // (zero-width injection, RLO/isolate overrides) before the output reaches
    // the model — the redline-safe, deterministic half of prompt-injection
    // defense, distinct from the secret scrub above.
    let (stdout_clean, n_out) = strip_unsafe_invisible(&out.stdout);
    let (stderr_clean, n_err) = strip_unsafe_invisible(&out.stderr);
    if n_out + n_err > 0 {
        tracing::warn!(
            removed = n_out + n_err,
            "sandbox neutralized invisible/bidi control chars in command output"
        );
    }
    out.stdout = stdout_clean.into_owned();
    out.stderr = stderr_clean.into_owned();

    blocked.sort_unstable();
    blocked.dedup();
    blocked
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
    fn invisible_strip_passthrough_when_clean() {
        let (out, n) = strip_unsafe_invisible(b"normal output\n");
        assert_eq!(n, 0);
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    #[test]
    fn invisible_strip_removes_zero_width_and_bidi() {
        // "ad\u{202E}min" (RLO) + a zero-width space + a BOM.
        let mut input = "ad".as_bytes().to_vec();
        input.extend_from_slice(&[0xE2, 0x80, 0xAE]); // U+202E RLO
        input.extend_from_slice("min".as_bytes());
        input.extend_from_slice(&[0xE2, 0x80, 0x8B]); // U+200B ZWSP
        input.extend_from_slice(&[0xEF, 0xBB, 0xBF]); // U+FEFF BOM
        let (out, n) = strip_unsafe_invisible(&input);
        assert_eq!(n, 3, "three control sequences removed");
        assert_eq!(
            out.as_ref(),
            b"admin",
            "visible text preserved, controls gone"
        );
    }

    #[test]
    fn invisible_strip_preserves_legit_e2_led_chars() {
        // U+20AC EURO SIGN is E2 82 AC — shares the 0xE2 lead byte but is NOT a
        // target; it must survive untouched.
        let euro = "€100".as_bytes().to_vec();
        let (out, n) = strip_unsafe_invisible(&euro);
        assert_eq!(n, 0);
        assert_eq!(out.as_ref(), euro.as_slice());
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
