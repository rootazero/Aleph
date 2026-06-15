//! De-obfuscation normaliser for the command-policy hard-filter.
//!
//! The [`rules`](super::rules) regexes match against the literal command text.
//! A motivated caller can defeat a literal-text matcher with cheap shell
//! obfuscation that the OS still executes verbatim:
//!
//! * invisible / zero-width characters spliced into a keyword
//!   (`d<U+200B>d if=…`, RTL/BOM overrides);
//! * escape characters the shell strips at parse time — `\` (POSIX sh,
//!   `r\m -rf` / `d\d if=…`), `^` (cmd.exe, `de^l /s C:\`), `` ` ``
//!   (PowerShell, `` Remo`ve-Item ``) — including their line continuations
//!   (`rm -r\<newline>f`);
//! * empty quote tokens that collapse to nothing (`r''m`, `d""d`).
//!
//! None of these change what the shell runs, but each can slip a catastrophic
//! pattern past a naive regex. This module produces a *matching copy* with
//! those tricks folded out; the original command is never mutated (the shell
//! still sees exactly what the model wrote).
//!
//! It maps hermes-agent's `_normalize_command_for_detection` (NFKC + escape /
//! empty-token stripping) onto Aleph, reusing the existing invisible-character
//! stripper ([`crate::sandbox::scrub::strip_unsafe_invisible`]) so there is one
//! source of truth for "unsafe invisible bytes" (R7 hard-filter — deterministic,
//! no content scoring).
//!
//! Deliberately conservative: it folds exactly the tricks above and nothing
//! semantic, so it cannot turn a clean command into a false positive beyond
//! those literal substitutions. Newlines are preserved — they separate
//! statements and anchor the single-line (`[^\n]*`) rules and the head/tail
//! scan seam in [`super::CommandPolicy::evaluate`].

use std::borrow::Cow;

use crate::sandbox::scrub::strip_unsafe_invisible;

/// Fold cheap shell obfuscation out of `text` for pattern matching.
///
/// Returns [`Cow::Borrowed`] when `text` contains none of the targeted tricks
/// (the common case — agent commands are usually plain), so a clean command
/// costs only a scan, not an allocation.
#[must_use]
pub fn normalize_for_matching(text: &str) -> Cow<'_, str> {
    let has_escape_or_quote = text
        .as_bytes()
        .iter()
        .any(|&b| matches!(b, b'\\' | b'\'' | b'"' | b'^' | b'`'));
    let (stripped, removed) = strip_unsafe_invisible(text.as_bytes());

    // Fast path: no invisible sequences removed and no escape/quote tricks.
    if removed == 0 && !has_escape_or_quote {
        return Cow::Borrowed(text);
    }

    // `strip_unsafe_invisible` only removes whole invisible UTF-8 sequences, so
    // the remaining bytes are still valid UTF-8; `from_utf8_lossy` is a
    // defensive no-op that also handles the borrowed (unchanged) case.
    let stripped: String = String::from_utf8_lossy(&stripped).into_owned();

    if !has_escape_or_quote {
        return Cow::Owned(stripped);
    }
    Cow::Owned(fold_escapes_and_quotes(&stripped))
}

/// Single pass that drops shell escape characters — `\` (POSIX), `^`
/// (cmd.exe), `` ` `` (PowerShell), including their `-newline line
/// continuations — and collapses empty quote pairs.
fn fold_escapes_and_quotes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // Escape characters the shell strips at parse time. Reveal the
            // escaped char so the obfuscated keyword folds back to plain text:
            //   * `\` (POSIX sh)      — `r\m`/`d\d` → `rm`/`dd`
            //   * `^` (cmd.exe)       — `de^l`/`fo^rmat` → `del`/`format`
            //   * `` ` `` (PowerShell) — `` Remo`ve-Item `` → `Remove-Item`
            // An escape immediately before a newline is a line continuation in
            // all three shells, so both chars drop (the shell joins the lines).
            '\\' | '^' | '`' => match chars.next() {
                Some('\n') => {}
                Some(next) => out.push(next),
                None => {}
            },
            // An *empty* quote pair (`''` / `""`) collapses to nothing
            // (`r''m` → `rm`). Non-empty quotes are kept so token boundaries
            // and quoted content survive.
            '\'' | '"' if chars.peek() == Some(&c) => {
                chars.next();
            }
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Owned-string view of the normalised text — `String` compares cleanly
    /// against string literals, sidestepping `Cow`/`&str` `PartialEq` surface.
    fn norm(s: &str) -> String {
        normalize_for_matching(s).into_owned()
    }

    #[test]
    fn plain_command_is_borrowed_unchanged() {
        let out = normalize_for_matching("dd if=/dev/zero of=/dev/sda bs=1M");
        assert!(matches!(out, Cow::Borrowed(_)), "no tricks → no allocation");
        assert_eq!(out.into_owned(), "dd if=/dev/zero of=/dev/sda bs=1M");
    }

    #[test]
    fn backslash_escape_is_folded() {
        // `d\d`/`o\f` are how a caller hides `dd`/`of` from a literal matcher.
        assert_eq!(
            norm(r"d\d if=/dev/zero o\f=/dev/sda"),
            "dd if=/dev/zero of=/dev/sda"
        );
    }

    #[test]
    fn line_continuation_is_joined() {
        // `\`-newline is removed entirely by the shell (the lines join).
        assert_eq!(norm("rm -r\\\nf /etc"), "rm -rf /etc");
    }

    #[test]
    fn empty_quote_pairs_collapse() {
        assert_eq!(norm("r''m -rf /"), "rm -rf /");
        assert_eq!(norm(r#"d""d if=x"#), "dd if=x");
    }

    #[test]
    fn nonempty_quotes_are_preserved() {
        // A real quoted token must survive — only *empty* pairs fold.
        assert_eq!(norm(r#"echo "hi there""#), r#"echo "hi there""#);
    }

    #[test]
    fn invisible_zero_width_is_stripped() {
        // U+200B ZERO WIDTH SPACE spliced into `dd` is removed by the shared
        // invisible-character stripper.
        assert_eq!(
            norm("d\u{200b}d if=/dev/zero of=/dev/sda"),
            "dd if=/dev/zero of=/dev/sda"
        );
    }

    #[test]
    fn newlines_are_preserved() {
        // Statement separators must survive so single-line rules stay anchored.
        assert_eq!(norm("echo a\necho b"), "echo a\necho b");
    }

    #[test]
    fn trailing_backslash_is_dropped() {
        assert_eq!(norm("echo hi\\"), "echo hi");
    }

    #[test]
    fn cmd_caret_escape_is_folded() {
        // cmd.exe `^` escape: `de^l`/`fo^rmat` are how a caller hides the
        // keyword from a literal matcher; the shell runs them as `del`/`format`.
        assert_eq!(norm("de^l /s /q C:"), "del /s /q C:");
        assert_eq!(norm("fo^rmat C:"), "format C:");
    }

    #[test]
    fn powershell_backtick_escape_is_folded() {
        // PowerShell `` ` `` escape: `` Remo`ve-Item `` runs as `Remove-Item`.
        assert_eq!(norm("Remo`ve-Item -Recurse C:"), "Remove-Item -Recurse C:");
    }

    #[test]
    fn caret_line_continuation_is_joined() {
        // `^`-newline is a cmd.exe line continuation — both chars drop.
        assert_eq!(norm("format^\nC:"), "formatC:");
    }
}
