//! Line-preserving rendering of a flattened tool result, for the readers that
//! work in lines.
//!
//! The model-facing channel flattens a typed tool result with
//! `Value::to_string()`: one line of compact JSON, every newline inside
//! `stdout` escaped to two characters. That is fine for the model (it reads
//! JSON), and fatal for the two readers of an **offloaded** original, both of
//! which work in lines:
//!
//! - `file_read` pages a file by line and clamps each overlong line, so a
//!   one-line blob reads as its clamped head and `offset`/`limit` page
//!   nothing;
//! - `ContentIndex` chunks by a fixed number of lines, so a one-line blob is
//!   ONE section and `ctx_search` can only ever return that same section.
//!
//! [`line_preserving`] is applied where an original is written to the result
//! store (`result_processing::recovery_footer_for`), so every writer — Layer 2, the
//! harness Layer-3 spill, the browser offload — stores the same shape.
//!
//! This is a *rendering*, not a serialization: it is for reading and searching,
//! and nothing parses it back. A string value `"true"` and the boolean `true`
//! render alike, and a content line that happens to start with `## ` looks like
//! a header. What it keeps is every string byte, on its own lines.

use std::borrow::Cow;

use serde_json::Value;

use super::walk::MAX_WALK_DEPTH;

/// A tool result as it is stored and indexed.
pub(crate) struct Rendered<'a> {
    /// The text to persist and index (see [`line_preserving`]).
    pub text: Cow<'a, str>,
    /// Some text field — or the whole text, when it is not a JSON envelope —
    /// is a fenced payload, i.e. the tool that produced it marked it as
    /// external, untrusted content. Decided by [`super::fence::is_fenced`],
    /// the same test the ingress rewrites route on, over the same fields
    /// (the ingress walk and this one share [`MAX_WALK_DEPTH`]).
    pub fenced: bool,
}

/// Render `text` line-preserving when it is a flattened JSON envelope;
/// otherwise return it unchanged.
///
/// The predicate is the shape `Value::to_string()` produces and nothing wider:
/// **one line** that parses as a JSON object or array. Text that already has a
/// newline is already line-oriented (a browser offload, an MCP text result, a
/// plain log) and is returned byte-identical, as is anything that does not
/// parse — a one-line log that merely starts with `{` included.
///
/// Layout: every single-line leaf first, as `path: value` (document order),
/// then every multi-line string as a `## path` header followed by its lines
/// verbatim. Single-line leaves go first so that a scalar such as
/// `exit_code: 101` can never read as the last line of the `stdout` block
/// above it. Containers nested deeper than [`MAX_WALK_DEPTH`] are written as
/// compact JSON under their path — lossless, and it bounds how often a long
/// path prefix is repeated.
#[must_use]
pub(crate) fn line_preserving(text: &str) -> Rendered<'_> {
    fn as_is(text: &str) -> Rendered<'_> {
        Rendered {
            text: Cow::Borrowed(text),
            fenced: super::fence::is_fenced(text),
        }
    }
    if text.contains('\n') {
        return as_is(text);
    }
    let head = text.trim_start();
    if !(head.starts_with('{') || head.starts_with('[')) {
        return as_is(text);
    }
    match serde_json::from_str::<Value>(text) {
        Ok(value @ (Value::Object(_) | Value::Array(_))) => render(&value),
        _ => as_is(text),
    }
}

#[derive(Default)]
struct Renderer {
    leaves: String,
    blocks: String,
    fenced: bool,
}

fn render(value: &Value) -> Rendered<'static> {
    let mut out = Renderer::default();
    out.visit(value, &mut Vec::new(), 0);
    let Renderer {
        mut leaves,
        blocks,
        fenced,
    } = out;
    leaves.push_str(&blocks);
    Rendered {
        text: Cow::Owned(leaves),
        fenced,
    }
}

impl Renderer {
    fn visit(&mut self, value: &Value, path: &mut Vec<String>, depth: usize) {
        match value {
            Value::Object(map) if !map.is_empty() && depth <= MAX_WALK_DEPTH => {
                for (key, child) in map {
                    path.push(key.clone());
                    self.visit(child, path, depth + 1);
                    path.pop();
                }
            }
            Value::Array(items) if !items.is_empty() && depth <= MAX_WALK_DEPTH => {
                for (idx, child) in items.iter().enumerate() {
                    path.push(idx.to_string());
                    self.visit(child, path, depth + 1);
                    path.pop();
                }
            }
            Value::String(s) if s.contains('\n') => {
                self.fenced |= super::fence::is_fenced(s);
                self.blocks.push_str("## ");
                self.blocks.push_str(&label(path));
                self.blocks.push('\n');
                self.blocks.push_str(s);
                if !s.ends_with('\n') {
                    self.blocks.push('\n');
                }
            }
            // A fence is never one line (its markers are lines of their own),
            // so a single-line string cannot be fenced.
            Value::String(s) => push_leaf(&mut self.leaves, path, s),
            // Scalars, empty containers, and containers past the depth cap.
            other => push_leaf(&mut self.leaves, path, &other.to_string()),
        }
    }
}

fn push_leaf(out: &mut String, path: &[String], value: &str) {
    out.push_str(&label(path));
    out.push_str(": ");
    out.push_str(value);
    out.push('\n');
}

/// Dotted field path (`content.0.text`); `.` for the root, which only a
/// top-level empty container can reach.
fn label(path: &[String]) -> String {
    if path.is_empty() {
        ".".to_string()
    } else {
        path.join(".")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_flattened_envelope_renders_its_text_fields_as_real_lines() {
        let value = json!({
            "success": false,
            "exit_code": 101,
            "stdout": "line one\nline two\n",
            "stderr": "",
        });
        let flat = value.to_string();
        assert!(
            !flat.contains('\n'),
            "precondition: the flat form is one line"
        );

        let rendered = line_preserving(&flat).text;
        let lines: Vec<&str> = rendered.lines().collect();

        for leaf in ["success: false", "exit_code: 101", "stderr: "] {
            assert!(lines.contains(&leaf), "missing leaf {leaf:?}:\n{rendered}");
        }
        let header = lines
            .iter()
            .position(|l| *l == "## stdout")
            .expect("the multi-line field gets a header");
        assert_eq!(&lines[header + 1..header + 3], ["line one", "line two"]);
        // Every single-line leaf precedes every block.
        let last_leaf = lines.iter().rposition(|l| l.contains(": ")).unwrap();
        assert!(
            last_leaf < header,
            "leaves must come before blocks:\n{rendered}"
        );
    }

    #[test]
    fn nested_mcp_content_keeps_its_path_and_its_fence_lines() {
        let fenced = "<<<EXTERNAL_UNTRUSTED_CONTENT id=\"x\">\nbody a\nbody b\n<<<END_EXTERNAL_UNTRUSTED_CONTENT id=\"x\">";
        let value = json!({ "content": [ { "type": "text", "text": fenced } ] });

        let flat = value.to_string();
        let out = line_preserving(&flat);
        assert!(
            out.fenced,
            "a fenced text field marks the whole result as fenced"
        );
        let rendered = out.text.into_owned();

        assert!(rendered.contains("content.0.type: text\n"), "{rendered}");
        assert!(
            rendered.contains(&format!("## content.0.text\n{fenced}\n")),
            "the fenced text must survive verbatim, both markers on their own lines:\n{rendered}"
        );
    }

    /// A bare fenced string (an MCP text result, a browser offload) is stored
    /// as-is and still reported as fenced.
    #[test]
    fn a_bare_fenced_text_passes_through_and_is_reported_fenced() {
        let fenced = crate::security::content_sanitizer::wrap_external_content(
            "page line 1\npage line 2",
            crate::security::content_sanitizer::ContentSource::BrowserContent,
        );
        let out = line_preserving(&fenced);
        assert!(matches!(out.text, Cow::Borrowed(t) if t == fenced));
        assert!(out.fenced);
    }

    #[test]
    fn arrays_of_scalars_become_one_line_each() {
        let value = json!({ "files": ["a.rs", "b.rs"], "empty": [], "none": null });
        let flat = value.to_string();
        let out = line_preserving(&flat);
        assert!(!out.fenced);
        let rendered = out.text.into_owned();
        for line in ["files.0: a.rs", "files.1: b.rs", "empty: []", "none: null"] {
            assert!(
                rendered.lines().any(|l| l == line),
                "missing {line:?}:\n{rendered}"
            );
        }
    }

    #[test]
    fn containers_past_the_depth_cap_stay_whole_as_compact_json() {
        let mut deep = json!({ "leaf": "x" });
        for _ in 0..8 {
            deep = json!({ "n": deep });
        }
        let rendered = line_preserving(&deep.to_string()).text.into_owned();
        assert!(
            rendered.contains("\"leaf\":\"x\""),
            "the part past the cap is kept, not dropped:\n{rendered}"
        );
        assert_eq!(rendered.lines().count(), 1, "one leaf line:\n{rendered}");
    }

    #[test]
    fn text_that_is_already_line_oriented_or_not_json_is_returned_untouched() {
        for text in [
            "plain log line one\nline two",
            "{\"multi\":\n\"line json is left alone\"}",
            "{ not json at all",
            "just words",
            "42",
            "\"a bare json string\"",
        ] {
            let out = line_preserving(text);
            assert!(
                matches!(out.text, Cow::Borrowed(t) if t == text),
                "must pass through byte-identical: {text:?}"
            );
            assert!(!out.fenced, "{text:?}");
        }
    }
}
