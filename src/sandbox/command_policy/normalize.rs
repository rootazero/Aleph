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
//! * empty quote tokens that collapse to nothing (`r''m`, `d""d`);
//! * Windows path prefixes that rename the same target — `\\?\C:\` and
//!   `\\.\C:\` address exactly what `C:\` addresses;
//! * PowerShell `-EncodedCommand <base64>`, which hides the *entire* script
//!   from every literal rule at once.
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
//! # Two views, because `\` is two different characters
//!
//! `\` is POSIX sh's escape character *and* Windows' path separator, and the
//! normaliser cannot tell which one it is looking at from the text alone.
//! Folding it (correct for `d\d`) destroys `C:\Windows`; keeping it (correct
//! for `C:\Windows`) lets `d\d` through. Earlier revisions folded
//! unconditionally, and every Windows rule carried an apology comment ("the
//! normaliser has already stripped path backslashes, so `\\?` is optional") —
//! which is how `\\?\C:\` came to normalise to `\?C:` and slip past the
//! catastrophic floor entirely.
//!
//! So the matching copy carries **both** readings, joined by a newline:
//!
//! * the **POSIX view** — `\` folded as an escape (`d\d` → `dd`);
//! * the **native view** — `\` preserved as a path separator (`C:\Windows`
//!   stays legible), with `^` / `` ` `` / empty quotes still folded.
//!
//! A rule matches if *either* view matches, so neither reading can hide a
//! command from the other. The second view is only emitted when it differs
//! (i.e. the text actually contains a backslash), so the common case still
//! costs one copy. Rules are single-line (`[^\n]*`) and the join is a `\n`, so
//! the seam cannot manufacture a cross-view match.
//!
//! Deliberately conservative otherwise: it folds exactly the tricks above and
//! nothing semantic. Newlines are preserved — they separate statements and
//! anchor the single-line rules and the head/tail scan seam in
//! [`super::CommandPolicy::evaluate`].

use std::borrow::Cow;

use base64::Engine as _;
use once_cell::sync::Lazy;
use regex::Regex;

use crate::sandbox::scrub::strip_unsafe_invisible;

/// Ceiling on decoded `-EncodedCommand` text appended to the matching copy.
/// A real agent command carries at most a handful of small scripts; the cap
/// keeps a hostile base64 bomb from turning one policy check into a latency
/// problem. The OS sandbox remains the backstop for anything past it.
const MAX_DECODED_BYTES: usize = 64 * 1024;

/// Ceiling on how many encoded payloads are decoded per command.
const MAX_DECODED_PAYLOADS: usize = 8;

/// How many times decoding re-runs over its own output, so an encoded command
/// that itself launches an encoded command is still unwrapped. Two covers every
/// observed nesting; deeper wrapping is left to the OS sandbox.
const MAX_DECODE_ROUNDS: usize = 2;

/// `-EncodedCommand` and every abbreviation PowerShell resolves to it
/// (`-e`, `-ec`, `-enc`, `-encodedcommand`, and the `/`-prefixed spellings),
/// followed by the base64 payload. Capture 1 is the payload.
///
/// The 16-character floor on the payload body is what keeps `-ExecutionPolicy
/// Unrestricted` and `grep -e pattern` from being decoded as if they were
/// payloads, while still reaching the short end of the real range: PowerShell
/// encodes UTF-16LE, so 16 base64 characters is a six-character script — the
/// length of `rd D:\`. A higher floor reads better on paper and silently misses
/// exactly the shortest catastrophic commands.
static ENCODED_COMMAND_RE: Lazy<Regex> = Lazy::new(|| {
    // rust-doctor-disable-next-line unwrap-in-production
    Regex::new(r#"(?i)[-/]e(?:c|n[a-z]*)?\s+["']?([a-zA-Z0-9+/]{16,}={0,2})"#)
        .expect("encoded-command pattern must compile")
});

/// Fold cheap shell obfuscation out of `text` for pattern matching.
///
/// Returns [`Cow::Borrowed`] when `text` contains none of the targeted tricks
/// (the common case — agent commands are usually plain), so a clean command
/// costs only a scan, not an allocation. Otherwise returns the POSIX view,
/// followed by a newline and the native view when they differ, and then by any
/// decoded `-EncodedCommand` payloads.
#[must_use]
pub fn normalize_for_matching(text: &str) -> Cow<'_, str> {
    let has_escape_or_quote = text
        .as_bytes()
        .iter()
        .any(|&b| matches!(b, b'\\' | b'\'' | b'"' | b'^' | b'`'));
    let (stripped, removed) = strip_unsafe_invisible(text.as_bytes());

    // Fast path: no invisible sequences removed, no escape/quote tricks, and
    // nothing that could be an encoded payload.
    if removed == 0 && !has_escape_or_quote && !may_carry_encoded_payload(text) {
        return Cow::Borrowed(text);
    }

    // `strip_unsafe_invisible` only removes whole invisible UTF-8 sequences, so
    // the remaining bytes are still valid UTF-8; `from_utf8_lossy` is a
    // defensive no-op that also handles the borrowed (unchanged) case.
    let stripped: Cow<'_, str> = if removed == 0 {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(String::from_utf8_lossy(&stripped).into_owned())
    };
    // Runs before folding: the prefixes are made of backslashes, which the
    // POSIX view is about to consume.
    let canonical = strip_windows_path_prefixes(&stripped);

    let mut scan = if has_escape_or_quote {
        let posix = fold_escapes_and_quotes(&canonical, true);
        // The two views differ exactly when a backslash was folded.
        if canonical.contains('\\') {
            let native = fold_escapes_and_quotes(&canonical, false);
            if native == posix {
                posix
            } else {
                let mut both = posix;
                both.push('\n');
                both.push_str(&native);
                both
            }
        } else {
            posix
        }
    } else {
        canonical.into_owned()
    };

    // Tested against the *folded* text, not the original: `-e^nc <base64>` only
    // looks like an encoded command after the cmd caret has been folded away.
    if may_carry_encoded_payload(&scan) {
        if let Some(decoded) = expand_encoded_payloads(&scan) {
            scan.push('\n');
            scan.push_str(&decoded);
        }
    }
    Cow::Owned(scan)
}

/// Cheap pre-filter for [`ENCODED_COMMAND_RE`]: an encoded payload always
/// follows a `-e…` / `/e…` switch, so a command with no such two-byte run
/// cannot carry one and skips the regex entirely.
fn may_carry_encoded_payload(text: &str) -> bool {
    text.as_bytes()
        .windows(2)
        .any(|w| matches!(w[0], b'-' | b'/') && matches!(w[1], b'e' | b'E'))
}

/// Rewrite Windows' alternate spellings of the same path to the plain form:
/// `\\?\C:\` and `\\.\C:\` → `C:\`, `\\?\UNC\srv\share` → `\\srv\share`.
///
/// These prefixes are not obfuscation in intent — they are the Win32
/// extended-length and device namespaces — but they are perfect obfuscation in
/// effect: `del /s /q \\?\C:\` wipes exactly what `del /s /q C:\` wipes while
/// looking nothing like it to a literal matcher.
fn strip_windows_path_prefixes(s: &str) -> Cow<'_, str> {
    // Fast reject: every prefix begins with a doubled backslash.
    if !s.contains("\\\\") {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(idx) = rest.find("\\\\") {
        out.push_str(&rest[..idx]);
        let after = &rest[idx + 2..];
        if after.starts_with("?\\") || after.starts_with(".\\") {
            let body = &after[2..];
            if body
                .get(..4)
                .is_some_and(|p| p.eq_ignore_ascii_case("unc\\"))
            {
                // The UNC namespace maps back onto the plain `\\server\share`.
                out.push_str("\\\\");
                rest = &body[4..];
            } else {
                rest = body;
            }
        } else {
            // A genuine `\\server\share` (or a doubled escape) — keep it.
            out.push_str("\\\\");
            rest = after;
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// Single pass that drops shell escape characters and collapses empty quote
/// pairs.
///
/// `fold_backslash` selects the view: `true` treats `\` as POSIX sh's escape
/// (the POSIX view), `false` leaves it in place as a Windows path separator
/// (the native view). `^` (cmd.exe) and `` ` `` (PowerShell) fold in both views
/// — neither is a path character, so there is no ambiguity to resolve.
fn fold_escapes_and_quotes(s: &str, fold_backslash: bool) -> String {
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
            '\\' if fold_backslash => match chars.next() {
                Some('\n') => {}
                Some(next) => out.push(next),
                None => {}
            },
            '^' | '`' => match chars.next() {
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

/// Decode every `-EncodedCommand` payload reachable from `scan`, re-running
/// over the decoded text so a nested encoding is unwrapped too.
///
/// Returns the decoded scripts joined by newlines, or `None` when nothing
/// decoded to plausible text. Appending can only *add* signal: a token that is
/// not really an encoded script either fails to decode or decodes to bytes the
/// text check rejects.
fn expand_encoded_payloads(scan: &str) -> Option<String> {
    let mut budget = MAX_DECODED_PAYLOADS;
    let mut current = decode_round(scan, &mut budget)?;
    let mut appended = String::with_capacity(current.len());
    appended.push_str(&current);
    for _ in 1..MAX_DECODE_ROUNDS {
        if budget == 0 || appended.len() >= MAX_DECODED_BYTES {
            break;
        }
        let Some(next) = decode_round(&current, &mut budget) else {
            break;
        };
        appended.push('\n');
        appended.push_str(&next);
        current = next;
    }
    Some(appended)
}

/// One decoding sweep over `source`, drawing from the shared payload budget.
fn decode_round(source: &str, budget: &mut usize) -> Option<String> {
    let mut out = String::new();
    for caps in ENCODED_COMMAND_RE.captures_iter(source) {
        if *budget == 0 || out.len() >= MAX_DECODED_BYTES {
            break;
        }
        let Some(token) = caps.get(1) else {
            continue;
        };
        let Some(decoded) = decode_encoded_command(token.as_str()) else {
            continue;
        };
        *budget -= 1;
        if !out.is_empty() {
            out.push('\n');
        }
        // The decoded text is a PowerShell script by construction, so fold its
        // backtick escapes but keep backslashes — `-EncodedCommand` payloads
        // are never POSIX sh.
        out.push_str(&fold_escapes_and_quotes(&decoded, false));
    }
    (!out.is_empty()).then_some(out)
}

/// Base64-decode one `-EncodedCommand` payload into script text.
///
/// PowerShell encodes UTF-16LE; a UTF-8 fallback covers hand-rolled callers
/// that got the encoding wrong but still run. Returns `None` unless the result
/// reads as text, which is what keeps an accidental match on a long
/// alphanumeric argument from injecting binary noise into the scan.
fn decode_encoded_command(token: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(token)
        .ok()
        .or_else(|| {
            base64::engine::general_purpose::STANDARD_NO_PAD
                .decode(token)
                .ok()
        })?;
    if bytes.is_empty() || bytes.len() > MAX_DECODED_BYTES {
        return None;
    }
    if bytes.len() % 2 == 0 {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        if let Some(text) = String::from_utf16(&units).ok().filter(|s| reads_as_text(s)) {
            return Some(text);
        }
    }
    String::from_utf8(bytes).ok().filter(|s| reads_as_text(s))
}

/// Whether a decoded payload plausibly *is* a script rather than binary noise.
/// A single control byte other than the usual whitespace is enough to reject,
/// because a real script has none.
fn reads_as_text(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Owned-string view of the normalised text — `String` compares cleanly
    /// against string literals, sidestepping `Cow`/`&str` `PartialEq` surface.
    fn norm(s: &str) -> String {
        normalize_for_matching(s).into_owned()
    }

    /// The POSIX reading on its own, for the fold-semantics assertions that
    /// predate the two-view split.
    fn posix(s: &str) -> String {
        fold_escapes_and_quotes(&strip_windows_path_prefixes(s), true)
    }

    /// The native (path-preserving) reading on its own.
    fn native(s: &str) -> String {
        fold_escapes_and_quotes(&strip_windows_path_prefixes(s), false)
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
            posix(r"d\d if=/dev/zero o\f=/dev/sda"),
            "dd if=/dev/zero of=/dev/sda"
        );
    }

    #[test]
    fn line_continuation_is_joined() {
        // `\`-newline is removed entirely by the shell (the lines join).
        assert_eq!(posix("rm -r\\\nf /etc"), "rm -rf /etc");
    }

    #[test]
    fn empty_quote_pairs_collapse() {
        assert_eq!(posix("r''m -rf /"), "rm -rf /");
        assert_eq!(posix(r#"d""d if=x"#), "dd if=x");
    }

    #[test]
    fn nonempty_quotes_are_preserved() {
        // A real quoted token must survive — only *empty* pairs fold.
        assert_eq!(posix(r#"echo "hi there""#), r#"echo "hi there""#);
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
        assert_eq!(posix("echo a\necho b"), "echo a\necho b");
    }

    #[test]
    fn trailing_backslash_is_dropped() {
        assert_eq!(posix("echo hi\\"), "echo hi");
    }

    #[test]
    fn cmd_caret_escape_is_folded() {
        // cmd.exe `^` escape: `de^l`/`fo^rmat` are how a caller hides the
        // keyword from a literal matcher; the shell runs them as `del`/`format`.
        assert_eq!(posix("de^l /s /q C:"), "del /s /q C:");
        assert_eq!(posix("fo^rmat C:"), "format C:");
    }

    #[test]
    fn powershell_backtick_escape_is_folded() {
        // PowerShell `` ` `` escape: `` Remo`ve-Item `` runs as `Remove-Item`.
        assert_eq!(posix("Remo`ve-Item -Recurse C:"), "Remove-Item -Recurse C:");
    }

    #[test]
    fn caret_line_continuation_is_joined() {
        // `^`-newline is a cmd.exe line continuation — both chars drop.
        assert_eq!(posix("format^\nC:"), "formatC:");
    }

    // --- Two-view normalisation -----------------------------------------

    #[test]
    fn caret_and_backtick_fold_in_the_native_view_too() {
        // Only `\` is view-dependent: the cmd / PowerShell escapes are not path
        // characters, so both views fold them.
        assert_eq!(native(r"de^l /s /q C:\"), r"del /s /q C:\");
        assert_eq!(
            native("Remo`ve-Item -Recurse C:\\"),
            r"Remove-Item -Recurse C:\"
        );
    }

    #[test]
    fn native_view_preserves_windows_path_separators() {
        // The POSIX reading destroys the path; the native one keeps it, which
        // is what lets a rule name `\Windows` at all.
        assert_eq!(
            posix(r"Remove-Item -Recurse C:\Windows"),
            "Remove-Item -Recurse C:Windows"
        );
        assert_eq!(
            native(r"Remove-Item -Recurse C:\Windows"),
            r"Remove-Item -Recurse C:\Windows"
        );
    }

    #[test]
    fn both_views_are_emitted_when_they_differ() {
        let out = norm(r"Remove-Item -Recurse C:\Windows");
        let (posix_line, native_line) = out.split_once('\n').expect("two views");
        assert_eq!(posix_line, "Remove-Item -Recurse C:Windows");
        assert_eq!(native_line, r"Remove-Item -Recurse C:\Windows");
    }

    #[test]
    fn single_view_when_no_backslash_is_present() {
        // A quote-only trick has one reading, so no second copy is emitted.
        assert_eq!(norm("r''m -rf /"), "rm -rf /");
    }

    #[test]
    fn posix_escape_still_folds_with_the_native_view_present() {
        // The union is what matters: the POSIX reading un-hides `dd`, and the
        // native reading being wrong about it costs nothing.
        let out = norm(r"d\d if=/dev/zero of=/dev/sda");
        assert!(
            out.contains("dd if=/dev/zero of=/dev/sda"),
            "POSIX view must un-hide the keyword: {out}"
        );
    }

    // --- Windows path-prefix canonicalisation ---------------------------

    #[test]
    fn extended_length_prefix_is_rewritten_to_the_plain_path() {
        // `\\?\C:\` addresses exactly what `C:\` addresses. Before this it
        // normalised to `\?C:` and slipped past the catastrophic floor.
        assert_eq!(native(r"del /s /q \\?\C:\"), r"del /s /q C:\");
        assert_eq!(native(r"del /s /q \\.\C:\"), r"del /s /q C:\");
    }

    #[test]
    fn unc_namespace_prefix_folds_back_to_the_plain_unc_path() {
        assert_eq!(native(r"dir \\?\UNC\srv\share"), r"dir \\srv\share");
    }

    #[test]
    fn ordinary_unc_path_is_left_alone() {
        assert_eq!(native(r"dir \\srv\share"), r"dir \\srv\share");
    }

    #[test]
    fn path_prefix_rewrite_is_skipped_without_a_doubled_backslash() {
        assert!(matches!(
            strip_windows_path_prefixes(r"del C:\temp"),
            Cow::Borrowed(_)
        ));
    }

    // --- PowerShell -EncodedCommand expansion ---------------------------

    /// Encode `script` the way `powershell -EncodedCommand` expects it.
    fn encode(script: &str) -> String {
        let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
        base64::engine::general_purpose::STANDARD.encode(utf16)
    }

    #[test]
    fn encoded_command_payload_is_decoded_into_the_scan_text() {
        let cmd = format!(
            "powershell -EncodedCommand {}",
            encode(r"Remove-Item -Recurse -Force C:\")
        );
        let out = norm(&cmd);
        assert!(
            out.contains(r"Remove-Item -Recurse -Force C:\"),
            "payload must be visible to the rules: {out}"
        );
    }

    #[test]
    fn every_encodedcommand_abbreviation_is_decoded() {
        // PowerShell resolves each of these to -EncodedCommand.
        for flag in ["-e", "-ec", "-enc", "-EncodedCommand", "/enc"] {
            let cmd = format!(
                "powershell {flag} {}",
                encode("dd if=/dev/zero of=/dev/sda")
            );
            assert!(
                norm(&cmd).contains("of=/dev/sda"),
                "{flag} payload must decode"
            );
        }
    }

    #[test]
    fn caret_obfuscated_encoded_flag_is_still_decoded() {
        // `-e^nc` runs as `-enc`. The payload scan therefore has to happen on
        // the *folded* text: testing the original would miss `-^enc` entirely.
        let payload = encode("wipefs -a /dev/sda");
        for flag in ["-e^nc", "-^enc", "-en^c"] {
            assert!(
                norm(&format!("powershell {flag} {payload}")).contains("wipefs -a /dev/sda"),
                "{flag} must decode after the caret fold"
            );
        }
    }

    #[test]
    fn nested_encoded_command_is_unwrapped() {
        let inner = format!("powershell -enc {}", encode("wipefs -a /dev/sda"));
        let outer = format!("powershell -enc {}", encode(&inner));
        assert!(
            norm(&outer).contains("wipefs -a /dev/sda"),
            "a payload that launches another payload must still be seen"
        );
    }

    #[test]
    fn utf8_encoded_payload_is_decoded_too() {
        let payload =
            base64::engine::general_purpose::STANDARD.encode("dd if=/dev/zero of=/dev/sda");
        let out = norm(&format!("powershell -enc {payload}"));
        assert!(
            out.contains("of=/dev/sda"),
            "UTF-8 fallback must decode: {out}"
        );
    }

    #[test]
    fn execution_policy_argument_is_not_mistaken_for_a_payload() {
        // `-e…` switches with a short operand cannot be payloads — the base64
        // length floor is what keeps them out.
        for cmd in [
            "powershell -ExecutionPolicy Bypass -File build.ps1",
            "powershell -ExecutionPolicy Unrestricted -File build.ps1",
        ] {
            assert_eq!(norm(cmd), cmd, "no spurious decode");
        }
    }

    #[test]
    fn a_very_short_encoded_command_is_still_decoded() {
        // The floor has to clear the shortest *real* command, not just the
        // comfortable middle: `rd D:\` encodes to exactly 16 base64 characters,
        // and a floor set for readability rather than measurement would let the
        // most catastrophic short commands through.
        let out = norm(&format!("powershell -enc {}", encode(r"rd D:\")));
        assert!(out.contains(r"rd D:\"), "short payload must decode: {out}");
    }

    #[test]
    fn binary_decode_never_reaches_the_scan_text() {
        // Decoding succeeds byte-wise on plenty of accidental inputs; the text
        // check is what stops binary noise from entering the scan.
        let out = norm("grep -e AAAAAAAAAAAAAAAAAAAAAAAAAAAA file.txt");
        assert!(
            !out.chars().any(|c| c.is_control() && c != '\n'),
            "binary decode must not reach the scan text: {out:?}"
        );
    }

    #[test]
    fn decoded_payload_count_is_bounded() {
        let payload = encode("echo hi");
        let cmd = (0..MAX_DECODED_PAYLOADS + 6)
            .map(|_| format!("powershell -enc {payload}"))
            .collect::<Vec<_>>()
            .join(" ; ");
        // Only the budgeted payloads are decoded, so the appended text carries
        // at most `MAX_DECODED_PAYLOADS` copies of the script.
        let appended = norm(&cmd);
        let decoded_copies = appended.matches("echo hi").count();
        assert!(
            decoded_copies <= MAX_DECODED_PAYLOADS,
            "decode budget must cap the appended text, got {decoded_copies}"
        );
        assert!(decoded_copies > 0, "at least one payload must decode");
    }
}
