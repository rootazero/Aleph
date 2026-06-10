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
    let p = pad(provided, n);
    let e = pad(expected, n);
    // ct_eq dominates the cost; the length check is an exact-match gate
    // applied after the constant-time compare so it does not short-circuit.
    let eq: bool = p.ct_eq(&e).into();
    eq && provided.len() == expected.len()
}

/// Convenience wrapper for UTF-8 string secrets (bearer tokens, signature
/// headers). Uses byte-level comparison; treats either side being absent as
/// "not equal".
#[must_use]
pub fn secret_equal(provided: Option<&str>, expected: Option<&str>) -> bool {
    match (provided, expected) {
        (Some(p), Some(e)) => secret_equal_bytes(p.as_bytes(), e.as_bytes()),
        _ => false,
    }
}

fn pad(input: &[u8], n: usize) -> Vec<u8> {
    if input.len() == n {
        input.to_vec()
    } else {
        let mut out = vec![0u8; n];
        out[..input.len()].copy_from_slice(input);
        out
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

    #[test]
    fn high_bit_bytes_compare_correctly() {
        let a = [0u8, 0xFF, 0x80, 0x7F];
        let b = [0u8, 0xFF, 0x80, 0x7F];
        let c = [0u8, 0xFF, 0x80, 0x7E];
        assert!(secret_equal_bytes(&a, &b));
        assert!(!secret_equal_bytes(&a, &c));
    }
}
