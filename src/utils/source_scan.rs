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
/// Declared boundary, not a bug: raw strings (`r#"…"#`) are not lexed as
/// raw — their interior `"` characters toggle `in_str` like any other
/// string. In practice braces inside raw strings still balance out, and a
/// full Rust lexer is more machinery than a brace-counting item-boundary
/// scanner needs.
#[derive(Default)]
struct LexState {
    in_str: bool,
    in_block_comment: bool,
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

/// A line with line-comments, block-comments, and string/char literal
/// *contents* removed, so braces inside them are not counted by
/// [`end_of_item`]. `state` carries `in_str`/`in_block_comment` across every
/// line of one item scan.
///
/// Char literals are recognised via [`char_literal_len`] and skipped whole,
/// so none of their interior characters — including a `{`/`}` written out
/// in an escape body like `'\u{7B}'` — are ever counted or emitted. A bare
/// `'` that [`char_literal_len`] does not recognise (a lifetime, or a
/// malformed literal) is emitted as an ordinary character and changes no
/// state — it is never treated as entering "inside a char literal".
fn code_only(line: &str, state: &mut LexState) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(chars.len());
    let mut idx = 0usize;
    let mut escaped = false;
    while idx < chars.len() {
        let c = chars[idx];
        if state.in_block_comment {
            if c == '*' && chars.get(idx + 1) == Some(&'/') {
                state.in_block_comment = false;
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
            match c {
                '\\' => escaped = true,
                '"' => state.in_str = false,
                _ => {}
            }
            idx += 1;
            continue;
        }
        match c {
            '"' => {
                state.in_str = true;
                idx += 1;
            }
            '\'' => {
                if let Some(len) = char_literal_len(&chars, idx) {
                    idx += len; // skip the whole literal; its interior is never counted
                } else {
                    out.push(c); // a lifetime (or malformed literal) is ordinary text
                    idx += 1;
                }
            }
            '/' if chars.get(idx + 1) == Some(&'/') => break, // line comment: rest of line is not code
            '/' if chars.get(idx + 1) == Some(&'*') => {
                state.in_block_comment = true;
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

/// Drop whole-line comments (`//`, `/*`, and continuation `*` lines).
///
/// A scanner judges code; a comment is documentation. A doc comment naming a
/// symbol is not a call site, and an explanatory comment describing a bug is
/// not the bug — this repo has been bitten in both directions.
#[must_use]
pub fn strip_comment_lines(src: &str) -> String {
    src.replace('\r', "")
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            !(t.starts_with("//") || t.starts_with("/*") || t.starts_with('*'))
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
        let src = "// a doc mention of foo()\npub fn real() {}\n/* block */\n * continued\n";
        let out = strip_comment_lines(src);
        assert!(out.contains("pub fn real()"));
        assert!(!out.contains("doc mention"));
        assert!(!out.contains("block"));
        assert!(!out.contains("continued"));
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
    /// tree. If this floor drops again, the first question is the same one
    /// this paragraph answers: did the corpus's own `"#[cfg(test)]"`-shaped
    /// text change, or did the extractor stop recognising a shape? Only the
    /// second is alarming.
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
            recovered >= 209,
            "expected >=209 files to recover production code (measured 209 against the \
             shipped extractor on 2026-08-24, post-migration; see the doc comment above \
             for why this moved down from 213); saw {recovered}. A further drop means the \
             extractor stopped recognising a shape — investigate before lowering this \
             floor. (Do not confuse this with the 276 once cited for this class: that \
             figure came from a pre-fix build that over-kept trailing test lines and \
             was itself wrong — see the doc comment above.)"
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
    /// than by pre-filtering the text through `strip_comment_lines` and then
    /// re-numbering the result — the latter reports line numbers that exist
    /// only in the stripped text and not in the file anyone would open.
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
            for (n, line) in text.lines().enumerate() {
                let t = line.trim_start();
                if t.starts_with("//") || t.starts_with("/*") || t.starts_with('*') {
                    continue;
                }
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
