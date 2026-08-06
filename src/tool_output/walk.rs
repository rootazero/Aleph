//! Depth-bounded walk over the text fields of a tool result, shared by every
//! ingress stage that edits fields in place.
//!
//! Two stages walk the same value — [`hygiene`](super::hygiene) (content-aware
//! reduction) and [`compressor`](super::compressor) (per-tool compression) — and
//! it is a correctness requirement, not a coincidence, that they visit exactly
//! the same set of fields: a field one stage can see and the other cannot is a
//! field whose treatment depends on which stage happened to run first. The
//! walker therefore lives here, once, with the depth cap beside it.
//!
//! The visitor receives the dotted path segments (`["stdout"]`,
//! `["content", "0", "text"]` — array indices stringified) so callers can
//! attribute a change to the field it happened in; the path is empty for a bare
//! `Value::String` root, which is how MCP text results arrive.

use serde_json::Value;

/// Cap on recursive descent into nested `serde_json::Value`s, applied by every
/// caller of [`walk_text_fields`].
///
/// The bound is about **time**, not cycles: a `serde_json::Value` is a tree and
/// cannot contain a reference cycle, so "protect against circular structures"
/// — the rationale an earlier version of this comment gave — describes a shape
/// that cannot exist. What can exist is a pathologically deep config dump, and
/// an unbounded walk over one would spend its time on the ingress path before
/// any stage even started. Four levels covers every tool-result shape observed
/// in production (`data.output`, `content.N.text`, one more for headroom); the
/// test that walks past it builds 64 levels of nesting and asserts the walk
/// bails rather than spinning.
pub(crate) const MAX_WALK_DEPTH: usize = 4;

/// Visit every `Value::String` reachable from `value` within `max_depth` levels,
/// passing its path segments and a mutable handle to the text.
///
/// Objects recurse by key, arrays by index; scalars that are not strings are
/// not visited. Depth is measured in container steps: the root's own fields are
/// visited at depth 0, and descent stops strictly past `max_depth`.
pub(crate) fn walk_text_fields(
    value: &mut Value,
    max_depth: usize,
    visit: &mut impl FnMut(&[String], &mut String),
) {
    let mut path = Vec::new();
    walk(value, max_depth, &mut path, 0, visit);
}

fn walk(
    value: &mut Value,
    max_depth: usize,
    path: &mut Vec<String>,
    depth: usize,
    visit: &mut impl FnMut(&[String], &mut String),
) {
    if depth > max_depth {
        return;
    }
    match value {
        Value::String(s) => visit(path, s),
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                path.push(key.clone());
                walk(child, max_depth, path, depth + 1, visit);
                path.pop();
            }
        }
        Value::Array(items) => {
            for (idx, child) in items.iter_mut().enumerate() {
                path.push(idx.to_string());
                walk(child, max_depth, path, depth + 1, visit);
                path.pop();
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn visits_every_string_within_the_depth_cap_with_its_path() {
        let mut value = json!({
            "stdout": "a",
            "n": 1,
            "data": { "output": "b" },
            "content": [ { "text": "c" } ],
        });
        let mut seen: Vec<(String, String)> = Vec::new();
        walk_text_fields(&mut value, MAX_WALK_DEPTH, &mut |path, s| {
            seen.push((path.join("."), s.clone()));
        });
        seen.sort();
        assert_eq!(
            seen,
            vec![
                ("content.0.text".to_string(), "c".to_string()),
                ("data.output".to_string(), "b".to_string()),
                ("stdout".to_string(), "a".to_string()),
            ],
            "non-strings are skipped, nested fields carry their path"
        );
    }

    #[test]
    fn a_bare_string_root_is_visited_with_an_empty_path() {
        let mut value = Value::String("root".to_string());
        let mut calls = 0;
        walk_text_fields(&mut value, MAX_WALK_DEPTH, &mut |path, s| {
            assert!(path.is_empty(), "the root has no field name");
            assert_eq!(s, "root");
            calls += 1;
        });
        assert_eq!(calls, 1);
    }

    #[test]
    fn descent_stops_past_the_depth_cap() {
        let mut value = json!({ "leaf": "x" });
        for _ in 0..64 {
            value = json!({ "n": value });
        }
        let mut calls = 0;
        walk_text_fields(&mut value, MAX_WALK_DEPTH, &mut |_, _| calls += 1);
        assert_eq!(calls, 0, "the walk must bail, not spin, on deep nesting");
    }
}
