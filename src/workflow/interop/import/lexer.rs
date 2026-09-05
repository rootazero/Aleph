//! Hand-rolled readers over a `.workflow.js` source: JS string literals, the
//! argument shapes `agent()` / `clarify()` accept, and the two whole-source
//! rewrites (`blank_comments`, `strip_string_literals`) the scan runs first.
//!
//! No JS engine and no parser (R3) — every reader either recognises a *static
//! literal* or abstains. Abstaining is what keeps the import honest: a reader
//! that guessed would produce a step nobody authored, and the caller could not
//! tell the difference. Nothing here knows what a workflow is; the meaning of
//! a recovered literal is decided one layer up, in [`super::scan`] and
//! [`super`].

/// Blank every JS comment (`//` to end of line, `/* … */`) so the bare scan
/// sees only code. String-aware, so a `//` inside a prompt survives untouched.
/// Comment bodies become spaces and newlines are preserved, keeping the line
/// structure the rest of the scan reads.
///
/// Without this the scanner had no notion of comments at all, and two ordinary
/// files broke it silently:
/// - `// don't forget the schema` — the apostrophe opened a phantom string
///   literal that ran to the next quote in the file, inverting quote parity for
///   everything after it. Every `agent()` call downstream vanished and the user
///   was told "no agent() calls found".
/// - `// await agent('old first pass')` — a deliberately commented-out step
///   imported as a live one.
///
/// Only reachable on the hand-written path: [`super::extract_embedded`] runs first,
/// and the `@aleph-workflow` header is itself a block comment.
pub(super) fn blank_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' | '"' | '`' => {
                out.push(c);
                while let Some(d) = chars.next() {
                    out.push(d);
                    if d == '\\' {
                        if let Some(esc) = chars.next() {
                            out.push(esc);
                        }
                        continue;
                    }
                    if d == c {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'/') => {
                out.push_str("  ");
                chars.next();
                for d in chars.by_ref() {
                    if d == '\n' {
                        out.push('\n');
                        break;
                    }
                    out.push(' ');
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                out.push_str("  ");
                chars.next();
                let mut prev_star = false;
                for d in chars.by_ref() {
                    // Newlines survive so line-oriented reads stay sane.
                    out.push(if d == '\n' { '\n' } else { ' ' });
                    if prev_star && d == '/' {
                        break;
                    }
                    prev_star = d == '*';
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// Read a single- or double-quoted string literal that is the *first
/// non-whitespace token* of `s` (UTF-8 safe, honours backslash escapes by
/// keeping the escaped char verbatim).
///
/// Requiring the literal to lead — rather than scanning arbitrarily far ahead —
/// keeps a non-literal argument (`agent(promptVar)`, `meta: { name: foo }`)
/// from silently capturing an *unrelated* later string elsewhere in the source.
/// Both real callers (a `meta.<field>:` value and an `agent(` first argument)
/// place the literal immediately after optional whitespace, so this is the
/// correct shape, not just a safer one.
pub(super) fn read_first_string_literal(s: &str) -> Option<String> {
    let chars: Vec<char> = s.chars().collect();
    read_first_string_literal_chars(&chars)
}

/// Char-slice core of [`read_first_string_literal`]: the leading token of
/// `chars` must be a quote, else `None` (no over-reach to a later literal).
pub(super) fn read_first_string_literal_chars(chars: &[char]) -> Option<String> {
    let i = first_non_ws(chars, 0);
    match chars.get(i).copied() {
        Some('\'' | '"') => read_literal_at(chars, i).map(|(lit, _)| lit),
        _ => None,
    }
}

/// Index of the first non-whitespace char at or after `start` (clamped to len).
pub(super) fn first_non_ws(chars: &[char], start: usize) -> usize {
    let mut i = start;
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    i
}

/// Read the quoted string literal beginning at `chars[start]` (which must be a
/// quote). Returns the decoded content and the index just past the closing
/// quote. Standard JS escapes are interpreted (`\n`/`\t`/`\r`/`\0` → the control
/// char; `\"`/`\'`/`\\`/`\/` and any other escape → the char verbatim) so a
/// `.join("\n")` separator decodes to a real newline — the round-trip inverse of
/// `export`'s `js_str`. UTF-8 safe (operates on `char`s).
pub(super) fn read_literal_at(chars: &[char], start: usize) -> Option<(String, usize)> {
    let quote = *chars.get(start)?;
    let mut i = start + 1;
    let mut out = String::new();
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' {
            let esc = *chars.get(i + 1)?;
            out.push(match esc {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                '0' => '\0',
                other => other,
            });
            i += 2;
            continue;
        }
        if c == quote {
            return Some((out, i + 1));
        }
        out.push(c);
        i += 1;
    }
    None
}

/// Read the prompt argument of an `agent(` call from `s` (the source *after* the
/// open paren). Handles the two declarative prompt shapes of the engineering
/// format:
///   * `agent("prompt", …)`              → the single string literal;
///   * `agent([ "a", "b" ].join("\n"), …)` → the array elements joined by the
///     `.join` separator (the format's signature multi-line idiom).
///
/// Returns `None` for a non-literal argument (`agent(promptVar)`, a `.map(...)`
/// expression, or an array with a non-literal element) — those are dynamic and
/// intentionally not statically importable (R7/R10); they surface elsewhere as
/// `dropped` constructs rather than being half-captured.
///
/// On success the second tuple element is the char index just past the prompt
/// argument, so the caller can continue into the `, { opts }` object.
pub(super) fn read_agent_prompt(chars: &[char], start: usize) -> Option<(String, usize)> {
    let i = first_non_ws(chars, start);
    match *chars.get(i)? {
        '\'' | '"' => read_literal_at(chars, i),
        '[' => read_joined_array(chars, i),
        _ => None,
    }
}

/// Parse `[ "a", "b", … ].join("sep")` starting at `chars[start] == '['`.
/// Every element must be a plain string literal; a non-literal element or an
/// element-level concatenation (`'a' + x`) makes the joined value not statically
/// known, so the whole read abstains (returns `None`). The separator defaults to
/// `"\n"` (the format's convention) when no explicit `.join(...)` follows.
///
/// The second tuple element is the index just past the array (and its `.join`,
/// if any), so the caller can resume scanning the agent opts.
fn read_joined_array(chars: &[char], start: usize) -> Option<(String, usize)> {
    let n = chars.len();
    let mut i = start + 1; // past '['
    let mut parts: Vec<String> = Vec::new();
    loop {
        while i < n && (chars[i].is_whitespace() || chars[i] == ',') {
            i += 1;
        }
        match *chars.get(i)? {
            ']' => {
                i += 1;
                break;
            }
            '\'' | '"' => {
                let (lit, next) = read_literal_at(chars, i)?;
                // `'a' + x` concatenation → joined string not statically known.
                if chars.get(first_non_ws(chars, next)) == Some(&'+') {
                    return None;
                }
                parts.push(lit);
                i = next;
            }
            // Identifier / expression element → dynamic array, abstain.
            _ => return None,
        }
    }
    // `i` now points just past `]`. An optional `.join("sep")` follows; absent
    // it, the convention separator is `"\n"` and the array ends at `]`.
    let (sep, end) = parse_join_separator(chars, i).unwrap_or_else(|| ("\n".to_string(), i));
    Some((parts.join(&sep), end))
}

/// After a `]`, read an optional `.join("sep")` and return the separator literal
/// plus the index just past the closing `)`. `None` if no well-formed
/// `.join(<string literal>)` follows.
fn parse_join_separator(chars: &[char], start: usize) -> Option<(String, usize)> {
    let i = first_non_ws(chars, start);
    // Match the `.join` identifier exactly.
    let dotjoin = ['.', 'j', 'o', 'i', 'n'];
    if (0..dotjoin.len()).any(|k| chars.get(i + k) != Some(&dotjoin[k])) {
        return None;
    }
    let i = first_non_ws(chars, i + dotjoin.len());
    if chars.get(i) != Some(&'(') {
        return None;
    }
    let i = first_non_ws(chars, i + 1);
    let (sep, next) = match *chars.get(i)? {
        '\'' | '"' => read_literal_at(chars, i)?,
        _ => return None,
    };
    // Consume the closing `)` so the returned end index is past the whole
    // `.join(...)` call, not stranded mid-expression.
    let j = first_non_ws(chars, next);
    if chars.get(j) != Some(&')') {
        return None;
    }
    Some((sep, j + 1))
}

/// Read an optional `, ["a", "b"]` choices array that follows a clarify
/// question. `start` is the index just past the prompt argument. Returns the
/// decoded choice literals, or an empty vec when no array follows (a free-text
/// clarification) — the inverse of `export`'s `render_clarify_call`. A
/// non-literal element makes the menu dynamic, so the whole read abstains
/// (returns empty) rather than half-capturing it (R7/R10), exactly like the
/// prompt readers.
pub(super) fn read_clarify_choices(chars: &[char], start: usize) -> Vec<String> {
    let i = first_non_ws(chars, start);
    if chars.get(i) != Some(&',') {
        return Vec::new();
    }
    let i = first_non_ws(chars, i + 1);
    if chars.get(i) != Some(&'[') {
        return Vec::new();
    }
    let n = chars.len();
    let mut j = i + 1; // past '['
    let mut out: Vec<String> = Vec::new();
    loop {
        while j < n && (chars[j].is_whitespace() || chars[j] == ',') {
            j += 1;
        }
        match chars.get(j) {
            Some(']') | None => break,
            Some('\'' | '"') => match read_literal_at(chars, j) {
                Some((lit, next)) => {
                    out.push(lit);
                    j = next;
                }
                None => break,
            },
            // Identifier / expression element → dynamic menu, abstain entirely.
            _ => {
                out.clear();
                return out;
            }
        }
    }
    out
}

/// Skip a value at the current opts cursor, stopping at the next top-level `,` or
/// the object's closing `}`. Nested `{}`/`[]`/`()` and string literals are
/// skipped wholesale so an inner separator never ends the value early. Returns
/// the index of the stopping delimiter.
pub(super) fn skip_value(chars: &[char], start: usize) -> usize {
    let n = chars.len();
    let mut i = start;
    let mut depth: i32 = 0;
    while i < n {
        let c = chars[i];
        match c {
            '\'' | '"' | '`' => {
                i += 1;
                while i < n {
                    let d = chars[i];
                    if d == '\\' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    if d == c {
                        break;
                    }
                }
            }
            '{' | '[' | '(' => {
                depth += 1;
                i += 1;
            }
            '}' | ']' | ')' => {
                if depth == 0 {
                    return i; // the opts object's closing brace
                }
                depth -= 1;
                i += 1;
            }
            ',' if depth == 0 => return i,
            _ => i += 1,
        }
    }
    i
}

/// Blank out the contents of every string literal (`'`, `"`, `` ` ``) so a
/// downstream keyword scan sees only the code skeleton, never prompt text.
/// Quote delimiters and surrounding code are preserved; an escaped quote
/// inside a literal does not terminate it. UTF-8 safe (iterates `chars`);
/// an unterminated literal degrades to dropping the trailing bytes.
pub(super) fn strip_string_literals(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' | '"' | '`' => {
                out.push(c);
                while let Some(d) = chars.next() {
                    if d == '\\' {
                        // Skip the escaped char so e.g. \" does not close early;
                        // its body is irrelevant to the skeleton, so drop it.
                        chars.next();
                        continue;
                    }
                    if d == c {
                        out.push(d); // keep the closing delimiter
                        break;
                    }
                    // literal body intentionally dropped
                }
            }
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_string_literals_blanks_bodies_keeps_skeleton() {
        // Bodies gone, delimiters + code kept; escaped quote does not close early.
        assert_eq!(
            strip_string_literals("for (x) agent('a b')"),
            "for (x) agent('')"
        );
        assert_eq!(strip_string_literals(r#"f("a \" b")"#), r#"f("")"#);
        assert_eq!(strip_string_literals("agent(`tpl text`)"), "agent(``)");
    }
}
