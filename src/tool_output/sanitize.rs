//! Sanitize raw command (`shell` / `python` / `javascript`) output before it
//! becomes the agent-facing tool envelope.
//!
//! Build tools, test runners and CLIs colourise output with ANSI/VT100 escape
//! sequences and occasionally emit stray binary control bytes (NUL, backspace,
//! bell, form-feed…). Forwarded verbatim into the model's context they waste
//! tokens and corrupt the surrounding text. The reference agents all clean
//! this up: pi runs `sanitizeBinaryOutput(stripAnsi(...))` on *every* output
//! chunk and openclaw has an equivalent sanitiser.
//!
//! Aleph previously stripped ANSI **only** on the error-digest path
//! ([`crate::tool_output::distill`]); clean output and the head/tail truncation
//! fallback kept the raw escape codes. This module closes that gap as a single
//! zero-copy pass that surpasses the references:
//!
//! - reuses the existing [`strip_ansi`] (one source of truth for escape
//!   handling — no duplicate scanner),
//! - then drops the residual non-printable C0/C1 control bytes that ANSI
//!   stripping leaves behind, keeping only the `\t` / `\n` / `\r` whitespace
//!   controls real output relies on,
//! - returns [`Cow::Borrowed`] for already-clean text, so output with no
//!   escape byte and no stray control char is **byte-identical** — the change
//!   is non-breaking by construction.

use std::borrow::Cow;

use super::distill::strip_ansi;

/// Control characters we KEEP — the whitespace trio that carries real
/// structure in command output.
#[inline]
const fn is_kept_control(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r')
}

/// Whether `c` is a non-printable control character we should drop: the C0
/// range (`0x00..=0x1f`) minus the kept whitespace trio, `DEL` (`0x7f`), and
/// the C1 range (`0x80..=0x9f`). ANSI `ESC` (`0x1b`) is handled upstream by
/// [`strip_ansi`]; any lone `ESC` it leaves behind is caught here too.
#[inline]
fn is_droppable_control(c: char) -> bool {
    if is_kept_control(c) {
        return false;
    }
    let cp = c as u32;
    cp < 0x20 || cp == 0x7f || (0x80..=0x9f).contains(&cp)
}

/// Strip ANSI/VT100 escapes and non-printable control bytes from raw command
/// output. Clean input is returned untouched ([`Cow::Borrowed`]).
///
/// Operates on the whole (multi-line) buffer in one pass — `strip_ansi` treats
/// `\n` as an ordinary character, so there is no need to split lines first.
pub fn sanitize_command_output(text: &str) -> Cow<'_, str> {
    // Stage 1: ANSI/VT100 escapes (reuses the distill scanner).
    let no_ansi = strip_ansi(text);

    // Stage 2: residual non-printable control bytes. Fast-path when there are
    // none so we preserve the borrow (and thus byte-identity) from stage 1.
    if !no_ansi.chars().any(is_droppable_control) {
        return no_ansi;
    }
    let cleaned: String = no_ansi
        .chars()
        .filter(|c| !is_droppable_control(*c))
        .collect();
    Cow::Owned(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_text_is_borrowed_and_byte_identical() {
        let input = "Compiling alephcore v0.1.0\n   Finished in 3.2s\n";
        let out = sanitize_command_output(input);
        assert!(
            matches!(out, Cow::Borrowed(_)),
            "clean output must not allocate"
        );
        assert_eq!(out, input);
    }

    #[test]
    fn keeps_whitespace_controls() {
        let input = "col1\tcol2\r\nrow\twith\ttabs\n";
        let out = sanitize_command_output(input);
        assert_eq!(out, input, "tab/cr/lf must survive untouched");
    }

    #[test]
    fn strips_ansi_color_codes() {
        // `cargo`/`pytest`-style colourised error line.
        let input = "\u{1b}[1m\u{1b}[31merror\u{1b}[0m: boom";
        let out = sanitize_command_output(input);
        assert_eq!(out, "error: boom");
    }

    #[test]
    fn drops_binary_control_bytes() {
        // NUL, backspace, bell, form-feed, DEL embedded in otherwise good text.
        let input = "ok\u{0}go\u{8}\u{7}\u{c}done\u{7f}!";
        let out = sanitize_command_output(input);
        assert_eq!(out, "okgodone!");
    }

    #[test]
    fn drops_c1_controls_but_keeps_unicode_text() {
        // U+0085 NEL (C1 control) dropped; ordinary multibyte text preserved.
        let input = "résumé\u{85}—café ☕";
        let out = sanitize_command_output(input);
        assert_eq!(out, "résumé—café ☕");
    }

    #[test]
    fn combined_ansi_and_binary_single_pass() {
        let input = "\u{1b}[32mPASS\u{1b}[0m\u{0} 12 tests\n";
        let out = sanitize_command_output(input);
        assert_eq!(out, "PASS 12 tests\n");
    }

    #[test]
    fn empty_stays_empty_and_borrowed() {
        let out = sanitize_command_output("");
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out, "");
    }
}
