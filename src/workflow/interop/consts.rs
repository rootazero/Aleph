//! Bounded JS *data-literal* resolution for the bare `.workflow.js` scan.
//!
//! The Claude-Code workflow engineering format hoists schemas as top-level
//! `const NAME_SCHEMA = { … }` declarations and references them by name
//! (`agent(prompt, { schema: AUDIT_SCHEMA })`). Those literals are written in
//! JS surface syntax — **bare object keys, single quotes, trailing commas** —
//! which is *not* valid JSON, so importing them faithfully needs a normaliser.
//!
//! This module is that normaliser, and it is deliberately **not** a JS engine
//! or a general JS-subset parser (R3). It recognises only the JSON value grammar
//! (`object` / `array` / `string` / `number` / `bool` / `null`) written with the
//! JS-lax surface above. Any value that is not pure data — an identifier, a
//! function call, a template string, a computed `[key]`, an arithmetic
//! expression — makes the whole literal *abstain* (`None`). Nothing is ever
//! evaluated, interpolated, or guessed; a schema that abstains is surfaced as a
//! `dropped` diagnostic by [`super::import`], never silently lost.

use std::collections::HashMap;

use serde_json::{Map, Number, Value};

/// Recursion cap for nested `{}` / `[]`, mirroring `serde_json`'s default
/// `RECURSION_LIMIT` (128). A literal nested deeper abstains (`None`) rather than
/// risking a stack overflow — the old raw-text + `serde_json::from_str` path was
/// implicitly bounded the same way.
const MAX_DEPTH: u32 = 128;

/// Symbol table of top-level `const NAME = <data-literal>` declarations, mapping
/// the name to its normalised JSON value. Only pure-data consts appear; a const
/// bound to an expression / function / template is absent (it abstained).
pub(crate) type ConstTable = HashMap<String, Value>;

/// Scan `src` for top-level `const NAME = <data-literal>` declarations and
/// return the resolvable ones as a [`ConstTable`]. String-aware: a `const`
/// appearing inside a prompt string is never matched. First declaration of a
/// name wins (JS forbids re-declaring a `const` in one scope; abstaining on
/// later shadows is the safe default).
pub(crate) fn collect_consts(src: &str) -> ConstTable {
    let chars: Vec<char> = src.chars().collect();
    let n = chars.len();
    let mut table = ConstTable::new();
    let mut i = 0;
    while i < n {
        let c = chars[i];
        // Skip string-literal bodies so a `const` mentioned inside a prompt does
        // not register as a declaration.
        if c == '\'' || c == '"' || c == '`' {
            i = skip_string(&chars, i);
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < n && is_ident_char(chars[i]) {
                i += 1;
            }
            if chars[start..i].iter().collect::<String>() == "const" {
                if let Some((name, val)) = read_const_decl(&chars, i) {
                    table.entry(name).or_insert(val);
                }
            }
            continue;
        }
        i += 1;
    }
    table
}

/// Read `NAME = <data-literal>` starting just after the `const` keyword. Returns
/// `None` unless the value is a pure data literal (see [`parse_js_data`]).
fn read_const_decl(chars: &[char], after_const: usize) -> Option<(String, Value)> {
    let mut i = first_non_ws(chars, after_const);
    let name_start = i;
    while i < chars.len() && is_ident_char(chars[i]) {
        i += 1;
    }
    if i == name_start {
        return None;
    }
    let name: String = chars[name_start..i].iter().collect();
    i = first_non_ws(chars, i);
    // Require a single `=`; `==` / `=>` are not a const initialiser.
    if chars.get(i) != Some(&'=') || matches!(chars.get(i + 1), Some('=' | '>')) {
        return None;
    }
    let (val, _next) = parse_js_data(chars, i + 1)?;
    Some((name, val))
}

/// Parse a single JS *data literal* beginning at or after `chars[start]`,
/// returning the normalised [`Value`] and the index just past it. Returns `None`
/// (abstains) for anything that is not pure data — the caller then treats the
/// value as dynamic. Never evaluates or interpolates (R3).
pub(crate) fn parse_js_data(chars: &[char], start: usize) -> Option<(Value, usize)> {
    parse_value(chars, start, 0)
}

/// Depth-tracked core of [`parse_js_data`]. `depth` bounds nesting so a
/// pathologically deep literal abstains instead of overflowing the stack.
fn parse_value(chars: &[char], start: usize, depth: u32) -> Option<(Value, usize)> {
    if depth > MAX_DEPTH {
        return None;
    }
    let i = first_non_ws(chars, start);
    match chars.get(i)? {
        '{' => parse_object(chars, i, depth),
        '[' => parse_array(chars, i, depth),
        '\'' | '"' => read_string(chars, i).map(|(s, next)| (Value::String(s), next)),
        't' | 'f' => parse_bool(chars, i),
        'n' => parse_keyword(chars, i, "null").map(|next| (Value::Null, next)),
        c if *c == '-' || c.is_ascii_digit() => parse_number(chars, i),
        // Identifier, `(`, backtick template, spread `...`, computed key — not
        // data. Abstain so nothing is guessed.
        _ => None,
    }
}

/// Parse `{ key: value, … }` with bare or quoted keys and tolerant commas.
/// Abstains on a computed `[key]`, a non-data value, or a malformed shape.
fn parse_object(chars: &[char], start: usize, depth: u32) -> Option<(Value, usize)> {
    let n = chars.len();
    let mut i = start + 1; // past '{'
    let mut map = Map::new();
    loop {
        i = first_non_ws(chars, i);
        match chars.get(i)? {
            '}' => return Some((Value::Object(map), i + 1)),
            ',' => {
                i += 1;
                continue;
            }
            _ => {}
        }
        // Key: a quoted string or a bare identifier. A computed `[key]`, a
        // numeric key, or a spread makes the object non-static → abstain.
        let key = match chars.get(i)? {
            '\'' | '"' => {
                let (k, next) = read_string(chars, i)?;
                i = next;
                k
            }
            c if c.is_alphabetic() || *c == '_' || *c == '$' => {
                let ks = i;
                while i < n && is_ident_char(chars[i]) {
                    i += 1;
                }
                chars[ks..i].iter().collect()
            }
            _ => return None,
        };
        i = first_non_ws(chars, i);
        if chars.get(i) != Some(&':') {
            return None;
        }
        let (val, next) = parse_value(chars, i + 1, depth + 1)?;
        map.insert(key, val);
        i = first_non_ws(chars, next);
        match chars.get(i)? {
            ',' => i += 1,
            '}' => return Some((Value::Object(map), i + 1)),
            _ => return None,
        }
    }
}

/// Parse `[ value, … ]` with tolerant commas. Abstains on any non-data element.
fn parse_array(chars: &[char], start: usize, depth: u32) -> Option<(Value, usize)> {
    let mut i = start + 1; // past '['
    let mut arr = Vec::new();
    loop {
        i = first_non_ws(chars, i);
        match chars.get(i)? {
            ']' => return Some((Value::Array(arr), i + 1)),
            ',' => {
                i += 1;
                continue;
            }
            _ => {}
        }
        let (val, next) = parse_value(chars, i, depth + 1)?;
        arr.push(val);
        i = first_non_ws(chars, next);
        match chars.get(i)? {
            ',' => i += 1,
            ']' => return Some((Value::Array(arr), i + 1)),
            _ => return None,
        }
    }
}

/// Read a single- or double-quoted string literal at `chars[start]`, decoding
/// the common JS escapes (`\n`/`\t`/`\r`/`\0`; any other escape → the char
/// verbatim). This is the same accepted set as `import::read_literal_at` — the
/// rarer `\b`/`\f`/`\uXXXX` forms decode lossily on this best-effort bare path;
/// the `@aleph-workflow` embed header is the byte-exact round-trip guarantee.
/// Returns the content and the index past the close quote.
fn read_string(chars: &[char], start: usize) -> Option<(String, usize)> {
    let quote = *chars.get(start)?;
    if quote != '\'' && quote != '"' {
        return None;
    }
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

/// Parse the `true` / `false` keyword as a whole token.
fn parse_bool(chars: &[char], start: usize) -> Option<(Value, usize)> {
    if let Some(next) = parse_keyword(chars, start, "true") {
        return Some((Value::Bool(true), next));
    }
    parse_keyword(chars, start, "false").map(|next| (Value::Bool(false), next))
}

/// Match the exact keyword `kw` at `start`, requiring it not run into a longer
/// identifier (`trueish` must not match `true`). Returns the index past it.
fn parse_keyword(chars: &[char], start: usize, kw: &str) -> Option<usize> {
    let kw: Vec<char> = kw.chars().collect();
    for (k, &ch) in kw.iter().enumerate() {
        if chars.get(start + k) != Some(&ch) {
            return None;
        }
    }
    let after = start + kw.len();
    match chars.get(after) {
        Some(c) if is_ident_char(*c) => None,
        _ => Some(after),
    }
}

/// Parse a JSON number token (`-`? digits, optional fraction / exponent).
fn parse_number(chars: &[char], start: usize) -> Option<(Value, usize)> {
    let n = chars.len();
    let mut i = start;
    if chars.get(i) == Some(&'-') {
        i += 1;
    }
    let digits_start = i;
    while i < n && (chars[i].is_ascii_digit() || matches!(chars[i], '.' | 'e' | 'E' | '+' | '-')) {
        i += 1;
    }
    if i == digits_start {
        return None;
    }
    let raw: String = chars[start..i].iter().collect();
    // Prefer exact integer representations (i64 then u64) so a large unsigned
    // schema value keeps full precision, matching `serde_json`'s number model;
    // fall back to f64 only for fractional / exponent forms.
    let num = raw
        .parse::<i64>()
        .map(Number::from)
        .ok()
        .or_else(|| raw.parse::<u64>().ok().map(Number::from))
        .or_else(|| raw.parse::<f64>().ok().and_then(Number::from_f64))?;
    Some((Value::Number(num), i))
}

/// Advance past a string literal beginning at `chars[start]` (a quote of any
/// JS kind). Returns the index just past the close; unterminated → end of input.
fn skip_string(chars: &[char], start: usize) -> usize {
    let quote = chars[start];
    let n = chars.len();
    let mut i = start + 1;
    while i < n {
        let c = chars[i];
        if c == '\\' {
            i += 2;
            continue;
        }
        i += 1;
        if c == quote {
            break;
        }
    }
    i
}

/// Index of the first non-whitespace char at or after `start` (clamped to len).
fn first_non_ws(chars: &[char], start: usize) -> usize {
    let mut i = start;
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    i
}

/// A char that may appear in a JS identifier body (`$` included).
fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Option<Value> {
        let chars: Vec<char> = src.chars().collect();
        parse_js_data(&chars, 0).map(|(v, _)| v)
    }

    #[test]
    fn normalises_js_object_literal_to_json() {
        // Bare keys, single quotes, trailing comma — the reference schema shape.
        let v = parse("{ type: 'object', additionalProperties: false, required: ['lens', 'gaps'], }")
            .expect("pure-data JS literal normalises");
        assert_eq!(v["type"], "object");
        assert_eq!(v["additionalProperties"], false);
        assert_eq!(v["required"], serde_json::json!(["lens", "gaps"]));
    }

    #[test]
    fn parses_valid_json_too() {
        // JSON is a subset — double quotes / no trailing comma still parse.
        let v = parse(r#"{"a": 1, "b": [true, null, -2.5]}"#).expect("json parses");
        assert_eq!(v, serde_json::json!({"a": 1, "b": [true, null, -2.5]}));
    }

    #[test]
    fn nested_schema_properties_round_trip() {
        let v = parse(
            "{ properties: { lens: { type: 'string' }, gaps: { type: 'array', items: { type: 'object' } } } }",
        )
        .expect("nested normalises");
        assert_eq!(v["properties"]["lens"]["type"], "string");
        assert_eq!(v["properties"]["gaps"]["items"]["type"], "object");
    }

    #[test]
    fn abstains_on_expression_value() {
        // `key: someVar` / a function call / concatenation is not pure data.
        assert!(parse("{ type: someVar }").is_none());
        assert!(parse("{ items: buildItems() }").is_none());
        assert!(parse("{ msg: 'a' + x }").is_none());
    }

    #[test]
    fn abstains_on_template_and_computed_key() {
        assert!(parse("{ msg: `hi ${name}` }").is_none());
        assert!(parse("{ [dynamicKey]: 1 }").is_none());
    }

    #[test]
    fn abstains_on_bare_identifier() {
        assert!(parse("AUDIT_SCHEMA").is_none());
        assert!(parse("(args && args.x)").is_none());
    }

    #[test]
    fn collect_consts_gathers_data_literals_only() {
        let src = r#"
export const meta = { name: 'x' }
const AUDIT_SCHEMA = { type: 'object', required: ['lens'], }
const cfg = (args && typeof args === 'object') ? args : {}
const subsystem = cfg.subsystem || 'workflow'
const TAGS = ['a', 'b']
const buildPrompt = (u) => 'p ' + u
"#;
        let table = collect_consts(src);
        // Pure-data consts are captured…
        assert!(table.contains_key("AUDIT_SCHEMA"));
        assert_eq!(table["AUDIT_SCHEMA"]["type"], "object");
        assert_eq!(table["TAGS"], serde_json::json!(["a", "b"]));
        // `meta` is a data object too (it is a `const` = { … }).
        assert!(table.contains_key("meta"));
        // …expression / function / member-access consts abstain.
        assert!(!table.contains_key("cfg"));
        assert!(!table.contains_key("subsystem"));
        assert!(!table.contains_key("buildPrompt"));
    }

    #[test]
    fn collect_consts_ignores_const_inside_strings() {
        let src = "const REAL = { a: 1 }\nawait agent('const FAKE = { b: 2 } in a prompt')";
        let table = collect_consts(src);
        assert!(table.contains_key("REAL"));
        assert!(!table.contains_key("FAKE"), "a const inside a prompt is not a declaration");
    }

    #[test]
    fn first_declaration_wins() {
        let table = collect_consts("const X = { v: 1 }\nconst X = { v: 2 }");
        assert_eq!(table["X"]["v"], 1);
    }

    #[test]
    fn deeply_nested_literal_abstains_past_depth_cap() {
        // Past MAX_DEPTH a literal abstains (None) instead of overflowing the
        // stack — the old raw-text + serde_json path was bounded the same way.
        let deep = format!("{}{}", "[".repeat(200), "]".repeat(200));
        assert!(parse(&deep).is_none(), "over-deep nesting abstains");
        // A shallow literal still parses.
        assert!(parse("[[[[[1]]]]]").is_some(), "shallow nesting still parses");
    }

    #[test]
    fn large_unsigned_and_float_numbers_keep_precision() {
        // u64 beyond i64::MAX keeps exact integer precision (not lossy f64).
        let v = parse("18446744073709551615").expect("u64 parses");
        assert_eq!(v, serde_json::json!(18_446_744_073_709_551_615u64));
        // Fractional / exponent forms fall back to f64.
        assert_eq!(parse("-2.5").unwrap(), serde_json::json!(-2.5));
    }
}
