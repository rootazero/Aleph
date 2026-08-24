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

/// A line with line-comments, block-comments, and string/char literal
/// *contents* removed, so braces inside them are not counted by
/// [`end_of_item`]. `state` carries `in_str`/`in_block_comment` across every
/// line of one item scan.
///
/// Never toggles any state on a bare `'`: a lifetime (`&'static str`,
/// `&'a T`) is a single unmatched quote, and treating it as a char-literal
/// delimiter would leave the scanner "inside a char literal" for the rest of
/// the line — dropping every brace after it, including the one that opens
/// the item's body. Since this scanner exists only to count braces, the
/// only char literals that can matter are `'{'` and `'}'`; those two are
/// recognised by exact lookahead and skipped whole. Every other `'` —
/// including every lifetime — is emitted as an ordinary character and
/// changes no state.
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
            '\'' if chars.get(idx + 1) == Some(&'{') && chars.get(idx + 2) == Some(&'\'') => {
                idx += 3; // the char literal '{' — skip whole, don't count the brace
            }
            '\'' if chars.get(idx + 1) == Some(&'}') && chars.get(idx + 2) == Some(&'\'') => {
                idx += 3; // the char literal '}' — skip whole, don't count the brace
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
}
