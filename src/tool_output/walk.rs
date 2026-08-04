//! Depth-bounded walk over the text fields of a tool's structured result.
//!
//! Both ingress stages that shorten text — the per-tool compressor and the
//! content-type router — have to visit the same places: a bare `Value::String`
//! root, the `stdout` / `stderr` of a command tool, the `content` blocks of an
//! MCP result, the `{ data: … }` wrapper the desktop tools use. Sharing one
//! walker keeps them from drifting into visiting different subsets of the same
//! value, which is the failure mode that decides whether a cleaner is reachable
//! at all.

use serde_json::Value;

/// Deepest nesting walked when looking for reducible text fields.
///
/// Bounded so a pathologically nested tool result can't blow the stack (P7).
/// Depth 4 covers every real shape: top-level strings (the opaque MCP
/// fallback), one-level structs (`stdout` / `stderr`), the `{ data: { … } }`
/// wrapper the desktop tools use, and `content[i].text` for an MCP result.
const MAX_DEPTH: usize = 4;

/// The dotted path of a visited field: `stdout`, `data.output`, `content.0.text`
/// — or [`ROOT_FIELD`] for a bare string root.
pub(super) const ROOT_FIELD: &str = "<result>";

/// Visit every string leaf of `value` in place, passing its dotted path.
///
/// The visitor mutates the field directly; returning is the only signal needed,
/// since each stage tracks its own bookkeeping.
pub(super) fn for_each_text_field<F>(value: &mut Value, mut visit: F)
where
    F: FnMut(&str, &mut String),
{
    let mut path = Vec::new();
    walk(value, &mut path, 0, &mut visit);
}

fn walk<F>(value: &mut Value, path: &mut Vec<String>, depth: usize, visit: &mut F)
where
    F: FnMut(&str, &mut String),
{
    if depth > MAX_DEPTH {
        return;
    }
    match value {
        Value::String(s) => {
            let field = if path.is_empty() {
                ROOT_FIELD.to_string()
            } else {
                path.join(".")
            };
            visit(&field, s);
        }
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                path.push(key.clone());
                walk(child, path, depth + 1, visit);
                path.pop();
            }
        }
        Value::Array(items) => {
            for (idx, child) in items.iter_mut().enumerate() {
                path.push(idx.to_string());
                walk(child, path, depth + 1, visit);
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
    fn visits_every_shape_the_stages_rely_on() {
        let mut value = json!({
            "stdout": "a",
            "data": { "output": "b" },
            "content": [ { "type": "text", "text": "c" } ],
        });
        let mut seen: Vec<String> = Vec::new();
        for_each_text_field(&mut value, |field, _| seen.push(field.to_string()));
        seen.sort();
        assert_eq!(
            seen,
            vec![
                "content.0.text".to_string(),
                "content.0.type".to_string(),
                "data.output".to_string(),
                "stdout".to_string(),
            ]
        );
    }

    #[test]
    fn a_bare_string_root_is_named() {
        let mut value = Value::String("x".into());
        let mut seen = Vec::new();
        for_each_text_field(&mut value, |field, _| seen.push(field.to_string()));
        assert_eq!(seen, vec![ROOT_FIELD.to_string()]);
    }

    #[test]
    fn mutation_lands_on_the_value() {
        let mut value = json!({ "stdout": "before" });
        for_each_text_field(&mut value, |_, s| *s = "after".to_string());
        assert_eq!(value["stdout"], "after");
    }

    #[test]
    fn deeper_than_the_bound_is_not_walked() {
        let mut value = json!({ "leaf": "x" });
        for _ in 0..64 {
            value = json!({ "n": value });
        }
        let mut seen = 0usize;
        for_each_text_field(&mut value, |_, _| seen += 1);
        assert_eq!(seen, 0);
    }
}
