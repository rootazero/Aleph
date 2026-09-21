//! G1 — the registration-returns-a-Disposer census (spec §3.5).
//!
//! Source-level, because at runtime a registration that bypassed the scope
//! looks exactly like one that went through it: both mutate shared state,
//! and only the next unmount would tell — silently.

use std::path::{Path, PathBuf};

/// Files that own the six effect producers. A new producer goes in one of
/// these, or this list grows in the same commit.
const CENSUS_FILES: [&str; 5] = [
    "src/extension/registrar/api.rs",
    "src/extension/registrar/mcp_registrar.rs",
    "src/extension/loader.rs",
    "src/extension/service_ops.rs",
    "src/extension/slash_effect.rs",
];

/// Name prefixes that mean "this writes something into the runtime".
const EFFECT_VERBS: [&str; 5] = ["register_", "load_", "start_", "add_", "mount_"];

/// (file, fn, why) — a crate-visible effect verb that legitimately does not
/// return a `Disposer`. Every entry must still exist, or the census is red.
const EXEMPT: [(&str, &str, &str); 1] = [(
    "src/extension/service_ops.rs",
    "start_service",
    "operator verb behind `services.start`; the plugin's `service` step disposer \
     stops every registered service of the plugin, so a manual start never \
     outlives the mount",
)];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Drops every `//…` line comment (doc comments included — `///` and `//!`
/// both start with `//`) and every `/* … */` block comment, wherever they
/// occur — not only when a comment is the whole line. A single-pass scan
/// that also steps over string/char literals (so a `//` inside a URL
/// string, or a `{`/`}` inside a comment's own prose, is not misread as a
/// comment start or a real brace, respectively), because comments and
/// literals are indistinguishable from plain text without that lexing: a
/// trailing comment on an otherwise-live line — `continue; // \`mod tests
/// {\` is inline` — desyncs the brace-depth walk below exactly like a full
/// comment line would.
///
/// A comment's own newlines are kept as blank lines (a multi-line `/* … */`
/// does not collapse the lines after it upward), and any code before or
/// after a comment on the same line survives — so `text.lines().enumerate()`
/// on the result still yields the source file's real line numbers, which
/// `production_text` and its callers rely on.
fn strip_line_comments(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut pos = 0usize;
    while pos < chars.len() {
        let Some(end) = skip_non_code(&chars, pos) else {
            out.push(chars[pos]);
            pos += 1;
            continue;
        };
        if chars[pos] == '/' {
            // A comment: drop its text, keeping only the newlines it spans
            // (a `//` comment spans none; a `/* … */` block comment may).
            out.extend(chars[pos..end].iter().filter(|&&c| c == '\n'));
        } else {
            // A string or char literal: kept verbatim.
            out.extend(&chars[pos..end]);
        }
        pos = end;
    }
    out
}

/// From `chars[pos]`, the index just past a `"…"` string, a raw/byte
/// string, a char literal, a `//…` line comment (up to but not including
/// the newline that ends it), or a `/* … */` block comment — or `None` if
/// `chars[pos]` does not start any of those (an ordinary character, a
/// lifetime tick, or a `/` that is division/a path separator, not a
/// comment).
fn skip_non_code(chars: &[char], pos: usize) -> Option<usize> {
    match chars[pos] {
        '"' => Some(skip_quoted_string(chars, pos)),
        '\'' => {
            let end = skip_char_literal(chars, pos);
            (end > pos + 1).then_some(end)
        }
        'r' | 'b' => skip_prefixed_string(chars, pos),
        '/' if chars.get(pos + 1) == Some(&'/') => {
            let mut p = pos;
            while p < chars.len() && chars[p] != '\n' {
                p += 1;
            }
            Some(p)
        }
        '/' if chars.get(pos + 1) == Some(&'*') => Some(skip_block_comment(chars, pos)),
        _ => None,
    }
}

/// `chars[pos..]` opens with `/*`. Rust block comments nest (`/* outer /*
/// inner */ still outer */` is one comment), unlike C, so this tracks
/// depth rather than stopping at the first `*/`. Returns the index just
/// past the outermost comment's closing `*/` (or EOF for an unterminated
/// one — malformed input the compiler would reject anyway).
fn skip_block_comment(chars: &[char], pos: usize) -> usize {
    let mut depth = 0i32;
    let mut p = pos;
    while p < chars.len() {
        if chars[p] == '/' && chars.get(p + 1) == Some(&'*') {
            depth += 1;
            p += 2;
        } else if chars[p] == '*' && chars.get(p + 1) == Some(&'/') {
            depth -= 1;
            p += 2;
            if depth == 0 {
                return p;
            }
        } else {
            p += 1;
        }
    }
    p
}

/// True if `line`'s trimmed start opens a `mod <name> {` item — optionally
/// `pub` / `pub(crate)` / `pub(super)` prefixed — the shape a `#[cfg(test)]`
/// attribute pairs with to mark an *inline* test module. `mod tests;` (a
/// file-module declaration, e.g. `#[cfg(test)] mod tests;` backed by
/// `tests.rs`) is deliberately excluded: its body lives in another file, so
/// there is nothing inline here to blank, and without this check the
/// forward `{`-search in `production_text` would run past it into some
/// unrelated later brace. Requiring the `{` on this same line matches how
/// rustfmt always renders `mod tests {`.
fn starts_test_mod(line: &str) -> bool {
    let t = line.trim_start();
    let after_vis = ["pub(crate) ", "pub(super) ", "pub "]
        .iter()
        .find_map(|p| t.strip_prefix(p))
        .unwrap_or(t);
    after_vis.starts_with("mod ") && t.trim_end().ends_with('{')
}

/// Advances past a `"…"` string body starting right after its opening `"`
/// (`pos` indexes that opening quote), honoring `\` escapes. Returns the
/// index just past the closing `"` (or past EOF for an unterminated
/// string — malformed input the compiler would reject anyway; this scanner
/// only needs to not desync on it).
fn skip_quoted_string(chars: &[char], pos: usize) -> usize {
    let mut p = pos + 1;
    while p < chars.len() {
        match chars[p] {
            '\\' => p += 2,
            '"' => return p + 1,
            _ => p += 1,
        }
    }
    p
}

/// Advances past a raw string body — `chars[quote_pos]` is its opening `"`,
/// already past the `r`/`br` and `hashes` `#` characters. The closer is `"`
/// followed by the same number of `#`; raw strings have no `\` escapes, so
/// nothing inside (a `{`/`}` in a raw JSON fixture, for instance) is
/// special except that exact closing sequence.
fn skip_raw_string(chars: &[char], quote_pos: usize, hashes: usize) -> usize {
    let mut p = quote_pos + 1;
    while p < chars.len() {
        if chars[p] == '"' && (1..=hashes).all(|h| chars.get(p + h) == Some(&'#')) {
            return p + 1 + hashes;
        }
        p += 1;
    }
    p
}

/// `chars[pos]` is `'r'` or `'b'`. If it actually opens a (possibly raw,
/// possibly byte-) string literal — `r"…"`, `r#"…"#`, `b"…"`, `br#"…"#`,
/// … — returns the index just past it. Otherwise `chars[pos]` was just an
/// ordinary identifier character (e.g. the `r` in `register_`, the `b` in
/// `bool`) and the caller should advance by exactly one.
fn skip_prefixed_string(chars: &[char], pos: usize) -> Option<usize> {
    let mut k = pos;
    if chars.get(k) == Some(&'b') {
        k += 1;
    }
    if chars.get(k) != Some(&'r') {
        return None;
    }
    k += 1;
    let mut hashes = 0usize;
    while chars.get(k) == Some(&'#') {
        hashes += 1;
        k += 1;
    }
    if chars.get(k) == Some(&'"') {
        Some(skip_raw_string(chars, k, hashes))
    } else {
        None
    }
}

/// `chars[pos]` is `'`. A char literal is unambiguous once decoded — unlike
/// a lifetime tick (`'a`, `'static`), it is immediately followed by exactly
/// one (possibly escaped) character and a closing `'`. Returns the index
/// just past that closing `'`, or `pos + 1` if this `'` did not open one
/// (a lifetime, or a bare `'` — leave it as an ordinary character, matching
/// how a lifetime tick is otherwise a no-op for brace counting).
///
/// Handles the escape shapes that can themselves contain `{`/`}` —
/// `'\u{7B}'` — plus the single-char escapes (`'\n'`, `'\\'`, `'\''`, …)
/// and `'\xNN'`, so none of their contents are mistaken for real braces.
fn skip_char_literal(chars: &[char], pos: usize) -> usize {
    let Some(&next) = chars.get(pos + 1) else {
        return pos + 1;
    };
    if next != '\\' {
        // A plain single character is a literal only if it is immediately
        // closed; otherwise this `'` is a lifetime tick.
        return if chars.get(pos + 2) == Some(&'\'') {
            pos + 3
        } else {
            pos + 1
        };
    }
    // An escape sequence.
    match chars.get(pos + 2) {
        Some('u') if chars.get(pos + 3) == Some(&'{') => {
            let mut p = pos + 4;
            while chars.get(p).is_some_and(|c| *c != '}') {
                p += 1;
            }
            if chars.get(p) == Some(&'}') {
                p += 1;
            }
            if chars.get(p) == Some(&'\'') {
                p + 1
            } else {
                pos + 1 // malformed — do not special-case
            }
        }
        Some('x') if chars.get(pos + 5) == Some(&'\'') => pos + 6,
        Some(_) if chars.get(pos + 3) == Some(&'\'') => pos + 4, // \n \t \\ \' \" \0 …
        _ => pos + 1,
    }
}

/// From `chars[start]` (the opening `{` of a `mod … {` block) onward,
/// returns the index just past its matching `}` — depth-counted across the
/// rest of the file, stepping over string and char literals (raw, byte,
/// and escaped forms included) so their contents — a raw JSON fixture, a
/// `'{'` trim-char, a `'\u{7B}'` escape — cannot desync the count. Not a
/// full lexer (no handling of e.g. nested doc-comment edge cases, but
/// comments are already blanked before this runs).
fn matching_close_index(chars: &[char], start: usize) -> Option<usize> {
    let mut depth: i32 = 0;
    let mut seen_open = false;
    let mut pos = start;
    while pos < chars.len() {
        match chars[pos] {
            '{' => {
                depth += 1;
                seen_open = true;
                pos += 1;
            }
            '}' => {
                depth -= 1;
                pos += 1;
                if seen_open && depth == 0 {
                    return Some(pos);
                }
            }
            '"' => pos = skip_quoted_string(chars, pos),
            '\'' => pos = skip_char_literal(chars, pos),
            'r' | 'b' => pos = skip_prefixed_string(chars, pos).unwrap_or(pos + 1),
            _ => pos += 1,
        }
    }
    None
}

/// The non-test, non-comment text of a source file: comment lines and every
/// `#[cfg(test)]`-attributed `mod … { … }` block are blanked to empty lines
/// (not deleted, not merely cut-at-first-marker), so a caller keying results
/// off `text.lines().enumerate()` still gets the file's real line numbers,
/// and a second test module or trailing `pub use` after the first one is
/// still scanned rather than silently skipped.
///
/// A registration placed after `mod tests` in one of the census files is
/// therefore production text this function does see — the census's own
/// scan is the guard against that shape, not a separate last-item check.
pub(crate) fn production_text(path: &Path) -> String {
    let src = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let stripped = strip_line_comments(&src);
    let mut lines: Vec<String> = stripped.lines().map(str::to_string).collect();

    // The brace walk below needs one continuous character stream (a string
    // or raw string can span a line break), so it runs over `chars`
    // directly rather than per-line. `line_of[k]` / `line_start_char[j]`
    // translate between that char-index space and the line-index space
    // `lines` is blanked in.
    let chars: Vec<char> = stripped.chars().collect();
    let mut line_of: Vec<usize> = Vec::with_capacity(chars.len());
    let mut line_start_char: Vec<usize> = vec![0];
    {
        let mut line = 0usize;
        for &c in &chars {
            line_of.push(line);
            if c == '\n' {
                line += 1;
                line_start_char.push(line_of.len());
            }
        }
    }

    let mut i = 0usize;
    while i < lines.len() {
        if lines[i].trim() != "#[cfg(test)]" {
            i += 1;
            continue;
        }
        // The next non-blank line must open a `mod <name> {`, or this
        // `#[cfg(test)]` attaches to something else (a fn, a const, …)
        // this scan does not need to touch.
        let mut j = i + 1;
        while j < lines.len() && lines[j].trim().is_empty() {
            j += 1;
        }
        if j >= lines.len() || !starts_test_mod(&lines[j]) {
            i += 1;
            continue;
        }
        let line_start = line_start_char[j];
        let Some(open_pos) = (line_start..chars.len()).find(|&k| chars[k] == '{') else {
            panic!(
                "{}: found `#[cfg(test)]` / `{}` with no opening brace",
                path.display(),
                lines[j].trim()
            );
        };
        let Some(close_pos) = matching_close_index(&chars, open_pos) else {
            panic!(
                "{}: found `#[cfg(test)]` / `{}` with no matching close brace",
                path.display(),
                lines[j].trim()
            );
        };
        let end_line = line_of[close_pos - 1];
        for line in &mut lines[i..=end_line] {
            line.clear();
        }
        i = end_line + 1;
    }

    lines.join("\n")
}

/// Every crate-visible `fn` declaration in `text` whose name starts with an
/// effect verb, with its signature (declaration up to the opening brace).
fn crate_visible_effect_fns(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let t = line.trim_start();
        let vis_ok = t.starts_with("pub fn ")
            || t.starts_with("pub async fn ")
            || t.starts_with("pub(crate) fn ")
            || t.starts_with("pub(crate) async fn ");
        if !vis_ok {
            continue;
        }
        let after_fn = &t[t.find("fn ").unwrap() + 3..];
        let name: String = after_fn
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !EFFECT_VERBS.iter().any(|v| name.starts_with(v)) {
            continue;
        }
        // Signature may span lines: read until the body's `{`.
        let mut sig = t.to_string();
        while !sig.contains('{') {
            match lines.next() {
                Some(l) => {
                    sig.push(' ');
                    sig.push_str(l.trim());
                }
                None => break,
            }
        }
        out.push((name, sig));
    }
    out
}

#[test]
fn every_crate_visible_registration_returns_a_disposer() {
    let root = repo_root();
    let mut offenders: Vec<String> = Vec::new();
    let mut disposer_returning = 0usize;
    let mut exempt_seen: Vec<(&str, &str)> = Vec::new();

    for rel in CENSUS_FILES {
        let text = production_text(&root.join(rel));
        for (name, sig) in crate_visible_effect_fns(&text) {
            if let Some((f, n, _)) = EXEMPT.iter().find(|(f, n, _)| *f == rel && *n == name) {
                exempt_seen.push((f, n));
                continue;
            }
            let returns_disposer = sig
                .split("->")
                .nth(1)
                .is_some_and(|ret| ret.contains("Disposer"));
            if returns_disposer {
                disposer_returning += 1;
            } else {
                offenders.push(format!("{rel}::{name} — `{}`", sig.trim()));
            }
        }
    }

    // Self-count: six producers exist today; a scanner that finds fewer is
    // not reading what it thinks it is.
    assert!(
        disposer_returning >= 6,
        "census found only {disposer_returning} Disposer-returning registration fns — \
         the scanner is blind (six producers are known to exist)"
    );
    for (f, n, _) in EXEMPT {
        assert!(
            exempt_seen.contains(&(f, n)),
            "stale exemption: {f}::{n} no longer exists — remove it from EXEMPT"
        );
    }
    assert!(
        offenders.is_empty(),
        "crate-visible registration fns that do not return a Disposer (spec §3.1: every \
         effect is owned by the plugin's EffectScope; either return `Disposer`, make it \
         `pub(super)` so it is an inner write path of one that does, or add an EXEMPT \
         entry with a reason):\n  {}",
        offenders.join("\n  ")
    );
}

/// The memory registry's plugin path has exactly one caller, in `loader.rs`
/// (`register_memory_extension_effect`). A second caller would be a
/// `[memory]` registration outside the scope.
#[test]
fn register_mcp_has_exactly_one_production_caller_and_it_is_the_effect() {
    let root = repo_root().join("src");
    let mut hits: Vec<String> = Vec::new();
    // This function's own assert_eq! message below contains the literal
    // `.register_mcp(` substring it is scanning for, so the walk necessarily
    // matches this file too; route that self-match into a self-count
    // instead of `hits` — same shape as `projection.rs`'s
    // `is_this_file`/`found_here` (keyed on the path suffix, not the bare
    // file name, since `src/capability/census.rs` also exists and must stay
    // in the scan).
    let mut found_here = 0usize;
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let rel = path.strip_prefix(&root).unwrap().display().to_string();
            let is_this_file = rel == "extension/effects/census.rs";
            let text = production_text(&path);
            for (i, line) in text.lines().enumerate() {
                if line.contains(".register_mcp(") && !line.contains("fn ") {
                    if is_this_file {
                        found_here += 1;
                    } else {
                        hits.push(format!("{rel}:{}", i + 1));
                    }
                }
            }
        }
    }
    assert!(
        found_here >= 1,
        "the scanner did not see its own assertion string in \
         extension/effects/census.rs — it is not reading the tree it thinks it is"
    );
    assert_eq!(
        hits.len(),
        1,
        "`.register_mcp(` must have exactly one production caller (the memory_extension \
         effect); found: {hits:?}"
    );
    assert!(
        hits[0].starts_with("extension/loader.rs:"),
        "the one caller must be register_memory_extension_effect in loader.rs, not {}",
        hits[0]
    );
}
