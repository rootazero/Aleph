//! Constant-time secret comparison.
//!
//! Single entry point for comparing secret bytes/strings (HMAC signatures,
//! bearer tokens, webhook secrets, OAuth state). Wraps `subtle::ConstantTimeEq`
//! with pad-to-equal-length semantics so callers cannot leak length via early
//! return, while still rejecting length-mismatched inputs.
//!
//! Mirrors `openclaw/src/security/secret-equal.ts` to keep the trust model
//! consistent across runtimes.

use subtle::ConstantTimeEq;

/// Constant-time byte-slice equality with length-leak protection.
///
/// Returns `true` iff `provided` and `expected` have identical contents AND
/// identical lengths. The comparison cost is `max(provided.len, expected.len)`
/// regardless of where bytes first differ, so an attacker cannot infer
/// matching-prefix length from timing.
///
/// Two empty inputs compare equal.
#[must_use]
pub fn secret_equal_bytes(provided: &[u8], expected: &[u8]) -> bool {
    let n = provided.len().max(expected.len());
    if n == 0 {
        return true;
    }
    let mut diff = 0u8;
    for i in 0..n {
        let p = provided.get(i).copied().unwrap_or(0);
        let e = expected.get(i).copied().unwrap_or(0);
        diff |= p ^ e;
    }
    (diff.ct_eq(&0) & provided.len().ct_eq(&expected.len())).into()
}

/// Convenience wrapper for UTF-8 string secrets (bearer tokens, signature
/// headers). Uses byte-level comparison; treats either side being absent as
/// "not equal".
///
/// Empty strings on either side are rejected outright: a misconfigured
/// expected secret (blank config value, partially-initialized store) would
/// otherwise accept any caller that also happens to send an empty token,
/// turning a length-zero string into an authentication bypass. The byte-level
/// primitive [`secret_equal_bytes`] still treats two empty inputs as equal —
/// that contract is preserved for callers that genuinely need it (fuzz
/// harnesses, hash-equality with a zero-length digest).
#[must_use]
pub fn secret_equal(provided: Option<&str>, expected: Option<&str>) -> bool {
    match (provided, expected) {
        (Some(p), Some(e)) if !p.is_empty() && !e.is_empty() => {
            secret_equal_bytes(p.as_bytes(), e.as_bytes())
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_bytes_match() {
        assert!(secret_equal_bytes(b"hunter2", b"hunter2"));
    }

    #[test]
    fn differing_bytes_reject() {
        assert!(!secret_equal_bytes(b"hunter2", b"hunter3"));
    }

    #[test]
    fn length_mismatch_rejects_even_if_prefix_matches() {
        assert!(!secret_equal_bytes(b"hunter", b"hunter2"));
        assert!(!secret_equal_bytes(b"hunter2", b"hunter"));
    }

    #[test]
    fn empty_inputs_are_equal() {
        assert!(secret_equal_bytes(b"", b""));
    }

    #[test]
    fn one_side_empty_is_not_equal() {
        assert!(!secret_equal_bytes(b"", b"x"));
        assert!(!secret_equal_bytes(b"x", b""));
    }

    #[test]
    fn str_wrapper_handles_none() {
        assert!(!secret_equal(None, Some("x")));
        assert!(!secret_equal(Some("x"), None));
        assert!(!secret_equal(None, None));
    }

    #[test]
    fn str_wrapper_matches() {
        assert!(secret_equal(Some("abc"), Some("abc")));
        assert!(!secret_equal(Some("abc"), Some("abd")));
    }

    /// Regression for `severed-wire-2026-09-05-modules2 security I-2`:
    /// empty string on either side used to authenticate (the byte-level
    /// primitive returns true for two empty inputs, and the wrapper
    /// forwarded). A misconfigured blank secret would then accept any
    /// caller that also sent an empty token — an auth bypass. The wrapper
    /// now rejects empty strings outright. The byte-level
    /// [`secret_equal_bytes`] contract is preserved.
    #[test]
    fn str_wrapper_rejects_empty_strings_on_either_side() {
        assert!(!secret_equal(Some(""), Some("")));
        assert!(!secret_equal(Some(""), Some("real-token")));
        assert!(!secret_equal(Some("real-token"), Some("")));
    }

    #[test]
    fn high_bit_bytes_compare_correctly() {
        let a = [0u8, 0xFF, 0x80, 0x7F];
        let b = [0u8, 0xFF, 0x80, 0x7F];
        let c = [0u8, 0xFF, 0x80, 0x7E];
        assert!(secret_equal_bytes(&a, &b));
        assert!(!secret_equal_bytes(&a, &c));
    }
}
