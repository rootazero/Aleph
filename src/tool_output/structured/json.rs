//! JSON tool-result reducer: shrink a large JSON document (API response, config
//! dump, structured tool output) while preserving its *shape* and its salient
//! short fields. The signal in a JSON blob is the key structure and the small
//! scalar values (`error`, `status`, `message`, ids) — not the multi-kilobyte
//! string leaves or the hundredth element of an array. First-line truncation
//! destroys JSON outright (it cuts mid-structure into invalid syntax); a
//! structural reduction keeps it parseable and legible.
//!
//! Unlike the log/search/diff reducers (pure line processing), this one parses
//! with `serde_json` — already the project-wide serialization stack (R3: not a
//! new dependency), and the type-safe `Value` tree is far more robust than the
//! manual string-surgery the TS/Python references resort to. Recursion is depth
//! bounded (P7 defensive design) so a pathologically nested blob can't blow the
//! stack.

use serde_json::Value;

use super::{ContentKind, Reduction};

/// String leaves longer than this (chars) are truncated to a head + a
/// `…(+N chars)` marker. 200 chars keeps an error message or short snippet
/// intact while shedding embedded file bodies / base64 / HTML.
const MAX_STRING_CHARS: usize = 200;
/// Arrays longer than this keep their first N elements plus a `…(+M more …)`
/// marker element. The head of a result list carries the shape; the tail is
/// usually homogeneous repetition.
const MAX_ARRAY_ELEMS: usize = 8;
/// Objects wider than this keep their first N keys plus a `"…": "(+M more keys)"`
/// marker entry. Generous compared to the array cap because an object's key set
/// *is* its shape, and short scalars like `error` / `status` / `message` are
/// exactly what must survive.
const MAX_OBJECT_KEYS: usize = 48;
/// Defensive recursion bound: beyond this depth a subtree collapses to a
/// placeholder rather than recursing further (guards against adversarial /
/// cyclic-looking deeply nested input — `serde_json` itself caps parse depth,
/// this caps our walk).
const MAX_DEPTH: usize = 16;

/// Cheap whole-text gate: the body is a single brace- or bracket-delimited JSON
/// document. The real parse (and the decision to actually reduce) happens in
/// [`reduce_json`]; keeping this detector allocation-free mirrors the other
/// reducers' `looks_like_*` contract.
pub(super) fn looks_like_json(lines: &[&str]) -> bool {
    let first = lines
        .iter()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim());
    let last = lines
        .iter()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim());
    match (first, last) {
        (Some(f), Some(l)) => {
            (f.starts_with('{') && l.ends_with('}')) || (f.starts_with('[') && l.ends_with(']'))
        }
        _ => false,
    }
}

/// Parse the body and return a structurally-reduced, re-serialized version.
///
/// Returns `None` when the body does not parse as JSON (classification is a
/// cheap heuristic — a malformed blob falls back to first-line truncation), or
/// when nothing was oversized (all signal — not worth a header), or when the
/// reduced form is not actually smaller than the input.
pub(super) fn reduce_json(text: &str) -> Option<Reduction> {
    let trimmed = text.trim();
    let body = match serde_json::from_str::<Value>(trimmed) {
        Ok(value) => {
            let (reduced, changed) = shrink(&value, 0);
            if !changed {
                return None;
            }
            serde_json::to_string_pretty(&reduced).ok()?
        }
        // Not one document — try ndjson / JSONL, which is what `rg --json`,
        // `docker events` and `kubectl -o json --watch` all emit. The cheap
        // `looks_like_json` gate (first line `{`, last line `}`) matches these,
        // so before this arm existed they claimed the JSON slot, failed to parse,
        // and — because `reduce` dispatched on one kind only — got no reduction
        // from any reducer at all.
        Err(_) => reduce_jsonl(trimmed)?,
    };
    // Whether the result is actually smaller is decided centrally, by
    // `Reduction::is_meaningful_shrink`; a local `>=` check here once let a
    // 91-byte saving on a 91 KB document pass as a reduction.
    // JSON tallies are chars, not lines: the body is re-pretty-printed, so
    // its line count is unrelated to the input's (a dense blob re-renders as
    // dozens of lines and the old "kept 43/1 lines" header lied). See the
    // unit note on [`Reduction::kept_lines`] and the char-unit header arm in
    // `Reduction::render`.
    let kept_chars = body.chars().count();
    Some(Reduction {
        kind: ContentKind::Json,
        body,
        kept_lines: kept_chars,
        total_lines: trimmed.chars().count(),
    })
}

/// Max records kept from a JSONL / ndjson stream, plus a tail marker. The head
/// of such a stream carries its shape; `rg --json` in particular repeats one
/// record type per hit.
const MAX_JSONL_RECORDS: usize = 12;

/// Reduce a newline-delimited JSON stream: every non-empty line must parse on its
/// own, or this isn't JSONL and we decline. Each kept record goes through the
/// same [`shrink`] used for a single document, re-emitted one compact record per
/// line so the result is still machine-readable.
fn reduce_jsonl(trimmed: &str) -> Option<String> {
    let mut records = Vec::new();
    let mut total = 0usize;
    for line in trimmed.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line).ok()?;
        total += 1;
        if records.len() < MAX_JSONL_RECORDS {
            let (reduced, _) = shrink(&value, 0);
            records.push(serde_json::to_string(&reduced).ok()?);
        }
    }
    if total < 2 {
        return None; // a single record is just a document; not our case
    }
    let mut body = records.join("\n");
    if total > records.len() {
        body.push_str(&format!("\n…(+{} more records)", total - records.len()));
    }
    Some(body)
}

/// Recursively shrink a JSON value, returning the reduced value and whether
/// anything was actually dropped.
///
/// - Strings over [`MAX_STRING_CHARS`] are head-truncated with a char count.
/// - Arrays over [`MAX_ARRAY_ELEMS`] keep their head plus an omission marker.
/// - Objects keep every key (the key set is the structural signal — short
///   scalars like `error`/`status`/`message` survive untouched) and recurse
///   into each value.
/// - Numbers / booleans / null are already tiny and pass through unchanged.
fn shrink(value: &Value, depth: usize) -> (Value, bool) {
    if depth >= MAX_DEPTH {
        return (Value::String("…(depth limit)".into()), true);
    }
    match value {
        Value::String(s) => {
            let n = s.chars().count();
            if n > MAX_STRING_CHARS {
                let head: String = s.chars().take(MAX_STRING_CHARS).collect();
                (
                    Value::String(format!("{head}…(+{} chars)", n - MAX_STRING_CHARS)),
                    true,
                )
            } else {
                (value.clone(), false)
            }
        }
        Value::Array(items) => {
            let mut changed = items.len() > MAX_ARRAY_ELEMS;
            let mut out: Vec<Value> = Vec::new();
            for item in items.iter().take(MAX_ARRAY_ELEMS) {
                let (v, c) = shrink(item, depth + 1);
                changed |= c;
                out.push(v);
            }
            if items.len() > MAX_ARRAY_ELEMS {
                out.push(Value::String(format!(
                    "…(+{} more items)",
                    items.len() - MAX_ARRAY_ELEMS
                )));
            }
            (Value::Array(out), changed)
        }
        Value::Object(map) => {
            // Keys are the structural signal, so they are kept — but not without
            // limit. A document that is large because it is *wide* (a lockfile,
            // `cargo metadata`, `npm ls --json`, a locale bundle: thousands of
            // short keys, no leaf over MAX_STRING_CHARS) set `changed = false`
            // all the way up and so was the one class of oversized JSON the
            // reducer structurally refused to touch. Cap parallel to the array
            // arm above.
            let mut changed = map.len() > MAX_OBJECT_KEYS;
            let mut out = serde_json::Map::new();
            for (k, v) in map.iter().take(MAX_OBJECT_KEYS) {
                let (nv, c) = shrink(v, depth + 1);
                changed |= c;
                out.insert(k.clone(), nv);
            }
            if map.len() > MAX_OBJECT_KEYS {
                out.insert(
                    "…".to_string(),
                    Value::String(format!("(+{} more keys)", map.len() - MAX_OBJECT_KEYS)),
                );
            }
            (Value::Object(out), changed)
        }
        _ => (value.clone(), false),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{classify, reduce, ContentKind};
    use super::*;

    #[test]
    fn detects_pretty_json_object() {
        let s = "{\n  \"a\": 1,\n  \"b\": 2,\n  \"c\": 3,\n  \"d\": 4,\n  \"e\": 5,\n  \"f\": 6,\n  \"g\": 7\n}";
        let lines: Vec<&str> = s.lines().collect();
        assert!(looks_like_json(&lines));
    }

    #[test]
    fn truncates_long_string_leaf_and_preserves_short_keys() {
        let big = "x".repeat(1000);
        let s = format!(
            "{{\n  \"status\": \"error\",\n  \"message\": \"boom\",\n  \"body\": \"{big}\",\n  \"a\": 1,\n  \"b\": 2,\n  \"c\": 3,\n  \"d\": 4\n}}"
        );
        // Routes through the public classify/reduce dispatch (proves wiring).
        assert_eq!(classify(&s), Some(ContentKind::Json));
        let r = reduce(&s).expect("oversized JSON should reduce");
        assert_eq!(r.kind, ContentKind::Json);
        // Short, salient fields survive verbatim.
        assert!(r.body.contains("\"status\": \"error\""), "got:\n{}", r.body);
        assert!(r.body.contains("\"message\": \"boom\""));
        // The huge leaf is truncated with a char-count marker, not dropped.
        assert!(r.body.contains("…(+800 chars)"), "got:\n{}", r.body);
        assert!(!r.body.contains(&"x".repeat(1000)));
        // Output is still valid JSON.
        assert!(serde_json::from_str::<serde_json::Value>(&r.body).is_ok());
    }

    #[test]
    fn caps_large_array() {
        let elems: Vec<String> = (0..50).map(|i| format!("\"item-{i}\"")).collect();
        let s = format!("[\n  {}\n]", elems.join(",\n  "));
        let r = reduce(&s).expect("large array should reduce");
        assert!(r.body.contains("…(+42 more items)"), "got:\n{}", r.body);
        assert!(serde_json::from_str::<serde_json::Value>(&r.body).is_ok());
    }

    #[test]
    fn header_tallies_chars_not_bogus_lines() {
        // A dense single-line blob: the old header claimed "kept N/1 lines"
        // with N = the pretty-printed body's line count (kept > total, wrong
        // unit). Chars are the honest unit for a re-serialized document.
        let big = "x".repeat(1000);
        let s = format!("{{\"status\": \"error\", \"body\": \"{big}\"}}");
        let r = reduce_json(&s).expect("oversized single-line JSON should reduce");
        let rendered = r.render();
        assert!(
            rendered.starts_with("[compacted json: reduced "),
            "got: {rendered}"
        );
        assert!(
            rendered.contains(" chars]"),
            "header unit must be chars; got: {rendered}"
        );
        // The tallies count what they report: original vs kept chars.
        assert_eq!(r.total_lines, s.chars().count());
        assert_eq!(r.kept_lines, r.body.chars().count());
        assert!(
            r.kept_lines < r.total_lines,
            "kept ({}) must be smaller than total ({})",
            r.kept_lines,
            r.total_lines
        );
    }

    #[test]
    fn all_small_json_not_worth_reducing() {
        // Multi-line but every value is tiny → nothing dropped → None, so the
        // caller keeps its existing handling.
        let s = "{\n  \"a\": 1,\n  \"b\": 2,\n  \"c\": 3,\n  \"d\": 4,\n  \"e\": 5,\n  \"f\": 6,\n  \"g\": 7,\n  \"h\": 8\n}";
        assert!(reduce_json(s).is_none());
    }

    #[test]
    fn malformed_json_returns_none() {
        let s = "{\n  not really: json,\n  missing quotes,\n  trailing,\n  a,\n  b,\n  c,\n  d\n}";
        assert!(reduce_json(s).is_none());
    }
}
