//! The production half of a Rust source file, for source-level census guards.
//!
//! # Why this is not `src.split("#[cfg(test)]").next()`
//!
//! That idiom cuts at the *first place a test attribute appears*, which is not
//! a boundary. Measured on this repo AT `a95475edd` (1734 files carrying the
//! attribute — state the commit, because this count moves: the round that
//! added this module took it to 1739, nine files gaining the attribute and
//! four losing it; four of the nine are files this round created, including
//! this one. An earlier revision of this sentence said "by adding four files
//! of its own", which does not reach its own endpoint — 1734 + 4 is 1738 —
//! and left the correct decomposition recorded only in FEATURE_LOCATOR, i.e.
//! the comment was the copy that lied):
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
    partition_on_cfg_test(src, Removed::Dropped).0
}

/// The other half: every line [`production_prefix`] removes — each
/// `#[cfg(test)]` attribute line, the item it applies to, and the blank
/// lines between them.
///
/// # Why this shares one walk with `production_prefix`
///
/// The two halves are one partition, so they must have one author. Written
/// as a second scan they would be a second answer to "where does test code
/// begin", free to drift from the first — the shape this repo has paid for
/// before. Here the walk emits both halves in one pass and neither can
/// disagree with the other about a line.
///
/// # What this does NOT cover
///
/// A file that is test-only because its PARENT declares it under
/// `#[cfg(test)] mod x;` (`src/memory/ripple/tests.rs` and 119 others,
/// measured 2026-08-25) carries no `#[cfg(test)]` of its own, so this
/// function returns the empty string for it — the file is entirely test
/// code and this walk cannot tell, because it is handed text and text alone.
///
/// [`test_text`] is that answer: it takes the PATH as well, resolves the one
/// module-graph question this cannot, and falls back to this function. A
/// census that walks a directory should call it rather than this — the
/// difference is invisible until the day someone moves a test module out of
/// its parent file, at which point the guard silently stops charging it.
#[must_use]
pub fn cfg_test_portion(src: &str) -> String {
    partition_on_cfg_test(src, Removed::Dropped).1
}

/// The test half of a source file, with the one question
/// [`cfg_test_portion`] cannot answer resolved: is this file test code
/// BECAUSE ITS PARENT SAYS SO?
///
/// `path` is the file's path (relative to `CARGO_MANIFEST_DIR` or absolute).
/// When the parent module declares it as `#[cfg(test)] mod <stem>;`, the
/// whole file is test code and is returned verbatim; otherwise this is
/// exactly `cfg_test_portion`.
///
/// # Why a path and not a name rule
///
/// "A file called `tests.rs` is tests" is a list that rots — it says nothing
/// about `foo_tests.rs`, it is wrong about a production module someone named
/// `tests`, and it cannot see a whole-file test module under any other name.
/// The declaration in the parent is the actual fact, it is one `read_to_string`
/// away, and it is what `rustc` itself uses. The lookup asks
/// `cfg_test_portion` of the parent, so a `mod tests;` that is NOT under
/// `#[cfg(test)]` (a production submodule that happens to be so named) is
/// correctly not matched.
///
/// # Why this exists
///
/// A directory-walking census reads as exhaustive — it walks every `.rs` file
/// under `src/` — while silently returning nothing for 120+ files that are
/// entirely tests. `gateway::handlers::pty`'s
/// `every_test_that_reaches_the_global_pty_manager_is_tagged` was blind to
/// `src/gateway/runtime/tests.rs` for exactly this reason: the file spawns
/// through the process-global `PtyManager`, the census exists to force the
/// serial key onto tests that do that, and the file was not in its corpus at
/// all. The tag happened to be there. (判据 §3: a guard's green only covers
/// the shapes it recognises.)
#[must_use]
pub fn test_text(path: &std::path::Path, src: &str) -> String {
    if declared_as_a_test_module(path) {
        return src.to_owned();
    }
    cfg_test_portion(src)
}

/// The production half of a file, ASKING ITS PARENT whether the whole file is
/// tests. The exact complement of [`test_text`], and the one a
/// directory-walking census wants.
///
/// [`production_prefix`] works one file at a time, so a whole-file test module
/// — `tests.rs`, `guard_tests.rs`, `drift_tests.rs`, `proptest_enums.rs` and
/// the rest — carries no `#[cfg(test)]` of its own (its PARENT applies one)
/// and comes back as 100% production. Every guard walking `src/` then scans
/// test code as if it shipped.
///
/// # Why not "skip files called tests.rs"
///
/// Because that is the list [`test_text`] already refused to write, and its
/// cost is measured, by the guard below, on this repo: **107** whole-file
/// test modules under `src/`, **30 of them not named `tests.rs`** —
/// `mock_server.rs`, `testkit.rs`, `test_utils.rs`, `census.rs`,
/// `guard_tests.rs`, `drift_tests.rs`, `dispatchable.rs`, the `proptest_*`
/// modules, and `config/tests/mod.rs`. A `rel.ends_with("/tests.rs")` rule
/// sees 77 of the 107 (判据 §3: a guard's green covers the shapes it
/// recognises, and §5: a name list only covers the day it was written).
///
/// Both numbers carry their predicate: "files under `src/` that
/// `declared_as_a_test_module` resolves", at `f84ad424a` plus this round's
/// two fixes to that function. An earlier sentence here said 104 and 19,
/// counted by grepping `#[cfg(test)] mod X;` declaration LINES across five
/// trees — a different population, and a smaller one precisely because the
/// grep shared the bug the guard then found (判据 §18).
///
/// A parent that cannot be read answers "not a test module", so the file is
/// scanned as an ordinary one — the pre-existing behaviour, not a new claim.
#[must_use]
pub fn production_text(path: &std::path::Path, src: &str) -> String {
    if declared_as_a_test_module(path) {
        return String::new();
    }
    production_prefix(src)
}

/// Whether `path`'s parent module declares it with `#[cfg(test)] mod <stem>;`.
///
/// Both spellings of a parent are tried — `dir/mod.rs` for `dir/child.rs`,
/// and `dir.rs` for `dir/child.rs` (the 2018-edition form this repo uses for
/// `src/gateway/runtime/`, whose parent is `src/gateway/runtime/mod.rs`, and
/// for `src/builtin_tools/terminal/`, whose parent is
/// `src/builtin_tools/terminal.rs`). A parent that cannot be read answers
/// `false`: the file is then scanned as an ordinary one, which is the
/// pre-existing behaviour rather than a new claim.
///
/// # Three shapes, and this recognised one of them
///
/// A whole-file test module can be `mod x;` (a file), `pub mod x;` (any
/// visibility), or `mod x;` where `x/` is a DIRECTORY with a `mod.rs`. The
/// first version accepted only the first: it compared the trimmed line to the
/// literal `"mod <stem>;"`, and for a directory module it asked for the stem
/// of `mod.rs`, i.e. a module called `mod`. Both misses were found by
/// `production_text_empties_whole_file_test_modules_and_only_those` the first
/// time it ran, on `src/acp/mock_server.rs` (`pub mod`) and
/// `src/config/tests/mod.rs` (a directory) — 判据 §3: what a guard
/// recognises, not what its author had in mind.
///
/// # The visibility prefix is part of the declaration
///
/// The first version compared the trimmed line to the literal
/// `"mod <stem>;"`, which is false for every `pub mod x;` — and this repo has
/// eight of them under `#[cfg(test)]`, `src/acp/mock_server.rs` and
/// `src/capability/census.rs` included. `census` is the one this file's OWN
/// test suite already discusses by name ("`capability/mod.rs`, which
/// genuinely holds a mid-file `#[cfg(test)] pub(crate) mod census;`"), so the
/// spelling was known here and the matcher still did not accept it —
/// 判据 §3: what a guard recognises, not what its author had in mind.
fn declared_as_a_test_module(path: &std::path::Path) -> bool {
    let absolute;
    let path = if path.is_absolute() {
        path
    } else {
        absolute = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
        &absolute
    };
    // A directory module is named by its DIRECTORY, and its parent is one
    // level further up: `src/config/tests/mod.rs` is `mod tests;` declared in
    // `src/config/mod.rs`, not `mod mod;` declared in `src/config/tests/`.
    // Without this the lookup asks for a module called `mod` and every
    // directory-shaped test module answers "not a test module".
    let (stem, dir) = if path.file_name().is_some_and(|n| n == "mod.rs") {
        let dir = path.parent();
        match (
            dir.and_then(std::path::Path::file_name),
            dir.and_then(std::path::Path::parent),
        ) {
            (Some(name), Some(up)) => (name.to_owned(), up),
            _ => return false,
        }
    } else {
        match (path.file_stem(), path.parent()) {
            (Some(stem), Some(dir)) => (stem.to_owned(), dir),
            _ => return false,
        }
    };
    let declaration = format!("mod {};", stem.to_string_lossy());
    [dir.join("mod.rs"), dir.with_extension("rs")]
        .iter()
        .filter_map(|parent| std::fs::read_to_string(parent).ok())
        .any(|text| {
            cfg_test_portion(&text)
                .lines()
                .any(|line| strip_visibility(line.trim()) == declaration)
        })
}

/// A leading `pub`, `pub(crate)`, `pub(super)` or `pub(in path)`, removed.
///
/// Returns the input unchanged when there is none, so a caller can compare
/// against the bare form either way.
fn strip_visibility(line: &str) -> &str {
    let Some(rest) = line.strip_prefix("pub") else {
        return line;
    };
    let rest = match rest.strip_prefix('(') {
        // `pub(crate)` / `pub(super)` / `pub(in a::b)` — skip to the matching
        // `)`. Module paths contain no nested parens, so `find` is exact.
        Some(inner) => match inner.find(')') {
            Some(end) => &inner[end + 1..],
            None => return line,
        },
        None => rest,
    };
    // `pub` must be a whole word: `public_mod x;` is not a visibility.
    match rest.strip_prefix(' ') {
        Some(tail) => tail.trim_start(),
        None => line,
    }
}

/// What the production half does with a line that belongs to the test half.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Removed {
    /// Delete it — the output is text only, and offsets into it are not line
    /// numbers of anything.
    Dropped,
    /// Replace it with an empty line, so the output still numbers like the
    /// file it came from.
    Blanked,
}

/// One walk, both halves: `(production, cfg-test)`.
fn partition_on_cfg_test(src: &str, removed: Removed) -> (String, String) {
    let normalized = src.replace('\r', "");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    let mut test: Vec<&str> = Vec::new();
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
            // Dangling attribute at EOF. It is not production; it applies to
            // no item, so the test half is where it belongs.
            test.extend_from_slice(&lines[i..]);
            if removed == Removed::Blanked {
                out.resize(lines.len(), "");
            }
            break;
        }
        let end = end_of_item(&lines, item);
        test.extend_from_slice(&lines[i..end]);
        if removed == Removed::Blanked {
            out.resize(end, "");
        }
        i = end;
    }
    (out.join("\n"), test.join("\n"))
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
        let code = code_only(lines[k], &mut state, Payloads::Stripped);
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
///
/// `payloads` selects between the two questions a census can ask of the same
/// walk — see [`Payloads`]. Comments are dropped either way; only literal
/// interiors move.
fn code_only(line: &str, state: &mut LexState, payloads: Payloads) -> String {
    let keep = payloads == Payloads::Kept;
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
            if keep {
                out.push(c);
            }
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
                    if keep {
                        out.push(c);
                    }
                    idx += 1;
                }
                continue;
            }
            match c {
                '\\' => {
                    escaped = true;
                    if keep {
                        out.push(c);
                    }
                }
                '"' => {
                    state.in_str = false;
                    out.push('"'); // sentinel
                }
                _ => {
                    if keep {
                        out.push(c);
                    }
                }
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
                    if keep {
                        out.extend(chars[idx..idx + len].iter());
                    } else {
                        out.push('_');
                    }
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

/// What [`code_only`] does with the *interior* of a string / char literal.
///
/// Two censuses want opposite things from the same walk and neither can be
/// served by the other:
///
/// - [`Payloads::Stripped`] ([`code_text`]) is what a guard searching for a
///   piece of CODE needs, because its own message strings and marker constants
///   live inside the corpus it scans — see `code_text`'s doc.
/// - [`Payloads::Kept`] ([`code_keeping_literals`]) is what a guard searching
///   for a *literal value* needs. `flow_scope_census` looks for
///   `get("scope_id")` — a raw metadata read spelled by the key's value rather
///   than by its constant — and `code_text` deletes exactly the bytes it is
///   looking for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Payloads {
    Stripped,
    Kept,
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
        .filter(|line| line_is_code(line, &mut state))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Does this line carry anything but comment?
///
/// One author for the predicate, because [`strip_comment_lines`] and
/// [`production_code_lines`] ask exactly this question and answer it
/// differently only in what they do with a `false` — drop the line, or blank
/// it. `state` must be threaded in line order: [`code_only`] is what advances
/// it, and `in_str` is read BEFORE that call on purpose (see
/// [`strip_comment_lines`]'s doc).
fn line_is_code(line: &str, state: &mut LexState) -> bool {
    let entered_in_str = state.in_str;
    let code = code_only(line, state, Payloads::Stripped);
    entered_in_str || line.trim().is_empty() || !code.trim().is_empty()
}

/// The production half with comment-only lines blanked — **and line numbers
/// that still match the file the reader will open**.
///
/// `strip_comment_lines(production_prefix(src))` answers the same question
/// about the TEXT and is the right call whenever only the text matters. Both
/// of those DELETE lines, though, so an offset into their output is not a line
/// number of anything: `production_prefix` removes each `#[cfg(test)]` item and
/// `strip_comment_lines` removes each comment-only line.
///
/// This exists because a guard reported one anyway.
/// `diagnostics::checks::presence_discipline::no_check_folds_a_bound_error_into_an_answer`
/// counted `'\n'` in the stripped text and printed the result as a file line
/// "so an offender can be opened" — in a directory that is ~40% doc comment,
/// off by 19 lines on the first real offender. The number pointed at innocent
/// code, which is worse than printing no number: a reader who opens it
/// concludes the guard is broken.
///
/// Blanking rather than deleting keeps every other property the two callers
/// rely on — the same predicate decides comment lines, and the same walk
/// decides test items, so this cannot drift from either.
#[must_use]
pub fn production_code_lines(src: &str) -> String {
    let production = partition_on_cfg_test(src, Removed::Blanked).0;
    let mut state = LexState::default();
    production
        .lines()
        .map(|line| {
            if line_is_code(line, &mut state) {
                line
            } else {
                ""
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `src` reduced to the code a compiler would see: comment text removed,
/// and the *payload* of every string, byte-string, C-string, raw-string and
/// char literal removed too, each literal leaving a delimiter sentinel in
/// place (see [`code_only`]). Line structure is preserved.
///
/// # Why a census wants this and not just [`strip_comment_lines`]
///
/// A scanner's own message strings, marker constants, and fixtures are
/// inside the corpus it scans. Comment-stripping alone leaves them there,
/// so a guard looking for the literal text `foo()` finds its OWN
/// `"...foo()..."` and either fires on itself or grows an exemption for
/// itself — and an exemption is the thing that later hides a real hit.
/// Removing literal payloads deletes the whole problem class instead of
/// naming its instances.
///
/// The naive alternative — walking `"` characters and blanking between
/// alternating pairs — desynchronises on the first raw string carrying an
/// odd number of embedded quotes (`tokenize(r#"--role "unclosed role"#)`,
/// `src/group_chat/channel.rs`), after which the "inside a literal" region
/// runs to the end of the scanned text and swallows every subsequent line
/// into blanks. That is the silent-approval direction: a guard reports a
/// clean scan of text it never looked at. [`LexState`] lexes raw strings
/// properly, which is why this composition is safe where that one is not.
#[must_use]
pub fn code_text(src: &str) -> String {
    let mut state = LexState::default();
    src.replace('\r', "")
        .lines()
        .map(|line| code_only(line, &mut state, Payloads::Stripped))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `src` with comment text removed and every literal payload KEPT — the other
/// half of [`code_text`]'s question, over the same lexer walk.
///
/// # Why this exists
///
/// [`code_text`] deletes literal payloads so a guard hunting for a piece of
/// code cannot fire on its own message strings. That is exactly wrong for a
/// guard hunting for a literal VALUE: `flow_scope_census` must see
/// `request.metadata.get("scope_id")`, a raw read spelled by the key's value
/// instead of by `crate::scope::SCOPE_META_KEY`, and `code_text` removes the
/// bytes that distinguish it. A review reproduced the shipped defect end to
/// end through that hole, on a build where all three census checks were green.
///
/// [`production_code_lines`] is not a substitute. It blanks comment-ONLY
/// lines, so a doc comment quoting a key is not a hit — but a comment that
/// TRAILS live code on the same line survives it, and a census searching that
/// text would fire on prose. That false positive is the expensive direction
/// (「一条会误报的守卫比一条不报的守卫贵，因为它会被当成证据引用」), so the
/// comment stripping has to come from the lexer rather than from a line
/// filter.
///
/// A literal payload's own quotes are preserved as the same `"` sentinel
/// `code_text` leaves, so `"scope_id"` — quotes included — is an EXACT
/// payload match: a message that merely mentions the key in prose
/// (`"scope_id missing"`) does not contain it, and an escaped inner quote
/// (`"\"scope_id\""`) does not either.
#[must_use]
pub fn code_keeping_literals(src: &str) -> String {
    let mut state = LexState::default();
    src.replace('\r', "")
        .lines()
        .map(|line| code_only(line, &mut state, Payloads::Kept))
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
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
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

/// One `#[test]` / `#[tokio::test]` function found by [`scan_test_bodies`].
#[cfg(test)]
pub(crate) struct TestBody {
    /// The `fn …` line, trimmed — the test's identity in a violation message.
    pub name: String,
    /// Every attribute line attached to it, trimmed: the ones ABOVE the
    /// `#[test]` marker as well as the ones between it and the `fn` line.
    /// Both sides, because `#[serial_test::serial(k)]` written above `#[test]`
    /// is the same tag, and a census that only looked below would report a
    /// tagged test as a violation — the expensive direction, since a guard
    /// that misfires gets cited as evidence.
    pub attrs: Vec<String>,
    /// Whether the predicate matched a line of the brace-matched body.
    pub reaches: bool,
}

/// What [`scan_test_bodies`] saw in one file.
#[cfg(test)]
pub(crate) struct TestBodyScan {
    /// Every test function in the file, in source order.
    pub tests: Vec<TestBody>,
    /// Matching lines that landed in NO test body — `(1-based line, trimmed
    /// text)`. A shared helper is the usual cause, and it is a finding rather
    /// than a hit to drop: see [`scan_test_bodies`].
    pub uncharged: Vec<(usize, String)>,
}

/// Attribute each line matching `reaches` to the `#[test]` / `#[tokio::test]`
/// function whose brace-matched body contains it.
///
/// # Why this is one function and not one per census
///
/// A "membership derived from the CALL" census — the shape that replaced
/// `gateway::handlers::pty`'s file-enumerating guard after a measured
/// 3-failures-in-8-runs flake, and that `providers::route_observe` now carries
/// over for the route globals (判据 #16) — is two separable halves: *which
/// lines reach the singleton* (the census's own question) and *which test owns
/// a line* (this walk). The second half is one fact, so it has one author:
/// written twice, the two copies are free to disagree about which attributes
/// belong to a test or where a body ends, and only one of them would ever get
/// the next fix.
///
/// Feed it [`code_text`] of [`cfg_test_portion`]: literal payloads are blanked
/// there, so a `{` inside a string cannot desynchronise the brace matching and
/// a census's own needle constants cannot match themselves.
///
/// # The one case it refuses to guess
///
/// A hit that lands in no test body is returned in
/// [`uncharged`](TestBodyScan::uncharged) rather than silently dropped or
/// charged to the nearest test: the walk cannot tell which tests call a shared
/// helper, and a verdict about the wrong function is worse than no verdict.
/// Every caller so far turns those into failures naming themselves.
#[cfg(test)]
pub(crate) fn scan_test_bodies(code: &str, reaches: &dyn Fn(&str) -> bool) -> TestBodyScan {
    let lines: Vec<&str> = code.lines().collect();
    let mut charged = vec![false; lines.len()];
    let mut tests: Vec<TestBody> = Vec::new();

    let mut i = 0usize;
    while i < lines.len() {
        let marker = lines[i].trim();
        if !(marker.starts_with("#[tokio::test") || marker == "#[test]") {
            i += 1;
            continue;
        }
        let mut attrs = Vec::new();
        let mut above = i;
        while above > 0 && lines[above - 1].trim().starts_with('#') {
            above -= 1;
            attrs.push(lines[above].trim().to_string());
        }
        // The rest of the attribute block, then the `fn` line.
        let mut j = i + 1;
        while j < lines.len() && lines[j].trim().starts_with("#[") {
            attrs.push(lines[j].trim().to_string());
            j += 1;
        }
        if j >= lines.len() {
            break;
        }
        let name = lines[j].trim().to_string();

        // Brace-match the body.
        let (mut depth, mut opened, mut end) = (0i32, false, j);
        for (k, l) in lines.iter().enumerate().skip(j) {
            depth += i32::try_from(l.matches('{').count()).unwrap_or(0);
            depth -= i32::try_from(l.matches('}').count()).unwrap_or(0);
            opened |= l.contains('{');
            end = k;
            if opened && depth <= 0 {
                break;
            }
        }

        tests.push(TestBody {
            name,
            attrs,
            reaches: lines[j..=end].iter().any(|l| reaches(l)),
        });
        for c in charged.iter_mut().take(end + 1).skip(j) {
            *c = true;
        }
        i = end + 1;
    }

    let uncharged = lines
        .iter()
        .enumerate()
        .filter(|(k, l)| !charged[*k] && reaches(l))
        .map(|(k, l)| (k + 1, (*l).trim().to_string()))
        .collect();

    TestBodyScan { tests, uncharged }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`test_text`] against both shapes, on real files in this repo rather
    /// than on a fixture — the claim it makes is about how THIS tree is laid
    /// out, and a synthetic parent/child pair would prove only that the
    /// function reads what the test wrote.
    ///
    /// The first assertion is the precondition, and it is the whole reason
    /// this function exists: `cfg_test_portion` answers "" for a file that is
    /// entirely tests, so a census asking it for that file's test code is
    /// handed nothing and skips the file in silence. If that ever stops being
    /// true, this test says so instead of quietly becoming a tautology.
    #[test]
    fn test_text_sees_a_whole_file_test_module_that_cfg_test_portion_cannot() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

        // Declared by its parent as `#[cfg(test)] mod tests;`.
        let whole = root.join("src/gateway/runtime/tests.rs");
        let src = std::fs::read_to_string(&whole).expect("src/gateway/runtime/tests.rs");
        assert!(
            cfg_test_portion(&src).is_empty(),
            "precondition: a whole-file test module carries no `#[cfg(test)]` of its own, so \
             the text-only function can only answer with the empty string"
        );
        assert_eq!(
            test_text(&whole, &src),
            src,
            "a file its parent declares under `#[cfg(test)]` is test code end to end"
        );

        // An ordinary file with an inline `#[cfg(test)] mod tests { .. }`:
        // unchanged, or this function would be a second answer rather than a
        // wider one.
        let inline = root.join("src/utils/source_scan.rs");
        let src = std::fs::read_to_string(&inline).expect("this file");
        let portion = cfg_test_portion(&src);
        assert!(
            !portion.is_empty(),
            "precondition: this file has inline tests"
        );
        assert_eq!(test_text(&inline, &src), portion);
        assert_ne!(
            test_text(&inline, &src),
            src,
            "and a file with production code in it must not be swallowed whole"
        );
    }

    /// [`scan_test_bodies`] is the shared half of two census guards
    /// (`gateway::handlers::pty`'s global `PtyManager`,
    /// `providers::route_observe`'s route globals), so its own failure modes
    /// are asserted here — on a fixture — instead of being rediscovered in
    /// whichever guard misfires first. All four are load-bearing: a tag
    /// written ABOVE `#[test]` is the false-positive direction (a tagged test
    /// reported as a violation, and a guard that misfires gets cited as
    /// evidence), a hit in a shared helper is the case the walk refuses to
    /// guess, and a test whose body does not match must come back
    /// `reaches: false` rather than be dropped — the pty census counts every
    /// test in a reaching file to prove it is not scanning nothing.
    #[test]
    fn scan_test_bodies_attributes_hits_from_both_attribute_sides_and_reports_orphans() {
        let src = r#"mod tests {
    fn helper() {
        manager();
    }

    #[serial_test::parallel(k)]
    #[test]
    fn tag_above_the_marker() {
        manager();
    }

    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::serial(k)]
    async fn tag_below_the_marker() {
        if true {
            manager();
        }
    }

    #[test]
    fn reaches_nothing() {
        assert!(true);
    }
}
"#;
        let scan = scan_test_bodies(src, &|l: &str| l.contains("manager()"));

        let names: Vec<&str> = scan.tests.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names.len(), 3, "one entry per test function, got {names:?}");
        assert!(names[0].contains("tag_above_the_marker"), "{names:?}");
        assert!(
            scan.tests[0].reaches
                && scan.tests[0]
                    .attrs
                    .iter()
                    .any(|a| a == "#[serial_test::parallel(k)]"),
            "an attribute written above `#[test]` belongs to that test: {:?}",
            scan.tests[0].attrs
        );
        assert!(
            scan.tests[1].reaches
                && scan.tests[1]
                    .attrs
                    .iter()
                    .any(|a| a == "#[serial_test::serial(k)]"),
            "a nested-brace body still ends at its own closing brace: {:?}",
            scan.tests[1].attrs
        );
        assert!(
            !scan.tests[2].reaches,
            "a test that does not match is reported, not dropped"
        );
        assert_eq!(
            scan.uncharged,
            vec![(3usize, "manager();".to_string())],
            "the helper's hit is charged to no test, with a 1-based line"
        );
    }

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
    /// The two halves are one partition: every line is in exactly one of
    /// them, and together they reconstruct the file. Asserted rather than
    /// assumed, because the whole point of deriving the test half from this
    /// walk is that it cannot disagree with the production half.
    /// The property `production_code_lines` exists for, stated against its
    /// deleting counterpart so the difference is visible rather than asserted.
    ///
    /// A guard reported a line number off by 19 because it counted `'\n'` in
    /// the deleting pair's output. Both halves are checked here: the line
    /// numbering is preserved, and the pair it replaces really does move it.
    #[test]
    fn production_code_lines_numbers_like_the_file_and_the_deleting_pair_does_not() {
        let src = "//! doc line\n//! doc line two\n\nfn a() {}\n\n#[cfg(test)]\nmod t {\n    fn x() {}\n}\n\nfn b() {}\n";
        let kept = production_code_lines(src);
        let deleted = strip_comment_lines(&production_prefix(src));

        let at = |text: &str| {
            text.lines()
                .position(|l| l.contains("fn b()"))
                .expect("the marker line must survive both")
        };
        assert_eq!(
            kept.lines().count(),
            src.lines().count(),
            "blanking must not change the line count"
        );
        assert_eq!(at(&kept), at(src), "the marker must keep its file line");
        assert_ne!(
            at(&deleted),
            at(src),
            "precondition: the deleting pair really does renumber -- if this \
             ever stops holding, the function above has no reason to exist"
        );
        // The removed regions are blank, not gone.
        assert_eq!(kept.lines().next().unwrap(), "", "doc line blanked");
        assert_eq!(
            kept.lines().nth(5).unwrap(),
            "",
            "#[cfg(test)] line blanked"
        );
    }

    #[test]
    fn the_two_halves_partition_the_file() {
        let src = "pub fn a() {}\n\n#[cfg(test)]\nmod tests {\n    fn t() {}\n}\n\npub fn b() {}\n";
        let prod = production_prefix(src);
        let tests = cfg_test_portion(src);
        assert!(prod.contains("pub fn a()") && prod.contains("pub fn b()"));
        assert!(!prod.contains("mod tests"));
        assert!(
            tests.contains("#[cfg(test)]")
                && tests.contains("mod tests")
                && tests.contains("fn t()")
        );
        assert!(!tests.contains("pub fn a()") && !tests.contains("pub fn b()"));
        let mut all: Vec<&str> = prod.lines().chain(tests.lines()).collect();
        let mut original: Vec<&str> = src.trim_end_matches('\n').lines().collect();
        all.sort_unstable();
        original.sort_unstable();
        assert_eq!(all, original, "every line must land in exactly one half");
    }

    /// A file whose test code is reached only through a parent's
    /// `#[cfg(test)] mod x;` carries no attribute of its own, so its test
    /// portion is empty. Declared, not accidental — a caller that treats an
    /// empty portion as "nothing to check" would be blind to 120 files in
    /// this repo.
    #[test]
    fn a_file_with_no_cfg_test_attribute_has_an_empty_test_portion() {
        let src = "use super::*;\n\n#[test]\nfn t() {}\n";
        assert_eq!(production_prefix(src), src);
        assert!(cfg_test_portion(src).is_empty());
    }

    /// The shape that desynchronises an alternating-quote blanker: a raw
    /// string carrying an odd number of embedded quotes. Everything after
    /// it must remain visible — the failure this guards against is the
    /// scanner silently approving text it blanked away.
    #[test]
    fn code_text_survives_a_raw_string_with_an_embedded_quote() {
        let src = "fn t() {\n    let x = tokenize(r#\"--role \"unclosed role\"#);\n    danger_marker();\n}\n";
        let code = code_text(src);
        assert!(
            code.contains("danger_marker()"),
            "code after an odd-quote raw string must stay visible; got:\n{code}"
        );
        assert!(
            !code.contains("unclosed role"),
            "the raw string's payload must be gone; got:\n{code}"
        );
    }

    /// The reason a census wants payloads gone: its own marker strings and
    /// messages are inside the corpus it scans.
    #[test]
    fn code_text_removes_literal_payloads_but_keeps_the_code_around_them() {
        let src =
            "fn t() {\n    let m = \"danger_marker()\";\n    let c = '{';\n    real_call();\n}\n";
        let code = code_text(src);
        assert!(!code.contains("danger_marker()"), "got:\n{code}");
        assert!(code.contains("real_call()"), "got:\n{code}");
        assert!(code.contains("let m ="), "got:\n{code}");
    }

    /// The mirror question, and the reason `code_keeping_literals` is not just
    /// `production_code_lines`: a guard hunting for a literal VALUE has to see
    /// the payload, and must still not see prose.
    #[test]
    fn code_keeping_literals_keeps_payloads_and_still_drops_every_comment() {
        let src = concat!(
            "fn t() {\n",
            "    let a = m.get(\"scope_id\");\n",
            "    let b = 1; // trailing mention of m.get(\"scope_id\")\n",
            "    // whole-line mention of m.get(\"scope_id\")\n",
            "    let c = /* inline */ m.get(r#\"scope_id\"#);\n",
            "}\n",
        );
        let kept = code_keeping_literals(src);
        assert_eq!(
            kept.matches("\"scope_id\"").count(),
            2,
            "exactly the two live reads — the raw string counts, both comments \
             do not. got:\n{kept}"
        );
        assert!(!kept.contains("trailing mention"), "got:\n{kept}");
        assert!(!kept.contains("whole-line mention"), "got:\n{kept}");
        assert!(!kept.contains("inline"), "got:\n{kept}");
        // The same input through the payload-stripping half sees neither —
        // which is the hole this function exists to close.
        assert!(!code_text(src).contains("scope_id"));
    }

    /// A quoted needle is an EXACT payload match, not a substring search:
    /// prose that mentions the key, and an escaped inner quote, are both
    /// misses. Without this the guard built on it would fire on its own
    /// failure messages.
    #[test]
    fn a_quoted_needle_does_not_match_a_literal_that_merely_mentions_it() {
        let src = concat!(
            "fn t() {\n",
            "    panic!(\"scope_id missing from the map\");\n",
            "    let q = \"a \\\"scope_id\\\" in prose\";\n",
            "}\n",
        );
        let kept = code_keeping_literals(src);
        assert!(kept.contains("scope_id"), "premise: payloads survive");
        assert_eq!(
            kept.matches("\"scope_id\"").count(),
            0,
            "neither line is a read of the key. got:\n{kept}"
        );
    }

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

    /// `production_text` empties a whole-file test module and leaves an
    /// ordinary file alone — measured against THIS repo, not a fixture.
    ///
    /// The population is derived by reading every `#[cfg(test)] mod X;` out
    /// of the file that declares it, so this cannot drift the way a list of
    /// filenames would. Two things are asserted about that population,
    /// because either alone is satisfiable by a broken implementation:
    ///
    ///  * every member empties — a `production_text` that returned the file
    ///    unchanged fails here;
    ///  * at least one member is NOT named `tests` — which is what makes the
    ///    `rel.ends_with("/tests.rs")` rule this replaced insufficient. If
    ///    that ever stops being true, the name rule became adequate and this
    ///    guard is testing nothing (判据 §2).
    ///
    /// And the negative arm: an ordinary production file must still come back
    /// with its code, or "everything is empty" would pass both checks above.
    #[test]
    fn production_text_empties_whole_file_test_modules_and_only_those() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let sources = rust_sources_under(&root);
        assert!(
            sources.len() > 1000,
            "the walk found only {} files — a guard that examined nothing is \
             green and blind",
            sources.len()
        );

        // Every file some parent declares as `#[cfg(test)] mod <stem>;`.
        let mut declared: std::collections::BTreeSet<String> = Default::default();
        for (rel, text) in &sources {
            let Some((parent, leaf)) = rel.rsplit_once('/') else {
                continue;
            };
            // The module PATH this file's `mod x;` lines are relative to:
            // `a/b/mod.rs` declares into `a/b`, and `a/b.rs` into `a/b` too
            // (it is the 2018-edition parent of `a/b/`).
            let dir = if leaf == "mod.rs" {
                parent.to_string()
            } else {
                format!("{parent}/{}", leaf.trim_end_matches(".rs"))
            };
            for line in cfg_test_portion(text).lines() {
                let t = line.trim_start();
                let t = t
                    .strip_prefix("pub(crate) ")
                    .or_else(|| t.strip_prefix("pub(super) "))
                    .or_else(|| t.strip_prefix("pub "))
                    .unwrap_or(t);
                let Some(rest) = t.strip_prefix("mod ") else {
                    continue;
                };
                let Some(name) = rest.strip_suffix(';') else {
                    continue;
                };
                let name = name.trim();
                if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                {
                    continue;
                }
                declared.insert(format!("{dir}/{name}.rs"));
                declared.insert(format!("{dir}/{name}/mod.rs"));
            }
        }

        let by_path: std::collections::BTreeMap<&str, &String> =
            sources.iter().map(|(r, t)| (r.as_str(), t)).collect();
        let mut emptied = 0usize;
        let mut not_named_tests: Vec<&str> = Vec::new();
        for rel in &declared {
            let Some(text) = by_path.get(rel.as_str()) else {
                continue;
            };
            let prod = production_text(std::path::Path::new(rel), text);
            assert!(
                prod.trim().is_empty(),
                "{rel} is declared `#[cfg(test)] mod …;` by its parent, so its \
                 production half is empty; production_text returned {} bytes",
                prod.len()
            );
            emptied += 1;
            if !rel.ends_with("/tests.rs") {
                not_named_tests.push(rel.as_str());
            }
        }

        assert!(
            emptied > 50,
            "only {emptied} whole-file test modules were found under src/ — \
             the derivation broke, and an empty population passes every \
             assertion in this test"
        );
        assert!(
            !not_named_tests.is_empty(),
            "every whole-file test module is now called `tests.rs`, so the \
             name rule `production_text` replaced would have been adequate \
             and this guard no longer separates them"
        );
        // A production file, for the negative arm.
        let me = by_path
            .get("src/utils/source_scan.rs")
            .expect("this file is under the walk");
        assert!(
            production_text(std::path::Path::new("src/utils/source_scan.rs"), me)
                .contains("pub fn production_text"),
            "an ordinary file must keep its production code"
        );
        eprintln!(
            "production_text: {emptied} whole-file test modules under src/, \
             {} of them not named tests.rs (e.g. {:?})",
            not_named_tests.len(),
            &not_named_tests[..not_named_tests.len().min(5)]
        );
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
    ///
    /// # It drifts UPWARD on documentation churn, so "it went up" says nothing either
    ///
    /// Re-measured 2026-08-25: the actual value is **196** and this floor is
    /// deliberately still 193. Today's extractor reproduces 193 exactly on the
    /// `8d963222a` corpus, so the extractor is exonerated and the whole +3 is
    /// corpus drift — but only ONE of the three is a real recovery
    /// (`capability/mod.rs`, which genuinely holds a mid-file
    /// `#[cfg(test)] pub(crate) mod census;`). The other two,
    /// `extension/manager_global.rs` and `mcp/sampling_bridge.rs`, recover only
    /// because they gained DOC COMMENTS that mention the attribute, and
    /// `old_prefix_cut` below is an unanchored whole-text match that truncates
    /// on prose exactly as it does on code. Writing a sentence about
    /// `#[cfg(test)]` therefore moves this number.
    ///
    /// So 193, 194 and 196 are all correct, under three different predicates:
    /// the last explained measurement, the count of genuine recoveries, and
    /// what the assertion below literally computes. The floor stays at the
    /// first of those, because every number in the chain above is one somebody
    /// explained. **State the predicate beside whatever number you write here**
    /// — that, not the integer, is what makes this a measurement.
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
        assert!(
            worst.0 > 10_000,
            "worst-case recovery {worst:?} is implausibly small"
        );
    }

    /// Does `line` hold a string literal that OPENS with `#[cfg(test)]`?
    ///
    /// This replaced three literal patterns —
    /// `split("#[cfg(test)]")` / `find("#[cfg(test)]")` /
    /// `split_once("#[cfg(test)]")` — that named the three spellings present
    /// on the day guard 3 was written and were therefore blind to every other
    /// one. Measured 2026-08-25, five live sites in `src/` used a spelling
    /// none of the three matched: two through a `const ATTR`, two through a
    /// longer needle (`"#[cfg(test)]\nmod "`), one through
    /// `starts_with("#[cfg(test)]")`. Guard 3 reported zero offenders the
    /// whole time, which is what a list of spellings always eventually
    /// reports.
    ///
    /// The discriminator is the literal's OFFSET, not the method called on it,
    /// so `splitn` / `match_indices` / whatever comes next is covered without
    /// anyone adding a row. Leading `\n` / `\r\n` escapes are stepped over —
    /// the line-anchored spelling is the same cut with a CRLF story attached.
    /// An attribute further inside a literal is prose or a fixture (an
    /// assertion message reading "the #[cfg(test)] split matched nothing" is
    /// not a second cut), and there are 11 such lines in `src/` that must not
    /// be flagged.
    ///
    /// # What it cannot see
    ///
    /// A needle assembled from pieces (`concat!`, two constants joined), and a
    /// cut that never spells the attribute at all — one keying on
    /// `"mod tests {"`, say. Both need value flow rather than text. Named here
    /// rather than left implied: a scanner that does not say what it misses
    /// gets read as one that misses nothing.
    ///
    /// # There is a byte-identical twin, deliberately
    ///
    /// `interfaces/webchat/src/i18n_census.rs` carries the same function for
    /// `aleph-panel`'s copy of this rule. It is a second implementation for the
    /// same reason `production_lines` is: that crate cannot depend on
    /// `alephcore` (wasm frontend, R1/R3) and the capability-wiring spec's
    /// non-goal 1 (不拆 crate) rules out a shared crate. Two copies of one
    /// predicate can drift, so both carry the same unit cases
    /// (`the_cfg_test_literal_detector_reads_the_offset_not_the_method`) and
    /// each doc names the other. If you change one, change both — this copy is
    /// the load-bearing one: it scans the whole `src/` tree and adjudicates the
    /// registered exemptions below.
    fn opens_a_cfg_test_literal(line: &str) -> bool {
        const ATTR: &str = "#[cfg(test)]";
        let bytes = line.as_bytes();
        for (quote, _) in line.match_indices('"') {
            let mut j = quote + 1;
            while j + 1 < bytes.len() && bytes[j] == b'\\' && matches!(bytes[j + 1], b'n' | b'r') {
                j += 2;
            }
            if line[j..].starts_with(ATTR) {
                return true;
            }
        }
        false
    }

    /// Files that still hand-roll the cut, and why each is still here.
    ///
    /// Task 3 migrated 36 sites onto [`production_prefix`] and closed with
    /// "zero offenders". That was true of the three spellings it searched for.
    /// Widening the search to the rule (above) found these five, all older
    /// than that round. They are registered rather than migrated because
    /// migrating each is a behaviour question of its own, not a rename:
    ///
    /// **This list may only shrink**, and the assertion below pins its size so
    /// that shrinking is the only silent direction — a sixth site fails loudly
    /// as an offender, and removing a row without migrating fails too.
    const KNOWN_UNMIGRATED_CUTS: &[(&str, &str)] = &[
        (
            "src/gateway/btw/guard_tests.rs",
            "const ATTR + find(ATTR), but it requires the attribute to be \
             followed by `mod` (stepping over a visibility qualifier), so it \
             deliberately does NOT stop at a `#[cfg(test)] use`. That is a \
             different rule from production_prefix's, argued in its own doc.",
        ),
        (
            "src/gateway/continuation_lifecycle.rs",
            "the same const-ATTR shape as guard_tests.rs, minus the visibility \
             handling — two copies of one idea, already drifted. Converging \
             them is worth doing and is not a rename.",
        ),
        (
            "src/gateway/execution_engine/run_loop/tests.rs",
            "splits on the longer needle `\"#[cfg(test)]\\nmod tests\"`, which \
             is a narrower cut than the naive one and not equivalent to \
             production_prefix on a file with a gated non-mod item.",
        ),
        (
            "src/session/steer_signal.rs",
            "splits on `\"#[cfg(test)]\\nmod \"` — same shape as run_loop's.",
        ),
        (
            "src/harness/tests/budget.rs",
            "counts budgeted LINES for the R10 harness ratchet rather than \
             extracting text. Swapping the cut moves that ratchet's number, \
             which is a decision for whoever owns the ratchet.",
        ),
    ];

    /// Guard 3 — no second author.
    ///
    /// The detector is a rule ([`opens_a_cfg_test_literal`]), not a list of
    /// spellings; the list it does carry ([`KNOWN_UNMIGRATED_CUTS`]) is five
    /// registered sites the rule found and Task 3's three-spelling search never
    /// could, size-pinned so it cannot grow into a licence. Read both docs
    /// before touching either: the previous version of this comment claimed
    /// "the rule, not an exemption list" while the code was three literals, and
    /// that is the direction this file exists to make impossible.
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
    /// output: the thing this guard hunts IS a string literal, and `code_only`
    /// removes literal contents by design. `code_only` decides only whether the
    /// line is code at all.
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
                if code_only(line, &mut state, Payloads::Stripped)
                    .trim()
                    .is_empty()
                {
                    continue;
                }
                if opens_a_cfg_test_literal(line) {
                    offenders.push((rel.clone(), format!("{rel}:{}", n + 1)));
                }
            }
        }

        let (known, rest): (Vec<_>, Vec<_>) = offenders
            .into_iter()
            .partition(|(file, _)| KNOWN_UNMIGRATED_CUTS.iter().any(|(f, _)| f == file));
        let rest: Vec<String> = rest.into_iter().map(|(_, at)| at).collect();
        assert!(
            rest.is_empty(),
            "these hand-roll the production-prefix cut instead of calling \
             `utils::source_scan::production_prefix`:\n  {}",
            rest.join("\n  ")
        );

        // Per FILE, not a total. A total is a scalar aggregate over two
        // independent events, and they cancel: migrate one registered site
        // while another registered file grows a second hand-rolled cut and
        // `5 == 5` still holds, so the message below — which anticipates both
        // events separately — could never fire on both at once.
        let mut per_file: std::collections::BTreeMap<&str, usize> = KNOWN_UNMIGRATED_CUTS
            .iter()
            .map(|(file, _)| (*file, 0usize))
            .collect();
        // Swapping a scalar for a map drops whatever the scalar could see that
        // the map cannot, and that loss is invisible because the map is
        // strictly better at the thing being fixed. Here it is duplicate rows:
        // two entries for one file collapse to one key and every check below
        // passes. Granting nothing extra is not the point — a list that
        // misdescribes itself is exactly what the registered-cut mechanism
        // exists to catch one level down.
        assert_eq!(
            per_file.len(),
            KNOWN_UNMIGRATED_CUTS.len(),
            "KNOWN_UNMIGRATED_CUTS has {} rows but only {} distinct files, so \
             at least one file is registered twice. Merge the duplicates: the \
             per-file check below cannot see them, and a list that does not \
             describe itself is the defect this list exists to record.",
            KNOWN_UNMIGRATED_CUTS.len(),
            per_file.len()
        );
        let mut matched: Vec<&str> = Vec::new();
        for (file, at) in &known {
            let hits = per_file
                .get_mut(file.as_str())
                .expect("`known` was partitioned by membership in this very map");
            *hits += 1;
            matched.push(at);
        }
        let wrong: Vec<String> = per_file
            .iter()
            .filter(|(_, hits)| **hits != 1)
            .map(|(file, hits)| format!("{file}: {hits} matching line(s), expected exactly 1"))
            .collect();
        assert!(
            wrong.is_empty(),
            "the registered-cut list no longer describes the tree:\n  {}\n\
             0 means that site was migrated onto `production_prefix` — delete \
             its row, the list may only shrink and it can only shrink if \
             someone is made to notice. More than 1 means a registered file \
             grew a SECOND hand-rolled cut: migrate that one, do not let the \
             file's existing row cover it. Matched lines were: {matched:?}",
            wrong.join("\n  ")
        );
    }
    /// The detector, on both shapes and on the prose it must not flag.
    ///
    /// Byte-for-byte the cases `aleph-panel`'s twin carries, so a change to one
    /// copy that is not made to the other shows up as a diff between two test
    /// bodies rather than as silence. See `opens_a_cfg_test_literal`'s doc for
    /// why there are two copies at all.
    #[test]
    fn the_cfg_test_literal_detector_reads_the_offset_not_the_method() {
        assert!(opens_a_cfg_test_literal(r##"    .split("#[cfg(test)]")"##));
        assert!(opens_a_cfg_test_literal(
            r##"    let p = src.split("#[cfg(test)]\nmod ").next();"##
        ));
        assert!(opens_a_cfg_test_literal(
            r##"    .or_else(|| body[1..].find("\n#[cfg(test)]"))"##
        ));
        assert!(opens_a_cfg_test_literal(
            r##"    .position(|l| l.starts_with("#[cfg(test)]"))"##
        ));
        // Prose and fixtures: the attribute is inside the literal, not at its
        // start. Flagging these would make the guard an allowlist maintenance
        // job within a round.
        assert!(!opens_a_cfg_test_literal(
            r##"    "the #[cfg(test)] split matched nothing — this test would be","##
        ));
        assert!(!opens_a_cfg_test_literal(
            r##"    let src = "pub fn a() {}\n#[cfg(test)]\nmod t {}";"##
        ));
        assert!(!opens_a_cfg_test_literal("#[cfg(test)]"));
        assert!(!opens_a_cfg_test_literal("mod tests {"));
    }
}
