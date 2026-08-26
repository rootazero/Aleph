//! Heuristic credential redaction for clipboard content.
//!
//! The `clipboard_read` IPC tool returns whatever the OS pasteboard holds;
//! `is_password_like` in [`crate::ax_secure`] only covers focused text-entry
//! fields (AX role + label set), so a credential that the user copied out of
//! a password manager lands in the model context with no defence in between.
//!
//! `clipboard_read` routes through [`redact_clipboard_text`] on the desktop
//! limb before the result crosses the wire. Long opaque tokens
//! (random-looking base64 / hex, no whitespace) are replaced with the
//! sentinel `<REDACTED:candidate-secret>` and the result carries a
//! `redacted: true` flag so the model can ask the user explicitly.
//!
//! The heuristic is deliberately conservative — false positives are
//! annoying (the user pastes something innocuous and the model sees
//! `<REDACTED>`); false negatives leak a credential. The defaults were
//! chosen so everyday text never triggers while token-shaped blobs do.

/// Replacement marker for a string that looked like a credential.
pub const REDACTED_SENTINEL: &str = "<REDACTED:candidate-secret>";

/// Minimum length (in chars) for a token to be considered a candidate.
/// 32 is the smallest AWS-style access key; shorter tokens have enough
/// collision risk with everyday text that we leave them alone.
pub const MIN_TOKEN_CHARS: usize = 32;

const HIGH_ENTROPY_CHARS: &str =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/=-_";

/// Outcome of redacting a clipboard string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactedText {
    /// The text with any candidate secrets replaced by [`REDACTED_SENTINEL`].
    pub text: String,
    /// Whether at least one redaction happened.
    pub redacted: bool,
}

/// Redact token-shaped substrings in `text` that look like credentials.
///
/// The detection rule is intentionally narrow:
/// 1. Whitespace-free run of 32+ characters from the high-entropy alphabet
///    `[A-Za-z0-9+/=\-_]`.
/// 2. Run contains at least two character classes (digit / upper / lower /
///    symbol) — pure-letter or pure-digit strings are far more likely to be
///    legitimate (model output, an identifier) than a credential.
///
/// Anything that does not match is returned verbatim — including
/// short tokens, whitespace-bearing strings, and runs that are pure letters
/// or pure digits.
#[must_use]
pub fn redact_clipboard_text(text: &str) -> RedactedText {
    if text.is_empty() {
        return RedactedText {
            text: String::new(),
            redacted: false,
        };
    }
    let mut out = String::with_capacity(text.len());
    let mut redacted = false;
    let mut buf = String::new();

    let flush = |buf: &mut String, out: &mut String| {
        if buf.is_empty() {
            return;
        }
        if looks_like_credential(buf) {
            out.push_str(REDACTED_SENTINEL);
        } else {
            out.push_str(buf);
        }
        buf.clear();
    };

    for ch in text.chars() {
        if ch.is_whitespace() {
            flush(&mut buf, &mut out);
            out.push(ch);
            continue;
        }
        if HIGH_ENTROPY_CHARS.contains(ch) {
            buf.push(ch);
        } else {
            // Non-token character: flush whatever we've accumulated (which
            // is now a non-token), copy the character verbatim, and start
            // a new token attempt.
            flush(&mut buf, &mut out);
            out.push(ch);
            // The verbatim char itself is not a credential — no flag.
        }
    }
    flush(&mut buf, &mut out);

    // Track whether redactions actually happened. `flush` uses the sentinel
    // when it replaces; we can detect that by comparing out after the run.
    redacted = out.contains(REDACTED_SENTINEL);
    RedactedText { text: out, redacted }
}

fn looks_like_credential(s: &str) -> bool {
    if s.len() < MIN_TOKEN_CHARS {
        return false;
    }
    let mut has_digit = false;
    let mut has_upper = false;
    let mut has_lower = false;
    let mut has_symbol = false;
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            has_digit = true;
        } else if ch.is_ascii_uppercase() {
            has_upper = true;
        } else if ch.is_ascii_lowercase() {
            has_lower = true;
        } else {
            // Anything outside [A-Za-z0-9] is a "symbol" — note this
            // includes base64 `+/=` and the URL-safe alphabet `-_`.
            has_symbol = true;
        }
    }
    // At least two distinct classes. Pure-letter and pure-digit runs are
    // a poor signal; mixed runs with digit OR symbol presence are the
    // strong credential signal.
    let classes = [has_digit, has_upper, has_lower, has_symbol]
        .iter()
        .filter(|x| **x)
        .count();
    classes >= 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_stays_empty() {
        let r = redact_clipboard_text("");
        assert!(!r.redacted);
        assert_eq!(r.text, "");
    }

    #[test]
    fn plain_text_passes_through() {
        let r = redact_clipboard_text("Hello, world!");
        assert!(!r.redacted);
        assert_eq!(r.text, "Hello, world!");
    }

    #[test]
    fn short_token_passes_through() {
        let r = redact_clipboard_text("a1b2c3");
        assert!(!r.redacted);
        assert_eq!(r.text, "a1b2c3");
    }

    #[test]
    fn long_mixed_token_is_redacted() {
        // 40+ chars, mixed letter/digit/base64-style.
        let tok = "abcdefghijklmnop0123456789+/ABCD";
        assert!(tok.len() >= MIN_TOKEN_CHARS);
        let r = redact_clipboard_text(tok);
        assert!(r.redacted);
        assert_eq!(r.text, REDACTED_SENTINEL);
    }

    #[test]
    fn long_pure_letter_token_passes_through() {
        // A 40-char all-lowercase run: could be prose / an identifier, not
        // a credential. Two-class minimum protects against this.
        let r = redact_clipboard_text("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        assert!(!r.redacted);
        assert_eq!(r.text, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    }

    #[test]
    fn long_pure_digit_token_passes_through() {
        let r = redact_clipboard_text("1234567890123456789012345678901234567890");
        assert!(!r.redacted);
        // Two-class rule catches this — pure digits are not a credential
        // signal.
        assert_eq!(
            r.text,
            "1234567890123456789012345678901234567890"
        );
    }

    #[test]
    fn embedded_token_inside_sentence_is_redacted() {
        let sentence = format!(
            "here is the API key {} and some trailing text",
            "abcdefghij0123456789ABCDEFG"
        );
        let r = redact_clipboard_text(&sentence);
        assert!(r.redacted);
        assert!(r.text.contains(REDACTED_SENTINEL));
    }

    #[test]
    fn whitespace_breaks_token_candidates() {
        // Two short tokens separated by space — neither should be redacted.
        let r = redact_clipboard_text("abcdefgh1 abcdefgh2");
        assert!(!r.redacted);
    }
}
