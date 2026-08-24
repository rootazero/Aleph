//! The production half of a Rust source file, for source-level census guards.
//!
//! # Why this is not `src.split("#[cfg(test)]").next()`
//!
//! That idiom cuts at the *first place a test attribute appears*, which is not
//! a boundary. Measured on this repo (1734 files carrying the attribute):
//! 1458 have one trailing `mod tests {` and are cut correctly; **73** open with
//! `#[cfg(test)] mod tests;` and lose the ENTIRE file; **203** carry a mid-file
//! test item and are truncated arbitrarily. `src/utils/paths.rs` declares a
//! test-only mutex at 5% of the file, so 95% of it was invisible to every
//! guard using the prefix cut — and `src/spend/mod.rs` (the anchor of the
//! §5.22 round-7 capability-handle fix) was cut at byte 2,024 of 30,470.
//!
//! `\r` is dropped first: this repo is checked out CRLF on Windows, where a
//! `\n`-anchored separator matches nothing and the scan silently covers the
//! test module too.

/// The production half of a Rust source file.
///
/// Removes each `#[cfg(test)]`-attributed *item* (by brace matching, or to the
/// terminating `;` for one-line items) and each `#[cfg(test)] mod <name>;`
/// declaration, keeping everything else — including production code that
/// follows a mid-file test item.
///
/// Deliberately orthogonal to [`strip_comment_lines`]: a guard decides for
/// itself whether comments are in scope. Call this function first if you
/// need both — `strip_comment_lines` drops whole physical lines and can
/// discard production code that shares a line with a comment delimiter
/// (e.g. `*/ pub fn x() {}`), so filtering comments after item extraction is
/// the safe order.
///
/// # Known gap (F2, review round 4, unfixed)
///
/// The `#[cfg(test)]` attribute is detected below by a raw
/// `trim_start().starts_with("#[cfg(test)]")` scan over `lines[i]` — not by
/// lexing. So a line whose TEXT begins with `#[cfg(test)]` but which is
/// really string-literal or comment-block payload is read as a live
/// attribute, and [`end_of_item`] is then started on the following line
/// with a fresh [`LexState`] that does not inherit the enclosing
/// string/comment state — which can discard the whole tail of a file. This
/// is the under-see direction (a guard silently approves what it cannot
/// see), and it predates this module: it is not something this round
/// introduced, and the fix is larger than a doc-comment pass — lex the
/// file once, ask the lexer whether the attribute line's first column is
/// live code, and seed `end_of_item`'s `LexState` from that walk rather
/// than defaulting it. Zero reachable instances in `src/` as of
/// 2026-08-24: the only 2 lines whose text starts with `#[cfg(test)]`
/// while actually being string payload are both inside their own file's
/// `#[cfg(test)] mod tests` block, which this scan already excises at the
/// outer attribute before reaching them.
#[must_use]
pub fn production_prefix(src: &str) -> String {
    let normalized = src.replace('\r', "");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    let mut i = 0usize;
    while i < lines.len() {
        if !lines[i].trim_start().starts_with("#[cfg(test)]") {
            out.push(lines[i]);
            i += 1;
            continue;
        }
        // The attribute applies to the next non-blank line's item.
        let mut item = i + 1;
        while item < lines.len() && lines[item].trim().is_empty() {
            item += 1;
        }
        if item >= lines.len() {
            break; // dangling attribute at EOF
        }
        i = end_of_item(&lines, item);
    }
    out.join("\n")
}

/// Index of the first line AFTER the item beginning at `start`.
fn end_of_item(lines: &[&str], start: usize) -> usize {
    let mut depth: i32 = 0;
    let mut opened = false;
    let mut k = start;
    // Carried across every line of this one item scan, so a string literal
    // or block comment that spans physical lines is tracked correctly
    // instead of being reset (and mis-scanned) on each new line.
    let mut state = LexState::default();
    while k < lines.len() {
        let code = code_only(lines[k], &mut state);
        depth += i32::try_from(code.matches('{').count()).unwrap_or(0);
        depth -= i32::try_from(code.matches('}').count()).unwrap_or(0);
        if code.contains('{') {
            opened = true;
        }
        if opened && depth <= 0 {
            return k + 1;
        }
        // One-line item (`mod tests;`, `static X: T = v;`) — no block opened.
        if !opened && code.trim_end().ends_with(';') {
            return k + 1;
        }
        k += 1;
    }
    lines.len()
}

/// Lexer state threaded across every physical line of one [`end_of_item`]
/// scan, so a string literal or block comment that spans multiple lines
/// is tracked correctly instead of resetting on each new line.
///
/// Raw strings (`r#"…"#`, `br#`, `cr#`) ARE lexed as raw. That was once a
/// declared boundary, on the reasoning that braces inside raw strings
/// balance out anyway so a brace-counting item-boundary scanner did not
/// need a full lexer. The reasoning was sound for the caller it was
/// written for and wrong for the second caller that arrived later:
/// [`strip_comment_lines`] reads `in_str` at line start to tell "inside an
/// open string" from "inside a comment", and there a desync does not
/// balance out — it inverts the decision, in the silent-approval
/// direction, for every line until the state resyncs.
///
/// Two payload shapes desynchronised, both common here: an odd number of
/// `"` in the payload (regex character classes like `["\']`,
/// `src/sandbox/command_policy/normalize.rs:142`) and a payload ending in
/// `\` (`raw.strip_prefix(r"\\?\")`, `src/utils/paths.rs:56` — one line,
/// 18 desync runs, 206 comment lines wrongly made visible). Once
/// desynchronised, a `/*` in the payload latched `block_comment_depth`,
/// which only `*/` clears: measured 2026-08-24 that swallowed lines
/// 117→580 of `src/sandbox/command_policy/rules.rs` — production source,
/// the sandbox hardline rule table — and ran to EOF in
/// `src/sandbox/config.rs`.
#[derive(Default)]
struct LexState {
    /// Inside a string literal of any kind: ordinary, byte, C, or raw.
    /// [`strip_comment_lines`] reads this at line start.
    in_str: bool,
    /// `Some(n)` while the open string is a RAW one opened with `n` hashes
    /// (`r"…"` is `Some(0)`, `r#"…"#` is `Some(1)`); `None` when the open
    /// string is ordinary, or when no string is open. Inside a raw string
    /// `\` is not an escape and a bare `"` is not a terminator.
    raw_hashes: Option<usize>,
    /// Block-comment nesting depth; `0` means not inside one. Rust nests
    /// `/* … /* … */ … */`, so this counts delimiters rather than toggling
    /// a bool. A bool cleared on the FIRST `*/`, releasing the lexer while
    /// an outer comment was still open (review round 4, finding F1) — that
    /// leaked comment text into `strip_comment_lines`'s output and could
    /// make `end_of_item` run long and discard production code. Zero live
    /// instances on this repo's `src/` tree as of 2026-08-24 (an
    /// independent nesting-aware scan of all 2,444 files found none); it
    /// was a future hazard, not a present defect.
    block_comment_depth: u32,
}

/// If `chars[i]` is `'` and it opens a genuine char literal, return the
/// number of characters the literal spans (including both quotes).
/// Otherwise — a lifetime, or a malformed/unterminated literal — return
/// `None`; the caller must then treat the `'` as an ordinary character.
///
/// Recognised by grammar, not by an enumerated list of shapes — a list is
/// exactly the defect this function exists to not repeat. Two forms:
///
/// - **Escaped**: `chars[i + 1] == '\\'` (`'\n'`, `'\\'`, `'\''`, `'\u{7B}'`).
///   The escape body cannot contain an unescaped `'`, with one exception:
///   if the character right after the backslash is itself `'`, that is the
///   escaped quote of `'\''`, and the real closing quote is the one after
///   *that*. Otherwise the closing quote is the first `'` found scanning
///   forward from just past the backslash — wherever it lands, including
///   after an arbitrary-length body like `\u{7B}`, whose written form
///   happens to contain a literal `{` and `}` that must never be counted.
/// - **Simple**: `chars[i + 2] == Some('\'')` (`'a'`, `'{'`, `'}'`, `'0'`) —
///   a bare one-character literal, three characters total.
///
/// Anything else starting with `'` is a lifetime (`'a`, `'static`, `'_`) or
/// too malformed to call a literal. A malformed literal with no closing
/// quote on this line returns `None` rather than consuming to end of line —
/// a scanner that runs off the end on bad input is how the original bug
/// (an unmatched lifetime quote swallowing the rest of the line) behaved.
fn char_literal_len(chars: &[char], i: usize) -> Option<usize> {
    debug_assert_eq!(chars.get(i), Some(&'\''));
    if chars.get(i + 1) == Some(&'\\') {
        if chars.get(i + 2) == Some(&'\'') {
            // '\'' — the quote at i+2 is the escaped character itself, not
            // the closing quote; the closing quote is the one after it.
            return (chars.get(i + 3) == Some(&'\'')).then_some(4);
        }
        let mut j = i + 2;
        while j < chars.len() {
            if chars[j] == '\'' {
                return Some(j - i + 1);
            }
            j += 1;
        }
        return None; // unterminated on this line
    }
    if chars.get(i + 2) == Some(&'\'') {
        return Some(3); // 'X'
    }
    None // a lifetime, or too malformed to call a literal
}

/// If the `"` at `quote` opens a RAW string, its hash count (`r"…"` is 0,
/// `r#"…"#` is 1); otherwise `None`.
///
/// Read backwards from the quote — over the run of `#`, then the required
/// `r`, then an optional `b`/`c` byte- or C-string prefix — because by the
/// time the scan reaches the quote those characters have already been
/// emitted as ordinary code. The character before the prefix must not be
/// an identifier character: that is what stops an identifier merely
/// *ending* in `r` from being read as a raw-string opener.
fn raw_string_hashes(chars: &[char], quote: usize) -> Option<usize> {
    debug_assert_eq!(chars.get(quote), Some(&'"'));
    let mut j = quote;
    let mut hashes = 0usize;
    while j > 0 && chars[j - 1] == '#' {
        j -= 1;
        hashes += 1;
    }
    if j == 0 || chars[j - 1] != 'r' {
        return None;
    }
    j -= 1; // the `r`
    if j > 0 && matches!(chars[j - 1], 'b' | 'c') {
        j -= 1; // `br"…"` / `cr"…"` are raw too
    }
    if j > 0 && (chars[j - 1].is_alphanumeric() || chars[j - 1] == '_') {
        return None; // the tail of an identifier, not a prefix
    }
    Some(hashes)
}

/// Whether the `"` at `quote` terminates a raw string opened with `hashes`
/// hashes — i.e. whether exactly that many `#` follow it. A shorter run
/// does NOT close it: the payload of `r##"… r#" …"##` contains one.
fn raw_string_closes(chars: &[char], quote: usize, hashes: usize) -> bool {
    (1..=hashes).all(|k| chars.get(quote + k) == Some(&'#'))
}

/// A line with line-comments, block-comments, and string/char literal
/// *contents* removed, so braces inside them are not counted by
/// [`end_of_item`]. `state` carries `in_str`/`block_comment_depth` across every
/// line of one item scan.
///
/// Char literals are recognised via [`char_literal_len`] and skipped whole,
/// so none of their interior characters — including a `{`/`}` written out
/// in an escape body like `'\u{7B}'` — are ever counted or emitted. A bare
/// `'` that [`char_literal_len`] does not recognise (a lifetime, or a
/// malformed literal) is emitted as an ordinary character and changes no
/// state — it is never treated as entering "inside a char literal".
///
/// Every literal the scan skips leaves a SENTINEL in the output: a single
/// `"` where a string delimiter stood, a single `_` for a whole char
/// literal. Never a brace, so [`end_of_item`]'s counting is unaffected.
/// The sentinel exists because this function has three callers asking two
/// different questions of one walk. For brace counting ([`end_of_item`]),
/// emitting nothing for a literal is exactly right. For "does this line
/// contain any code?" — asked by [`strip_comment_lines`]'s filter (which
/// also consults `entered_in_str`, since a previously-open string counts as
/// code even where nothing is visible on this line) and, more plainly, by
/// `no_module_hand_rolls_the_cfg_test_prefix_cut`'s `!code.trim().is_empty()`
/// — a literal IS code, and emitting nothing made a line whose entire
/// content is a literal indistinguishable from a comment, which on this
/// repo silently hid 9 763 lines of real code from ~36 census guards
/// (measured 2026-08-24). One sentinel answers both questions; forking the
/// lexer would have made a second author for the same walk.
fn code_only(line: &str, state: &mut LexState) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(chars.len());
    let mut idx = 0usize;
    let mut escaped = false;
    while idx < chars.len() {
        let c = chars[idx];
        if state.block_comment_depth > 0 {
            if c == '/' && chars.get(idx + 1) == Some(&'*') {
                // Nested opener: only the matching `*/` may close this one.
                state.block_comment_depth += 1;
                idx += 2;
            } else if c == '*' && chars.get(idx + 1) == Some(&'/') {
                state.block_comment_depth -= 1;
                idx += 2;
            } else {
                idx += 1;
            }
            continue;
        }
        if escaped {
            escaped = false;
            idx += 1;
            continue;
        }
        if state.in_str {
            if let Some(hashes) = state.raw_hashes {
                // Raw: `\` is not an escape, and only a `"` followed by the
                // matching run of `#` terminates.
                if c == '"' && raw_string_closes(&chars, idx, hashes) {
                    state.in_str = false;
                    state.raw_hashes = None;
                    out.push('"'); // sentinel
                    idx += 1 + hashes;
                } else {
                    idx += 1;
                }
                continue;
            }
            match c {
                '\\' => escaped = true,
                '"' => {
                    state.in_str = false;
                    out.push('"'); // sentinel
                }
                _ => {}
            }
            idx += 1;
            continue;
        }
        match c {
            '"' => {
                state.in_str = true;
                state.raw_hashes = raw_string_hashes(&chars, idx);
                out.push('"'); // sentinel
                idx += 1;
            }
            '\'' => {
                if let Some(len) = char_literal_len(&chars, idx) {
                    // Skip the whole literal — its interior is never counted —
                    // but leave a sentinel so a line that is nothing but a char
                    // literal does not read as empty.
                    out.push('_');
                    idx += len;
                } else {
                    out.push(c); // a lifetime (or malformed literal) is ordinary text
                    idx += 1;
                }
            }
            '/' if chars.get(idx + 1) == Some(&'/') => break, // line comment: rest of line is not code
            '/' if chars.get(idx + 1) == Some(&'*') => {
                state.block_comment_depth = 1;
                idx += 2;
            }
            _ => {
                out.push(c);
                idx += 1;
            }
        }
    }
    out
}

/// Drop lines that are comment ONLY: a `//` line, a line consumed entirely
/// by an already-open block comment, or a line that opens and closes one
/// with nothing else on it.
///
/// A scanner judges code; a comment is documentation. A doc comment naming a
/// symbol is not a call site, and an explanatory comment describing a bug is
/// not the bug — this repo has been bitten in both directions.
///
/// Stateful on purpose, reusing `code_only`'s `LexState` — the exact
/// `block_comment_depth` tracking `end_of_item` already carries across lines —
/// rather than pattern-matching one line in isolation. A block-comment
/// continuation line (` * text`) and rustfmt's own leading-binary-operator
/// continuation style (`    * cfg.warning_threshold.clamp(0.0, 1.0)`) have
/// IDENTICAL single-line shape; no stateless per-line predicate can tell
/// them apart, because that is a property of the two shapes, not a gap in
/// any one heuristic. Measured on this repo's `src/` tree, over two
/// generations of that mistake: the bare `starts_with('*')` rule matched
/// **479** lines and **zero** of them were block-comment continuations
/// (460 real code, 19 raw-string payload). The refined stateless predicate
/// that replaced it (`is_block_comment_continuation`, since deleted)
/// narrowed those 479 down to **5** — and was still wrong on every one:
/// four are rustfmt's multiplication continuation and the fifth is a line
/// of CSS inside an `r#"…"#`. Its own "distinguishing fact" (whitespace
/// after `*` means comment) was false on its own measured corpus
/// (`* cfg.warning_threshold` is whitespace-followed and is a
/// multiplication). Knowing "am I inside a `/* */` right now" is the only
/// thing that actually answers the question; a line can only be classified
/// correctly by walking the file, not by looking at it alone.
///
/// A previously-open string counts as code even on a line where nothing is
/// visible outside it: `code_only` excludes string *interior* from its
/// output (correct for its brace-counting caller, where a `{` written
/// inside a string must not count), so a line wholly inside an open string
/// still produces the same empty output a comment line does — the
/// delimiter sentinel appears only on lines that carry a delimiter.
/// Multi-line raw strings (a CSS or JSON payload embedded via `r#"…"#`)
/// make that shape common, not exotic. `state.in_str` at the start of each
/// line is what disambiguates "inside an open string" from "inside a
/// comment"; `code_only` cannot tell the two apart from its output alone
/// on such a line, because it treats both the same way on purpose.
///
/// That makes `in_str` a correctness input here, not a detail of brace
/// counting — which is why [`LexState`] lexes raw strings properly (see
/// its doc). A spurious `in_str == true` at line start does not balance
/// out the way a brace desync does: it KEEPS every line until the state
/// resyncs, feeding comment text to ~36 census guards as if it were code.
/// 750 comment lines across 38 files leaked that way before raw strings
/// were lexed — 1.5x the population of the defect the round before this
/// one was written to fix, in the opposite direction.
#[must_use]
pub fn strip_comment_lines(src: &str) -> String {
    let mut state = LexState::default();
    src.replace('\r', "")
        .lines()
        .filter(|line| {
            let entered_in_str = state.in_str;
            let code = code_only(line, &mut state);
            entered_in_str || line.trim().is_empty() || !code.trim().is_empty()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Walk `root` for `.rs` files, returning `(repo-relative path, contents)`.
///
/// Test-only. Aleph already has 12+ independent copies of this walk in
/// individual census guards; the four guards this round adds share this one
/// instead of minting a 13th. The pre-existing copies are deliberately left
/// alone — refactoring twelve guard files to no behavioural end is churn.
#[cfg(test)]
pub(crate) fn rust_sources_under(root: &std::path::Path) -> Vec<(String, String)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(root, &mut files);
    files
        .into_iter()
        .filter_map(|file| {
            let rel = file
                .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/");
            std::fs::read_to_string(&file).ok().map(|text| (rel, text))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape the old `split("#[cfg(test)]")` handles correctly: one
    /// trailing test module and nothing after it.
    #[test]
    fn trailing_test_module_is_removed() {
        let src = "pub fn a() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n";
        let out = production_prefix(src);
        assert!(out.contains("pub fn a()"));
        assert!(!out.contains("mod tests"));
        assert!(!out.contains("fn t()"));
    }

    /// The 203-file class: a mid-file test item, with production code AFTER
    /// it. The old prefix cut discarded everything from the attribute on.
    #[test]
    fn production_after_a_mid_file_test_item_survives() {
        let src = "pub fn before() {}\n\
                   #[cfg(test)]\n\
                   pub(crate) static GUARD: Mutex<()> = Mutex::new(());\n\
                   pub fn after() {}\n";
        let out = production_prefix(src);
        assert!(out.contains("pub fn before()"));
        assert!(
            out.contains("pub fn after()"),
            "production after a mid-file #[cfg(test)] item must survive; got:\n{out}"
        );
        assert!(!out.contains("GUARD"));
    }

    /// The 73-file class: `#[cfg(test)] mod tests;` at the top of the file.
    /// The old prefix cut discarded the ENTIRE file.
    #[test]
    fn top_of_file_test_module_declaration_does_not_eat_the_file() {
        let src = "#[cfg(test)]\nmod tests;\n\npub fn everything() {}\n";
        let out = production_prefix(src);
        assert!(
            out.contains("pub fn everything()"),
            "a `#[cfg(test)] mod tests;` declaration must not discard the file; got:\n{out}"
        );
        assert!(!out.contains("mod tests;"));
    }

    /// A brace inside a string literal must not be counted, or the item skip
    /// runs off the end and eats the rest of the file.
    #[test]
    fn braces_inside_string_literals_do_not_confuse_the_skip() {
        let src = "#[cfg(test)]\n\
                   mod tests {\n\
                       const S: &str = \"unbalanced { brace\";\n\
                   }\n\
                   pub fn after() {}\n";
        let out = production_prefix(src);
        assert!(out.contains("pub fn after()"), "got:\n{out}");
        assert!(!out.contains("unbalanced"));
    }

    /// Critical: a lifetime (`&'static str`) is a single unmatched `'`. A
    /// scanner that toggles a char-literal flag on any bare `'` gets stuck
    /// "inside a char literal" for the rest of the line, silently dropping
    /// every brace after it — including the one that opens the item's body.
    #[test]
    fn cfg_test_item_containing_a_lifetime_does_not_eat_following_code() {
        let src = r#"#[cfg(test)]
pub(super) fn declared_scopes() -> Vec<&'static str> {
    vec![]
}
pub fn after() {}
"#;
        let out = production_prefix(src);
        assert!(out.contains("pub fn after()"), "got:\n{out}");
        assert!(!out.contains("declared_scopes"));
    }

    /// A brace inside a `/* ... */` block comment must not be counted.
    #[test]
    fn cfg_test_item_containing_a_block_comment_brace_does_not_eat_following_code() {
        let src = r#"#[cfg(test)]
fn t() {
    /* a comment with a { brace inside */
    let x = 1;
}
pub fn after() {}
"#;
        let out = production_prefix(src);
        assert!(out.contains("pub fn after()"), "got:\n{out}");
        assert!(!out.contains("let x = 1"));
    }

    /// A string literal that spans physical lines (an unescaped newline
    /// inside the quotes) must keep its `in_str` state across the line
    /// break, or a `}` on the continuation line is counted as structural.
    ///
    /// The trailing `let also_hidden` line is load-bearing: a scanner that
    /// resets `in_str` every line miscounts the phantom `}` on the
    /// continuation line as the real closing brace of `fn t()`, ending the
    /// item one statement too early and leaking `also_hidden` — still
    /// `#[cfg(test)]`-only code — into the production output. Without this
    /// second statement the false-close and the true close land on the same
    /// line by coincidence and the bug produces no observable difference.
    #[test]
    fn cfg_test_item_containing_a_multiline_string_brace_does_not_eat_following_code() {
        let src = r#"#[cfg(test)]
fn t() {
    let s = "first
} still inside the string";
    let also_hidden = 2;
}
pub fn after() {}
"#;
        let out = production_prefix(src);
        assert!(out.contains("pub fn after()"), "got:\n{out}");
        assert!(!out.contains("still inside the string"));
        assert!(!out.contains("also_hidden"), "got:\n{out}");
    }

    /// The char literals `'{'` and `'}'` must still be recognised (by exact
    /// lookahead) and skipped whole, not miscounted as structural braces.
    #[test]
    fn cfg_test_item_containing_brace_char_literals_does_not_eat_following_code() {
        let src = r#"#[cfg(test)]
fn t() {
    let open = '{';
    let close = '}';
}
pub fn after() {}
"#;
        let out = production_prefix(src);
        assert!(out.contains("pub fn after()"), "got:\n{out}");
        assert!(!out.contains("let open"));
    }

    /// Round-2 defect: `'\u{7B}'` is a Unicode-escape char literal whose
    /// *written* form contains a literal `{` and `}`. A recogniser that
    /// special-cased only the exact three-character forms `'{'`/`'}'` let
    /// both braces leak into the scanned text; they self-balance (net depth
    /// 0), but the line still "contains a `{`", which flips `opened` true
    /// and — combined with depth 0 — closes the item scan right there, even
    /// though the item's REAL opening brace (on a later line) was never
    /// reached. `-> bool` only appears past that real opening brace, so its
    /// absence from the output is what distinguishes "correctly stripped
    /// through the real close" from "prematurely closed on the escape".
    #[test]
    fn cfg_test_item_containing_a_unicode_escape_char_literal_does_not_eat_following_code() {
        let src = "#[cfg(test)]\n\
                   fn matches(buf: [u8; '\\u{7B}' as usize])\n\
                   \x20\x20\x20\x20-> bool\n\
                   {\n\
                   \x20\x20\x20\x20true\n\
                   }\n\
                   pub fn after() {}\n";
        let out = production_prefix(src);
        assert!(out.contains("pub fn after()"), "got:\n{out}");
        assert!(!out.contains("-> bool"), "got:\n{out}");
    }

    /// The escaped-quote literal `'\''` contains no braces of its own; this
    /// pins that the new grammar-based recogniser still consumes all four
    /// of its characters (`'`, `\`, `'`, `'`) as one unit rather than
    /// miscounting any of them individually.
    #[test]
    fn cfg_test_item_containing_an_escaped_quote_char_literal_does_not_eat_following_code() {
        let src = "#[cfg(test)]\n\
                   fn t() {\n\
                   \x20\x20\x20\x20let q = '\\'';\n\
                   }\n\
                   pub fn after() {}\n";
        let out = production_prefix(src);
        assert!(out.contains("pub fn after()"), "got:\n{out}");
        assert!(!out.contains("let q"));
    }

    /// Review round 4, finding 6: this branch is defensive and, until this
    /// test, unpinned — deleting it (mutant `MI`) survived all seven of the
    /// round's mutation-killing tests AND is byte-identical on this repo's
    /// whole `src/` corpus (2,444 files, 0 diffs), because every corpus
    /// instance of `'\''` happens to be followed by a non-`'` character that
    /// the generic scan below also stops on one position early — the
    /// generic scan alone would return `Some(3)` here (closing on the
    /// escaped quote itself, at `i + 2`) instead of the correct `Some(4)`.
    #[test]
    fn char_literal_len_recognises_the_escaped_quote_special_case() {
        let chars: Vec<char> = "'\\''".chars().collect();
        assert_eq!(char_literal_len(&chars, 0), Some(4));
    }

    /// A lifetime and a char literal on the same line must not interfere
    /// with each other: the lifetime changes no state, and the `'{'`
    /// literal is still recognised and its brace still excluded from the
    /// count, so the line's three real brace pairs are counted correctly.
    #[test]
    fn line_with_both_a_lifetime_and_a_char_literal_counts_braces_correctly() {
        let src = "#[cfg(test)]\n\
                   fn f<'a>(c: char) -> &'a str { if c == '{' { \"x\" } else { \"y\" } }\n\
                   pub fn after() {}\n";
        let out = production_prefix(src);
        assert!(out.contains("pub fn after()"), "got:\n{out}");
        assert!(!out.contains("if c =="), "got:\n{out}");
    }

    /// CRLF checkouts are real on Windows; a `\n`-anchored scan matches
    /// nothing there and the guard silently covers the test module too.
    #[test]
    fn crlf_input_is_handled() {
        let src = "pub fn a() {}\r\n#[cfg(test)]\r\nmod tests {\r\n    fn t() {}\r\n}\r\n";
        let out = production_prefix(src);
        assert!(out.contains("pub fn a()"));
        assert!(!out.contains("fn t()"));
    }

    #[test]
    fn strip_comment_lines_drops_line_and_block_comment_lines() {
        // The block comment genuinely stays open into ` * continued` — unlike
        // the fix-round-3 defect this file used to encode, a bare `* text`
        // line is only a comment when the lexer is actually still inside a
        // `/* */` when it reaches that line, not because of what the line
        // looks like on its own.
        let src = "// a doc mention of foo()\npub fn real() {}\n/* block\n * continued\n */\n";
        let out = strip_comment_lines(src);
        assert!(out.contains("pub fn real()"));
        assert!(!out.contains("doc mention"));
        assert!(!out.contains("block"));
        assert!(!out.contains("continued"));
    }

    /// The bare-`*` rule must not eat dereferences and globs. `starts_with('*')`
    /// alone matched 474 real-code lines to every 5 genuine comment
    /// continuations on this repo's own `src/` tree (measured 2026-08-24) —
    /// this pins the shape of the fix, not just one example.
    #[test]
    fn strip_comment_lines_keeps_dereferences_and_globs() {
        let src = "*count += 1;\n*vendor,\n*ref_val = 3;\n*self.captured.lock().unwrap() += 1;\nuse std::io::*;\n";
        let out = strip_comment_lines(src);
        for kept in [
            "*count += 1;",
            "*vendor,",
            "*ref_val = 3;",
            "*self.captured.lock().unwrap() += 1;",
            "use std::io::*;",
        ] {
            assert!(out.contains(kept), "wrongly dropped real code: {kept}");
        }
    }

    /// rustfmt's own style for a wrapped multi-line expression puts the
    /// continuing operator first — a `*` followed by whitespace, the exact
    /// single-line shape a stateless predicate cannot tell apart from a
    /// block-comment continuation. Confirmed RED under the predicate this
    /// replaced (`is_block_comment_continuation`) before the fix landed:
    /// that predicate matched this shape and dropped both lines.
    #[test]
    fn strip_comment_lines_keeps_a_leading_multiplication_continuation() {
        // Deliberately kept on one physical source line: a `\n`-escaped
        // fixture spread across real physical lines would itself start a
        // line with `*`, polluting any census that scans this file's own
        // source text for that exact shape (as this fix's own measurement
        // does — see the doc comment on `strip_comment_lines`).
        let src = "let window_chars = (cfg.token_budget as f64)\n    * cfg.warning_threshold.clamp(0.0, 1.0)\n    * cfg.token_estimate_ratio.max(1.0);\n";
        let out = strip_comment_lines(src);
        for kept in [
            "* cfg.warning_threshold.clamp(0.0, 1.0)",
            "* cfg.token_estimate_ratio.max(1.0);",
        ] {
            assert!(
                out.contains(kept),
                "a leading-multiplication continuation line was wrongly dropped as a comment: {kept}"
            );
        }
    }

    /// A line in the middle of a multi-line raw string produces the same
    /// empty `code_only` output a comment line does — no delimiter stands
    /// on it, so no sentinel is emitted — and only `in_str` at line-start
    /// tells the two apart. Confirmed RED under
    /// `is_block_comment_continuation` before the fix (that predicate never
    /// looked at string state at all, so it matched this line's leading `*`
    /// the same way it matched a comment continuation).
    #[test]
    fn strip_comment_lines_keeps_a_css_line_inside_a_raw_string() {
        // See the sibling test above for why this stays on one physical line.
        let src = "let css = r#\"\n* { margin: 0; padding: 0; box-sizing: border-box; }\n\"#;\n";
        let out = strip_comment_lines(src);
        assert!(
            out.contains("* { margin: 0; padding: 0; box-sizing: border-box; }"),
            "a CSS universal-selector line inside a raw string was wrongly dropped as a comment"
        );
    }

    /// The case statefulness actually buys, as opposed to the case a
    /// stateless predicate got right only by accident (the two tests
    /// above): a genuine block comment spanning several lines. Its
    /// continuation lines and the closing `*/` must still be dropped.
    #[test]
    fn strip_comment_lines_drops_a_genuine_multi_line_block_comment() {
        let src = "/* block\n * a continuation line\n *\n */\npub fn survives() {}\n";
        let out = strip_comment_lines(src);
        assert_eq!(out.trim(), "pub fn survives() {}");
    }

    /// F1 (review round 4, finding 1): Rust nests `/* … /* … */ … */`, but
    /// `LexState`'s block-comment flag used to be a bool that cleared on the
    /// FIRST `*/` — so the outer comment released early and " still outer
    /// */" leaked into `code_only`'s output as ordinary code, making the
    /// whole line read as containing code and survive `strip_comment_lines`.
    /// Zero live instances on this repo's `src/` tree as of 2026-08-24 (an
    /// independent nesting-aware scan of all 2,444 files found none) — this
    /// pins the fix against the shape a future commit could still produce
    /// (e.g. commenting out a function that itself contains `/* */`).
    #[test]
    fn nested_block_comments_are_fully_dropped() {
        let src = "/* outer /* inner */ still outer */\npub fn after() {}\n";
        let out = strip_comment_lines(src);
        assert!(
            !out.contains("still outer"),
            "a nested block comment released on the FIRST `*/` instead of \
             the matching outer one, leaking comment text as code; got:\n{out}"
        );
        assert!(out.contains("pub fn after()"), "got:\n{out}");
    }

    /// M1, the dominant wrong-drop mechanism: a line whose ENTIRE content is
    /// a string literal is CODE, not a comment.
    ///
    /// `code_only`'s documented job for [`end_of_item`] is to remove literal
    /// *contents* so braces inside them are not counted, and emitting nothing
    /// at all is exactly right for brace counting. Reusing that same output as
    /// a proxy for "does this line contain any code?" answers a *different*
    /// question, and for that one a literal IS code — so before the sentinel
    /// landed, such a line rendered empty and was indistinguishable from a
    /// comment. Measured on this repo 2026-08-24: 9 715 lines dropped this
    /// way, 96.3 % of all wrong drops. The blind spot stayed empty only
    /// because rustfmt puts a trailing `,` or `)` on most wrapped argument
    /// lines — an accident of formatting, not an invariant, and nothing
    /// tested it. The shapes that fall through are the last argument of a
    /// wrapped `assert!`/`format!`/`anyhow!` and `\`-continued fragments.
    #[test]
    fn strip_comment_lines_keeps_a_lone_string_literal_line() {
        let src = "assert!(\n    ok,\n    \"the last argument of a wrapped assert carries no trailing punctuation\"\n);\n";
        let out = strip_comment_lines(src);
        assert!(
            out.contains("the last argument of a wrapped assert"),
            "a line whose whole content is a string literal is a token, not a comment — and \
             string literals are exactly the shape this repo's censuses scrape \
             (`count(\"\\\".tx\\\"\")`, `name: \"foo\"`, `register_handler!(\"method\")`); got:\n{out}"
        );
    }

    /// M3, the same contract mismatch on char literals: `char_literal_len`
    /// skips the literal whole, so a line whose entire content is `'_'` or
    /// `' '` — the whole body of a match arm or a wrapped argument — rendered
    /// empty and was dropped. 48 lines on this repo.
    #[test]
    fn strip_comment_lines_keeps_a_lone_char_literal_line() {
        let src = "let under = matches!(\n    c,\n    '_'\n);\nlet space = s.trim_matches(\n    ' '\n);\n";
        let out = strip_comment_lines(src);
        for kept in ["'_'", "' '"] {
            assert!(
                out.contains(kept),
                "a line whose whole content is a char literal was dropped as a comment: {kept}\ngot:\n{out}"
            );
        }
    }

    /// M2/5(a): a raw-string payload carrying an odd number of `"` used to
    /// desynchronise `in_str`, because the lexer honoured ordinary string
    /// rules inside raw payloads. Regex character classes like `["\']` are
    /// this repo's dominant source of that shape
    /// (`src/sandbox/command_policy/normalize.rs:142`,
    /// `src/security/injection_patterns.rs:245`).
    ///
    /// The direction of the failure is the dangerous one: a spurious
    /// `in_str == true` at line start makes `strip_comment_lines` KEEP the
    /// line, so genuine comments are fed to ~36 census guards as if they were
    /// code — attacking precisely the property this function exists for. 750
    /// comment lines across 38 files leaked this way, measured 2026-08-24.
    #[test]
    fn a_raw_string_payload_with_unbalanced_quotes_does_not_desync() {
        let src = "let re = Regex::new(r#\"(?i)[-/]e\\s+[\"\\']?([a-zA-Z0-9]+)\"#);\n// this comment must still be dropped\npub fn after() {}\n";
        let out = strip_comment_lines(src);
        assert!(
            !out.contains("this comment must still be dropped"),
            "an unbalanced `\\\"` inside a raw payload desynchronised `in_str`, so the comment \
             line after it was kept as code; got:\n{out}"
        );
        assert!(out.contains("pub fn after()"), "got:\n{out}");
    }

    /// The second, distinct desync mechanism: a raw string ENDING in a
    /// backslash. The trailing `\` is literal in a raw string, but the old
    /// lexer applied escape rules inside every string, so it consumed the
    /// closing `"` and `in_str` latched true. `src/utils/paths.rs:56`
    /// (`raw.strip_prefix(r"\\?\")`) is the live instance: that one line
    /// produced 18 desync runs and 206 newly-visible comment lines in that
    /// file alone.
    #[test]
    fn a_raw_string_ending_in_a_backslash_does_not_desync() {
        let src = "let Some(rest) = raw.strip_prefix(r\"\\\\?\\\") else {\n// this comment must still be dropped\npub fn after() {}\n";
        let out = strip_comment_lines(src);
        assert!(
            !out.contains("this comment must still be dropped"),
            "a raw string ending in a backslash ate its own closing quote; got:\n{out}"
        );
        assert!(out.contains("pub fn after()"), "got:\n{out}");
    }

    /// M4, the worst of the four and the only one that reached production
    /// source: once `in_str` reads false while still inside a raw payload, a
    /// `/*` in that payload latches `block_comment_depth`, which only `*/`
    /// clears. `src/sandbox/command_policy/rules.rs:117` latches on
    /// `r#"["']?/+(?:\.{1,2}/*)*…"#` and swallows lines 117→580 — 463 lines of
    /// the sandbox hardline rule table, and it SURVIVES `production_prefix`.
    /// `src/sandbox/config.rs:522` latches on a `"**/*.pem"` glob and never
    /// clears, reaching EOF still inside a block comment.
    ///
    /// The `description:` line is the load-bearing assertion: the comment line
    /// is dropped either way (it is a comment), so only a real code line after
    /// the latch distinguishes "the latch is gone" from "the latch swallowed
    /// everything, comment included".
    #[test]
    fn a_block_comment_opener_inside_a_raw_payload_does_not_latch() {
        let src = "let rule = r#\"[\"']?/+(?:\\.{1,2}/*)*\"#;\n// this comment must still be dropped\ndescription: \"recursive remove targeting an absolute root\",\n";
        let out = strip_comment_lines(src);
        assert!(
            out.contains("recursive remove targeting an absolute root"),
            "a `/*` inside a raw payload latched a block comment over the production code \
             after it — this is the sandbox hardline rule table's shape; got:\n{out}"
        );
        assert!(
            !out.contains("this comment must still be dropped"),
            "got:\n{out}"
        );
    }

    /// A raw string closes only on its own hash count, and the `b`/`c` byte
    /// and C-string prefixes are raw too. A closer that ignored the hash run
    /// would end the string at the payload's own `r#"`, leaving the lexer
    /// inside a phantom ordinary string at end of line — the same
    /// keep-the-comment failure as the tests above, reached a different way.
    #[test]
    fn a_raw_string_closes_only_on_its_own_hash_count() {
        for prefix in ["r##", "br##", "cr##"] {
            let src = format!(
                "let s = {prefix}\"payload with a lone r#\" inside\"##;\n// this comment must still be dropped\npub fn after() {{}}\n"
            );
            let out = strip_comment_lines(&src);
            assert!(
                !out.contains("this comment must still be dropped"),
                "{prefix} closed on a shorter hash run; got:\n{out}"
            );
            assert!(out.contains("pub fn after()"), "{prefix}; got:\n{out}");
        }
    }

    /// Review round 4, finding 6: this branch is defensive and, until this
    /// test, unpinned — deleting it (mutant `MH`) survived all seven of the
    /// round's mutation-killing tests AND is byte-identical on this repo's
    /// whole `src/` corpus (2,444 files, 0 diffs). Without it, `foor"x"` —
    /// an identifier ending in `r` immediately followed by a string — would
    /// be misread as `foo` plus a zero-hash raw-string opener (`Some(0)`)
    /// instead of `None`, an ordinary string preceded by an identifier.
    #[test]
    fn raw_string_hashes_rejects_a_quote_preceded_by_an_identifier_tail() {
        let chars: Vec<char> = "foor\"x\"".chars().collect();
        let quote = chars.iter().position(|&c| c == '"').unwrap();
        assert_eq!(raw_string_hashes(&chars, quote), None);
    }

    /// The sentinel must not leak into what a consumer reads.
    /// `strip_comment_lines` returns the ORIGINAL line text — `code_only`'s
    /// output feeds only the keep/drop decision — so no `"` or `_` invented by
    /// the lexer may appear in the result.
    #[test]
    fn strip_comment_lines_returns_original_line_text_not_lexer_output() {
        let src = "let a = \"x\";\nlet b = '_';\nlet c = r#\"raw\"#;\n";
        let out = strip_comment_lines(src);
        assert_eq!(out.trim(), src.trim(), "got:\n{out}");
    }

    /// The shared walker used by guards this round: sanity-checks it can
    /// find this crate's own source tree.
    #[test]
    fn rust_sources_under_finds_the_crate_source_tree() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let files = rust_sources_under(&root);
        assert!(
            files.len() > 100,
            "expected more than 100 .rs files under src/, got {}",
            files.len()
        );
        for (rel, _) in &files {
            assert!(
                rel.starts_with("src/"),
                "expected repo-relative path starting with src/, got {rel}"
            );
        }
    }

    /// Every `.rs` file under `src/`, as `(repo-relative path, text)` — shared
    /// by the guards below via [`rust_sources_under`], rather than minting a
    /// second directory walk in this module.
    fn all_sources() -> Vec<(String, String)> {
        rust_sources_under(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"))
    }

    fn old_prefix_cut(src: &str) -> String {
        let src = src.replace('\r', "");
        src.split("#[cfg(test)]").next().unwrap_or(&src).to_string()
    }

    /// Guard 1 — no regression. Where the old cut was right, we agree with it.
    ///
    /// "Agree" is checked on the retained *code*, not byte-for-byte text: the
    /// old cut keeps the blank lines and the attribute's leading whitespace
    /// that preceded the test module, which carry no meaning to any scanner.
    #[test]
    fn production_prefix_agrees_with_the_old_cut_where_the_old_cut_was_right() {
        let mut compared = 0usize;
        for (rel, text) in all_sources() {
            let old = old_prefix_cut(&text);
            // "old cut was right" == nothing but whitespace follows the test
            // module, i.e. the new extractor found no extra code.
            let new = production_prefix(&text);
            if new.split_whitespace().eq(old.split_whitespace()) {
                compared += 1;
                continue;
            }
            assert!(
                new.len() >= old.trim_end().len(),
                "{rel}: new extraction is SHORTER than the old prefix cut — the \
                 extractor is dropping production code"
            );
        }
        assert!(
            compared > 1_000,
            "expected >1000 files where old and new agree, saw {compared} — either the \
             extractor regressed or the corpus changed shape; investigate, do not relax"
        );
    }

    /// Guard 2 — real expansion. The ~209-file class must actually recover code.
    ///
    /// The count is asserted because a shrinking census and a broken census
    /// look identical in a passing report. The floor was 213, measured
    /// directly against the shipped `production_prefix` on 2026-08-24 — NOT
    /// the 276 first quoted for this class while this guard was being
    /// planned. That 276 was measured against a pre-fix build of the
    /// extractor whose `end_of_item` returned early and so over-kept
    /// trailing test lines, which were then double-counted as "recovered
    /// production code". Fixing that over-keep necessarily moved that
    /// number DOWN from 276; a number that had gone UP would have been the
    /// alarming one.
    ///
    /// The floor moved again, to 209, the same day: the plan round that
    /// migrated 35 hand-rolled `src.split("#[cfg(test)]")` call sites onto
    /// `production_prefix` deleted the literal string `"#[cfg(test)]"` from
    /// 4 of those sites' *production* code (bodies not under their own
    /// file's `#[cfg(test)]` — a local `production_prefix`/`production_source`
    /// helper, mostly). Those 4 files' old bodies held that literal
    /// *earlier* in the file than the file's own real `#[cfg(test)]`
    /// boundary, which is exactly the shape that fools `old_prefix_cut`
    /// below (a bare, unanchored whole-text match with no syntax awareness)
    /// into truncating far too early — so those 4 files counted toward
    /// "recovered" for the wrong reason: not a genuine mid-file test item,
    /// but the guard's own comparison baseline being fooled by the very
    /// text this guard searches for. Removing that literal text made all 4
    /// files' naive cut and canonical cut agree for the first time, so they
    /// stopped counting: 213 − 4 = 209. Re-measured directly against the
    /// shipped extractor post-migration (instrumented print, run, reverted);
    /// not a Python transliteration, which — lacking `code_only` and
    /// `char_literal_len` — measured a different, wrong number on the same
    /// tree.
    ///
    /// The floor moved a third time, to 193, on 2026-08-24 — for the SAME
    /// reason as the 276 → 213 move: the number that came down had been
    /// counting leaked TEST code as recovered production code. Fix round 4
    /// taught [`LexState`] to lex raw strings, so a JSON or TOML payload
    /// written `r#"{ … }"#` inside a `#[cfg(test)]` item no longer feeds its
    /// braces to `end_of_item`'s depth. Before that, those braces
    /// desynchronised the scan, it returned EARLY, and the tail of the test
    /// module stayed in the "production" output — which then measured longer
    /// than the naive cut and counted as recovery. 16 files were in that
    /// state. Checked tree-wide across all 34 files whose output moved:
    /// 3 065 non-blank lines removed, every one of them at or after its own
    /// file's `#[cfg(test)]` attribute and inside a `#[cfg(test)]` item by an
    /// independent formatting oracle; 76 lines of genuine production code
    /// RECOVERED in `src/hub/install.rs` (a line-multiset diff of pre-vs-post
    /// output: 332 removed, 76 added, 75 non-blank), where the desync had
    /// instead run the scan PAST the item's true end; zero production lines
    /// lost. The companion guard above moved the other way in step —
    /// `compared` 2 235 → 2 251, i.e. those 16 files now AGREE with the naive
    /// cut instead of beating it — and `worst` is unchanged at 62 222 bytes.
    ///
    /// If this floor drops again, the first question is the same one these
    /// paragraphs answer: did the corpus's own `"#[cfg(test)]"`-shaped text
    /// change, did the extractor stop recognising a shape, or did it stop
    /// MIS-recognising one? Only the second is alarming — but the third has
    /// now fired twice, so "the number went down" is never by itself the
    /// answer.
    #[test]
    fn production_prefix_recovers_code_the_old_cut_discarded() {
        let mut recovered = 0usize;
        let mut worst = (0usize, String::new());
        for (rel, text) in all_sources() {
            let old = old_prefix_cut(&text).trim_end().len();
            let new = production_prefix(&text).trim_end().len();
            if new > old {
                recovered += 1;
                if new - old > worst.0 {
                    worst = (new - old, rel);
                }
            }
        }
        assert!(
            recovered >= 193,
            "expected >=193 files to recover production code (measured 193 against the \
             shipped extractor on 2026-08-24, after raw strings were lexed as raw; see \
             the doc comment above for why this moved down from 209, and before that from \
             213 and 276); saw {recovered}. A further drop means the extractor stopped \
             recognising a shape — investigate before lowering this floor, and note that \
             every previous drop was the opposite: it stopped MIS-recognising one, and \
             the falling number was leaked test code being counted as recovered \
             production code."
        );
        assert!(worst.0 > 10_000, "worst-case recovery {worst:?} is implausibly small");
    }

    /// Guard 3 — no second author. The rule, not an exemption list.
    ///
    /// Scans WHOLE files on purpose — not [`production_prefix`]. 25 of the 32
    /// offending files hold their occurrence inside a same-file
    /// `#[cfg(test)] mod tests { .. }` block, because census guards are
    /// themselves tests. Routing this guard through `production_prefix`
    /// would strip that block before the scan ever saw it, making this
    /// guard blind to 25 of the 32 files it exists to police. If you are
    /// here to make this module internally consistent: this asymmetry is
    /// the point.
    ///
    /// Walks `tests/` in addition to `src/` — `production_prefix` is `pub`
    /// and reachable from this crate's own integration tests, so a hand-roll
    /// there is the identical defect this guard exists to police, not a
    /// different one. `rust_sources_under` reports paths relative to
    /// `CARGO_MANIFEST_DIR` regardless of which root it is walking, so a
    /// `tests/` file renders as `tests/…` and a `src/` file as `src/…` — the
    /// self-exemption below matches on the full `src/…` string and is
    /// unaffected by the second root.
    ///
    /// Line numbers are computed against the RAW file, not a pre-stripped
    /// one: comment lines are skipped inline, one line at a time, rather
    /// than by pre-filtering the text through [`strip_comment_lines`] and
    /// then re-numbering the result — the latter reports line numbers that
    /// exist only in the stripped text, not in the file anyone would open.
    ///
    /// Skipping inline is not licence to re-answer "is this line a comment"
    /// locally. This guard threads a [`LexState`] down the same walk and
    /// asks [`code_only`] — the one recogniser — so keeping raw line
    /// numbers costs no second author. It used to hand-roll
    /// `starts_with("//") || starts_with("/*") || starts_with('*')`: the
    /// *pre-narrowing* bare-`*` rule, standing in the file whose entire
    /// subject is that this question cannot be answered by looking at a
    /// line alone. That third disjunct by itself skipped 496 lines across
    /// 235 files on this repo, every one of them real code, in the
    /// direction where the guard silently approves.
    ///
    /// The needle search still runs on the RAW line, never on `code_only`'s
    /// output: the patterns this guard hunts ARE string literals
    /// (`split("#[cfg(test)]")`), and `code_only` removes literal contents
    /// by design. `code_only` decides only whether the line is code at all.
    #[test]
    fn no_module_hand_rolls_the_cfg_test_prefix_cut() {
        let tests_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
        let mut offenders = Vec::new();
        for (rel, text) in all_sources()
            .into_iter()
            .chain(rust_sources_under(&tests_root))
        {
            if rel == "src/utils/source_scan.rs" {
                continue; // defines the replacement and tests the old shape
            }
            let mut state = LexState::default();
            for (n, line) in text.lines().enumerate() {
                if code_only(line, &mut state).trim().is_empty() {
                    continue;
                }
                let t = line.trim_start();
                if t.contains(r##"split("#[cfg(test)]")"##)
                    || t.contains(r##"find("#[cfg(test)]")"##)
                    || t.contains(r##"split_once("#[cfg(test)]")"##)
                {
                    offenders.push(format!("{rel}:{}", n + 1));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "these hand-roll the production-prefix cut instead of calling \
             `utils::source_scan::production_prefix`:\n  {}",
            offenders.join("\n  ")
        );
    }
}
