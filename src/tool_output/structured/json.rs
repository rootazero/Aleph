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

use super::{ContentKind, Profile, Reduction, Tally};

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
pub(super) fn reduce_json(text: &str, profile: &Profile) -> Option<Reduction> {
    let trimmed = text.trim();
    let body = match serde_json::from_str::<Value>(trimmed) {
        Ok(value) => {
            let (reduced, changed) = shrink(&value, 0, profile);
            if !changed {
                return None;
            }
            serialize_like(&reduced, trimmed)?
        }
        // Not one document — try ndjson / JSONL, which is what `rg --json`,
        // `docker events` and `kubectl -o json --watch` all emit. The cheap
        // `looks_like_json` gate (first line `{`, last line `}`) matches these,
        // so before this arm existed they claimed the JSON slot, failed to parse,
        // and — because `reduce` dispatched on one kind only — got no reduction
        // from any reducer at all.
        Err(_) => reduce_jsonl(trimmed, profile)?,
    };
    // Whether the result is actually smaller is decided centrally, by
    // `Reduction::is_meaningful_shrink`; a local `>=` check here once let a
    // 91-byte saving on a 91 KB document pass as a reduction.
    // JSON tallies are chars, not lines: the body is re-serialized, so its line
    // count is unrelated to the input's (a dense blob re-rendered as dozens of
    // lines and the old "kept 43/1 lines" header lied). [`Tally`] carries the
    // unit so `Reduction::render` cannot mismatch it.
    let kept_chars = body.chars().count();
    Some(Reduction {
        kind: ContentKind::Json,
        body,
        tally: Tally::Chars {
            kept: kept_chars,
            total: trimmed.chars().count(),
        },
    })
}

/// Re-serialize `value` at the *input's* density.
///
/// Density is a property of the input this reducer has no business changing, and
/// changing it defeated the reducer outright: `to_string_pretty` expands a
/// compact document roughly threefold, so a 90 KB single-line API response
/// re-rendered at ~250 KB, the central byte guard correctly rejected it as "not
/// smaller", and the model received a head/tail byte slice of JSON — after the
/// reducer had done all of the work. A pretty input still gets a pretty body:
/// there the expansion is already paid for and legibility is free.
fn serialize_like(value: &Value, input: &str) -> Option<String> {
    if input.contains('\n') {
        serde_json::to_string_pretty(value).ok()
    } else {
        serde_json::to_string(value).ok()
    }
}

/// Reduce a newline-delimited JSON stream: the head must parse record by record,
/// or this isn't JSONL and we decline. Each kept record goes through the same
/// [`shrink`] used for a single document, re-emitted one compact record per line
/// so the result is still machine-readable.
///
/// Parse work is bounded by [`Profile::jsonl_records`] plus one, not by the
/// stream's length. The obvious loop ran `serde_json::from_str` over **every**
/// line purely to count them — a `kubectl -o json --watch` capture or a
/// repo-wide `rg --json` is hundreds of thousands of records, and this runs
/// synchronously on the tool-result ingress path, so counting cost more than the
/// reduction saved. Past the sample each line is only counted, screened for a
/// document opener, and the last one is parsed in full: both ends of the stream
/// therefore anchor the "these are N records" claim the marker makes.
fn reduce_jsonl(trimmed: &str, profile: &Profile) -> Option<String> {
    let mut records = Vec::new();
    let mut total = 0usize;
    let mut last: Option<&str> = None;
    for line in trimmed.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        total += 1;
        last = Some(line);
        if records.len() < profile.jsonl_records {
            let value: Value = serde_json::from_str(line).ok()?;
            let (reduced, _) = shrink(&value, 0, profile);
            records.push(serde_json::to_string(&reduced).ok()?);
        } else if !line.starts_with('{') && !line.starts_with('[') {
            // Cheap screen only, but a line that cannot even open a document
            // means this was never a stream of them.
            return None;
        }
    }
    if total < 2 {
        return None; // a single record is just a document; not our case
    }
    if records.len() < total {
        // Anchor the tail as well as the head before claiming a record count.
        serde_json::from_str::<Value>(last?).ok()?;
    }
    let mut body = records.join("\n");
    if total > records.len() {
        body.push_str(&format!("\n…(+{} more records)", total - records.len()));
    }
    Some(body)
}

/// A leaf that is cheap to keep and usually the answer: a bounded string, a
/// number, a bool, or null. These are the `error` / `status` / `message` / id
/// fields the module doc promises survive untouched.
fn is_short_scalar(value: &Value, max_string_chars: usize) -> bool {
    match value {
        Value::String(s) => s.chars().count() <= max_string_chars,
        Value::Number(_) | Value::Bool(_) | Value::Null => true,
        _ => false,
    }
}

/// Recursively shrink a JSON value, returning the reduced value and whether
/// anything was actually dropped.
///
/// - Strings over [`Profile::json_string_chars`] are head-truncated with a char
///   count.
/// - Arrays over [`Profile::json_array_elems`] keep their head plus an omission
///   marker.
/// - Objects keep up to [`Profile::json_object_keys`] keys (the key set is the structural
///   signal) and recurse into each value. When the cap binds, short scalars like
///   `error` / `status` / `message` are admitted first — `serde_json::Map` is a
///   `BTreeMap` here, so taking the first N would take the alphabetically-first N.
/// - Numbers / booleans / null are already tiny and pass through unchanged.
fn shrink(value: &Value, depth: usize, profile: &Profile) -> (Value, bool) {
    if depth >= MAX_DEPTH {
        return (Value::String("…(depth limit)".into()), true);
    }
    match value {
        Value::String(s) => {
            let n = s.chars().count();
            if n > profile.json_string_chars {
                let head: String = s.chars().take(profile.json_string_chars).collect();
                (
                    Value::String(format!("{head}…(+{} chars)", n - profile.json_string_chars)),
                    true,
                )
            } else {
                (value.clone(), false)
            }
        }
        Value::Array(items) => {
            let mut changed = items.len() > profile.json_array_elems;
            let mut out: Vec<Value> = Vec::new();
            for item in items.iter().take(profile.json_array_elems) {
                let (v, c) = shrink(item, depth + 1, profile);
                changed |= c;
                out.push(v);
            }
            if items.len() > profile.json_array_elems {
                out.push(Value::String(format!(
                    "…(+{} more items)",
                    items.len() - profile.json_array_elems
                )));
            }
            (Value::Array(out), changed)
        }
        Value::Object(map) => {
            // Keys are the structural signal, so they are kept — but not without
            // limit. A document that is large because it is *wide* (a lockfile,
            // `cargo metadata`, `npm ls --json`, a locale bundle: thousands of
            // short keys, no leaf over the string cap) set `changed = false`
            // all the way up and so was the one class of oversized JSON the
            // reducer structurally refused to touch. Cap parallel to the array
            // arm above.
            //
            // Which keys survive is chosen, not incidental. `serde_json::Map` is a
            // `BTreeMap` unless the `preserve_order` feature is on — it isn't here
            // — so `iter()` yields *alphabetical* order, and a plain `take(N)`
            // dropped `status`, `message` and `error` from any wide object purely
            // because those names sort late. Short scalars go first: they are the
            // salient fields this reducer exists to preserve, and they are nearly
            // free.
            let mut changed = map.len() > profile.json_object_keys;
            let mut out = serde_json::Map::new();
            if map.len() > profile.json_object_keys {
                for (k, v) in map
                    .iter()
                    .filter(|(_, v)| is_short_scalar(v, profile.json_string_chars))
                {
                    if out.len() >= profile.json_object_keys {
                        break;
                    }
                    out.insert(k.clone(), v.clone());
                }
            }
            for (k, v) in map {
                if out.len() >= profile.json_object_keys {
                    break;
                }
                if out.contains_key(k) {
                    continue;
                }
                let (nv, c) = shrink(v, depth + 1, profile);
                changed |= c;
                out.insert(k.clone(), nv);
            }
            if map.len() > out.len() {
                out.insert(
                    "…".to_string(),
                    Value::String(format!("(+{} more keys)", map.len() - out.len())),
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

    fn reduce_doc(text: &str) -> Option<Reduction> {
        reduce_json(text, &Profile::DEFAULT)
    }

    fn shrink_default(value: &Value) -> (Value, bool) {
        shrink(value, 0, &Profile::DEFAULT)
    }

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
        let r = reduce_doc(&s).expect("oversized single-line JSON should reduce");
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
        assert_eq!(r.tally.total(), s.chars().count());
        assert_eq!(r.tally.kept(), r.body.chars().count());
        assert!(
            r.tally.kept() < r.tally.total(),
            "kept ({}) must be smaller than total ({})",
            r.tally.kept(),
            r.tally.total()
        );
    }

    #[test]
    fn all_small_json_not_worth_reducing() {
        // Multi-line but every value is tiny → nothing dropped → None, so the
        // caller keeps its existing handling.
        let s = "{\n  \"a\": 1,\n  \"b\": 2,\n  \"c\": 3,\n  \"d\": 4,\n  \"e\": 5,\n  \"f\": 6,\n  \"g\": 7,\n  \"h\": 8\n}";
        assert!(reduce_doc(s).is_none());
    }

    #[test]
    fn malformed_json_returns_none() {
        let s = "{\n  not really: json,\n  missing quotes,\n  trailing,\n  a,\n  b,\n  c,\n  d\n}";
        assert!(reduce_doc(s).is_none());
    }

    /// The reducer must not change the input's *density*. Pretty-printing a
    /// compact document expands it roughly threefold, so the reduction came out
    /// larger than the input, the central byte guard correctly rejected it, and
    /// the model got a head/tail byte slice of JSON after the reducer had done
    /// all of the work.
    #[test]
    fn a_compact_document_reduces_to_a_compact_body() {
        let big = "x".repeat(20_000);
        let compact = format!(r#"{{"status":"error","body":"{big}","code":7}}"#);
        let r = reduce_doc(&compact).expect("an oversized compact document must reduce");
        assert!(
            !r.body.contains('\n'),
            "a compact input must not come back pretty-printed: {}",
            &r.body[..r.body.len().min(120)]
        );
        assert!(r.body.len() < compact.len() / 10);
        // …while a pretty input still gets a pretty (legible) body.
        let pretty =
            serde_json::to_string_pretty(&serde_json::from_str::<Value>(&compact).expect("valid"))
                .expect("valid");
        let r2 = reduce_doc(&pretty).expect("the pretty form must reduce too");
        assert!(r2.body.contains('\n'), "a pretty input stays pretty");
    }

    /// Counting a stream must not cost a parse per record: a `--watch` capture is
    /// hundreds of thousands of lines and this runs synchronously on the ingress
    /// path. Only the sample plus the last record are parsed in full.
    #[test]
    fn a_long_ndjson_stream_is_counted_without_parsing_every_record() {
        let mut s = String::new();
        for i in 0..5_000 {
            s.push_str(&format!(
                r#"{{"type":"match","n":{i},"path":"src/f{i}.rs"}}"#
            ));
            s.push('\n');
        }
        let body = reduce_jsonl(s.trim(), &Profile::DEFAULT).expect("ndjson must reduce");
        assert!(
            body.contains(&format!(
                "…(+{} more records)",
                5_000 - Profile::DEFAULT.jsonl_records
            )),
            "the record tally must count every line: {}",
            &body[body.len().saturating_sub(80)..]
        );
        assert_eq!(
            body.lines().count(),
            Profile::DEFAULT.jsonl_records + 1,
            "only the sample plus the marker are emitted"
        );
    }

    /// A stream whose tail is not a document must not be reported as one: the
    /// head sample alone cannot vouch for the rest.
    #[test]
    fn a_stream_that_stops_being_json_is_declined() {
        let mut s = String::new();
        for i in 0..40 {
            s.push_str(&format!(r#"{{"n":{i}}}"#));
            s.push('\n');
        }
        s.push_str("panic: worker died\n");
        assert!(reduce_jsonl(s.trim(), &Profile::DEFAULT).is_none());
    }

    /// `serde_json::Map` is a `BTreeMap` here (no `preserve_order` feature), so a
    /// plain `take(N)` kept the alphabetically-first keys and dropped exactly the
    /// salient scalars the module doc promises to preserve.
    #[test]
    fn the_object_cap_keeps_salient_scalars_not_the_alphabetically_first_keys() {
        let mut obj = serde_json::Map::new();
        // 200 bulky keys that all sort before "status"/"message".
        for i in 0..200 {
            obj.insert(
                format!("aaa_bucket_{i:03}"),
                serde_json::json!({ "nested": "x".repeat(40) }),
            );
        }
        obj.insert("status".into(), serde_json::json!("failed"));
        obj.insert("message".into(), serde_json::json!("connection refused"));
        obj.insert("error".into(), serde_json::json!("ECONNREFUSED"));
        obj.insert("retries".into(), serde_json::json!(3));

        let (reduced, changed) = shrink_default(&Value::Object(obj));
        assert!(changed, "a 204-key object must count as reduced");
        let out = reduced.as_object().expect("still an object");
        for key in ["status", "message", "error", "retries"] {
            assert!(
                out.contains_key(key),
                "the salient scalar {key:?} must survive the cap; got keys: {:?}",
                out.keys().take(8).collect::<Vec<_>>()
            );
        }
        assert_eq!(out["status"], serde_json::json!("failed"));
        assert!(
            out.contains_key("…"),
            "the omission marker must say how many keys were dropped"
        );
    }
}
