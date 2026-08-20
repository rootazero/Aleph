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
//! * quote tokens that vanish at parse time — the empty pairs (`r''m`, `d""d`)
//!   *and* the non-empty ones that splice a keyword (`d'd'`, `r"m"`, `de'l'`),
//!   which the shell joins back into one word;
//! * `$IFS` / `${IFS}`, whose expansion supplies the word separator a rule
//!   wants to anchor on (`rm${IFS}-rf${IFS}/`);
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
//! # Several views, because one folding cannot be right for every reading
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
//! Quotes have the same shape of problem in the other direction: dropping them
//! is correct for `d'd'` (the shell runs `dd`) but *loses* the left token
//! boundary several Windows rules anchor on (`del /s /q'C:\'` — see
//! `win_bare_root!`).
//!
//! So the matching copy carries **every** reading, newline-joined, and a rule
//! matches if any one of them matches (see [`append_views`]):
//!
//! * the **POSIX view** — `\` folded as an escape (`d\d` → `dd`);
//! * the **native view** — `\` preserved as a path separator (`C:\Windows`
//!   stays legible), with `^` / `` ` `` / empty quotes still folded;
//! * the **shell-word view** of each — all quotes removed and `$IFS` expanded,
//!   so `d'd'` and `rm${IFS}-rf${IFS}/` read as what the shell will run.
//!
//! Each extra view is emitted only when it differs from what is already there
//! (the native view needs a backslash present; the shell-word views need a
//! *splicing* quote or an `IFS` reference — an ordinary `-m "message"` produces
//! neither), so a plain command still costs one copy and a plainer one costs
//! none at all. Because the views are additive, a reading that turns out to be
//! wrong can only fail to match; it can never mask what another view sees.
//! Rules are single-line (`[^\n]*`) and the join is a `\n`, so the seam cannot
//! manufacture a cross-view match.
//!
//! Deliberately conservative otherwise: it folds exactly the tricks above and
//! nothing semantic. Newlines are preserved — they separate statements and
//! anchor the single-line rules and the head/tail scan seam in
//! [`super::CommandPolicy::evaluate`].
//!
//! # Why not a shell parser
//!
//! codex reaches the same place by parsing the script with `tree-sitter-bash`
//! and matching on argv token sets. Aleph deliberately does not: the flat text
//! already carries every wrapper an AST would have to recurse through (`sudo`,
//! `env A=B`, `timeout 30`, `xargs`, `bash -c '…'`, `nohup … &` are all matched
//! today precisely *because* nothing is scoped to argv), so the AST would buy
//! only the readings this module folds — at the cost of a grammar dependency
//! (R3) and a third shell parser in a repo that already has two
//! ([`crate::exec::parser`] for approval-card rendering,
//! `slash_command::tokenize_with_quotes` for command dispatch). Neither of
//! those two can serve here anyway: both *reject* on the constructs this layer
//! most needs to read (`exec::parser` errors on a background `&`, on backticks
//! and on newlines), and a fail-soft skip is not evidence that a command is
//! safe.

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

/// Ceiling on the whole newline-joined multi-view buffer.
///
/// [`super::CommandPolicy::evaluate`] windows the *input* first, so the input
/// reaching here is already bounded; this bounds the constant factor the views
/// multiply it by (up to four readings plus decoded payloads). Matching is
/// linear in the buffer, so this is a latency bound, not a correctness one —
/// the OS sandbox remains the backstop for anything past it.
const MAX_VIEW_BYTES: usize = 2 * 1024 * 1024;

/// Every spelling of a reference to `$IFS`, the variable whose expansion the
/// shell uses to split words. `rm${IFS}-rf${IFS}/` runs exactly what
/// `rm -rf /` runs, but carries no whitespace for a rule to anchor on.
/// `\b` after `IFS` is what keeps `$IFSX` — a *different* variable — from
/// being read as `$IFS` followed by `X`.
static IFS_REFERENCE_RE: Lazy<Regex> = Lazy::new(|| {
    // rust-doctor-disable-next-line unwrap-in-production
    Regex::new(r"\$\{IFS[^}]*\}|\$IFS\b").expect("IFS reference pattern must compile")
});

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
/// costs only a scan, not an allocation. Otherwise returns every view produced
/// by [`append_views`], newline-joined.
#[must_use]
pub fn normalize_for_matching(text: &str) -> Cow<'_, str> {
    if is_plain(text) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() * 2);
    let mut budget = MAX_DECODED_PAYLOADS;
    append_views(text, 0, &mut budget, &mut out);
    Cow::Owned(out)
}

/// Whether `text` carries none of the tricks any view folds out, so the raw
/// text *is* its own normalisation and can be borrowed.
///
/// Every condition here must correspond to a transform in [`append_views`]:
/// a trick handled there but missing here is silently skipped for the fast
/// path — which is how a normaliser stops normalising without any test going
/// red. `no_transform_is_missing_from_the_fast_path_guard` pins the pairing.
fn is_plain(text: &str) -> bool {
    if text
        .as_bytes()
        .iter()
        .any(|&b| matches!(b, b'\\' | b'\'' | b'"' | b'^' | b'`'))
    {
        return false;
    }
    if may_carry_encoded_payload(text) || text.contains("IFS") {
        return false;
    }
    let (_, removed) = strip_unsafe_invisible(text.as_bytes());
    removed == 0
}

/// Append every matching view of `raw` to `out`, newline-separated.
///
/// This is the **single** normalisation pipeline. Earlier revisions ran it once
/// for the outer command and then a degraded copy — escape folding only — for
/// each decoded `-EncodedCommand` payload, so an encoded script kept the two
/// tricks the degraded copy did not know about: `del /s /q \\?\C:\` (never
/// canonicalised, so no Windows rule could name the drive root) and a
/// zero-width character spliced into `dd` (never stripped). Both reached the
/// catastrophic floor as a `warn`. Recursing through *this* function instead
/// means a payload is normalised exactly as hard as the text that carried it,
/// by construction rather than by two authors agreeing.
///
/// Views emitted, in order:
///
/// 1. **POSIX** — `\` folded as sh's escape (`d\d` → `dd`);
/// 2. **native** — `\` kept as Windows' path separator (`C:\Windows`), emitted
///    only when it differs (i.e. the text really contains a backslash);
/// 3. **shell-word** — each of the above with *all* quotes removed and `$IFS`
///    expanded to a space, emitted only when a quote actually splices a token
///    or an `IFS` reference is present (see [`needs_word_view`]).
///
/// A rule matches if *any* view matches, so no single reading can hide a
/// command from the others. `depth` and `budget` bound the `-EncodedCommand`
/// recursion; [`MAX_VIEW_BYTES`] bounds the buffer.
fn append_views(raw: &str, depth: usize, budget: &mut usize, out: &mut String) {
    if out.len() >= MAX_VIEW_BYTES {
        return;
    }

    // `strip_unsafe_invisible` only removes whole invisible UTF-8 sequences, so
    // the remaining bytes are still valid UTF-8; `from_utf8_lossy` is a
    // defensive no-op that also handles the borrowed (unchanged) case.
    let (stripped_bytes, removed) = strip_unsafe_invisible(raw.as_bytes());
    let stripped: Cow<'_, str> = if removed == 0 {
        Cow::Borrowed(raw)
    } else {
        Cow::Owned(String::from_utf8_lossy(&stripped_bytes).into_owned())
    };
    // Runs before folding: the prefixes are made of backslashes, which the
    // POSIX view is about to consume.
    let canonical = strip_windows_path_prefixes(&stripped);

    let posix = fold_escapes_and_quotes(&canonical, true);
    // The two escape views differ exactly when a backslash was folded.
    let native = if canonical.contains('\\') {
        let n = fold_escapes_and_quotes(&canonical, false);
        (n != posix).then_some(n)
    } else {
        None
    };

    push_view(out, &posix);
    if let Some(n) = &native {
        push_view(out, n);
    }

    if needs_word_view(&canonical) {
        let word = shell_word_fold(&posix);
        let word_differs = word != posix;
        if word_differs {
            push_view(out, &word);
        }
        if let Some(n) = &native {
            let word_native = shell_word_fold(n);
            if word_native != *n && !(word_differs && word_native == word) {
                push_view(out, &word_native);
            }
        }
    }

    // Tested against the *folded* text, not the original: `-e^nc <base64>` only
    // looks like an encoded command after the cmd caret has been folded away.
    if depth >= MAX_DECODE_ROUNDS || *budget == 0 || !may_carry_encoded_payload(&posix) {
        return;
    }
    for caps in ENCODED_COMMAND_RE.captures_iter(&posix) {
        if *budget == 0 || out.len() >= MAX_VIEW_BYTES {
            break;
        }
        let Some(token) = caps.get(1) else {
            continue;
        };
        let Some(decoded) = decode_encoded_command(token.as_str()) else {
            continue;
        };
        *budget -= 1;
        append_views(&decoded, depth + 1, budget, out);
    }
}

/// Append one view, newline-separated from whatever is already there and
/// truncated on a char boundary at [`MAX_VIEW_BYTES`].
fn push_view(out: &mut String, view: &str) {
    if !out.is_empty() {
        out.push('\n');
    }
    let room = MAX_VIEW_BYTES.saturating_sub(out.len());
    if view.len() <= room {
        out.push_str(view);
        return;
    }
    let mut end = room;
    while end > 0 && !view.is_char_boundary(end) {
        end -= 1;
    }
    out.push_str(&view[..end]);
}

/// Whether a shell-word view would say anything the escape views do not.
///
/// True when a quote *splices a token* — a quote with non-whitespace on both
/// sides, which is what `d'd'` / `r"m"` do and what an ordinary `-m "message"`
/// never does — or when the text references `$IFS`, whose expansion supplies
/// the word separator that `rm${IFS}-rf${IFS}/` is missing. Cheap on purpose:
/// the common quoted command produces no extra view at all.
fn needs_word_view(s: &str) -> bool {
    if s.contains("IFS") {
        return true;
    }
    let mut prev: Option<char> = None;
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if matches!(c, '\'' | '"')
            && prev.is_some_and(|p| !p.is_whitespace())
            && it.peek().is_some_and(|n| !n.is_whitespace())
        {
            return true;
        }
        prev = Some(c);
    }
    false
}

/// The reading the shell arrives at after quote removal and `$IFS` expansion:
/// every quote character dropped (not just the empty pairs
/// [`fold_escapes_and_quotes`] collapses) and every `$IFS` reference replaced
/// by a space.
///
/// Emitted as an *extra* view rather than folded into the escape views on
/// purpose: several Windows rules use a quote as the left token boundary of a
/// bare drive root (`win_bare_root!`), so removing quotes everywhere would
/// *lose* matches like `del /s /q'C:\'`. Additive views can only add signal.
fn shell_word_fold(s: &str) -> String {
    let expanded = IFS_REFERENCE_RE.replace_all(s, " ");
    expanded.chars().filter(|c| !matches!(c, '\'' | '"')).collect()
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

    /// The fast path is a second statement of "what this module folds out", and
    /// a transform present in [`append_views`] but missing from [`is_plain`] is
    /// silently skipped for every input that carries only *that* trick — with
    /// no test going red, because every other input still normalises. So the
    /// pairing is asserted directly: each input below carries exactly one trick
    /// and must therefore be recognised as *not* plain.
    #[test]
    fn no_transform_is_missing_from_the_fast_path_guard() {
        for (trick, sample) in [
            ("posix escape", r"r\m -rf /"),
            ("cmd caret", "de^l /s C:"),
            ("powershell backtick", "Remo`ve-Item"),
            ("empty quote pair", "d''d if=x"),
            ("splicing quote", "d'd' if=x"),
            ("windows path prefix", r"del \\?\C:\"),
            ("ifs reference", "rm${IFS}-rf${IFS}/"),
            ("bare ifs reference", "rm$IFS-rf$IFS/"),
            ("invisible character", "d\u{200b}d if=x"),
            ("encoded command", "powershell -enc ZABlAGwAIAAvAHMAIAA="),
        ] {
            assert!(
                !is_plain(sample),
                "{trick}: fast path would skip normalisation for {sample:?}"
            );
        }
        // The converse: an ordinary command must still be borrowed, or the
        // fast path has stopped being fast.
        for plain in [
            "cargo build --release",
            "git status",
            "ls -la /tmp",
            "rm -rf target",
        ] {
            assert!(is_plain(plain), "{plain:?} must take the fast path");
            assert!(
                matches!(normalize_for_matching(plain), Cow::Borrowed(_)),
                "{plain:?} must not allocate"
            );
        }
    }

    /// The shell-word view is emitted only when it would say something new.
    /// Ordinary quoting — a quoted argument, a possessive apostrophe — has a
    /// quote at a token boundary and must not cost a view.
    #[test]
    fn the_shell_word_view_is_only_emitted_when_a_quote_splices_a_token() {
        for spliced in ["d'd'", r#"r"m" -rf /"#, "de'l' /s", "rm${IFS}-rf"] {
            assert!(needs_word_view(spliced), "{spliced:?} splices a token");
        }
        for ordinary in [
            r#"git commit -m "message""#,
            "echo 'hello world'",
            r#"grep -e "pattern" file"#,
            "cargo build",
        ] {
            assert!(
                !needs_word_view(ordinary),
                "{ordinary:?} must not need a word view"
            );
        }
    }

    /// `$IFSX` is a different variable; reading it as `$IFS` followed by `X`
    /// would rewrite a command the shell never splits.
    #[test]
    fn only_a_real_ifs_reference_expands() {
        assert_eq!(shell_word_fold("rm${IFS}-rf${IFS}/"), "rm -rf /");
        assert_eq!(shell_word_fold("rm$IFS-rf$IFS/"), "rm -rf /");
        assert_eq!(shell_word_fold("${IFS:0:1}"), " ");
        assert_eq!(shell_word_fold("echo $IFSX"), "echo $IFSX");
    }

    /// Every view is additive: a decoded payload appends readings, it never
    /// replaces the carrier's own, so a rule that matched the outer text before
    /// still matches it after.
    #[test]
    fn views_are_additive_not_replacing() {
        let out = norm(r"dd if=/dev/zero of=/dev/s\da");
        assert!(
            out.contains("dd if=/dev/zero of=/dev/sda"),
            "posix view present: {out:?}"
        );
        assert!(
            out.contains(r"of=/dev/s\da"),
            "native view present: {out:?}"
        );
    }

    /// The buffer the views multiply into is bounded, and truncation lands on a
    /// char boundary (the slice would panic outright otherwise — which is the
    /// real assertion here, since a panic in a `SandboxBeforeHook` is a denial
    /// of the whole exec path).
    #[test]
    fn the_view_buffer_is_bounded() {
        // Multibyte padding, so a byte-indexed truncation would split a char.
        let payload = "é".repeat(MAX_VIEW_BYTES / 2);
        let text = format!("echo '{payload}' \\ && echo '{payload}'");
        let out = normalize_for_matching(&text);
        // Each view is truncated to the room left before its own separator, so
        // the only overshoot possible is one `\n` per view.
        assert!(
            out.len() <= MAX_VIEW_BYTES + 16,
            "view buffer must stay bounded, got {}",
            out.len()
        );
    }
}
