//! Ingress hygiene: clean a tool's **structured** result before it is flattened
//! into the model's context.
//!
//! # The bug this exists to fix
//!
//! Every builtin tool returns a typed struct, so
//! [`AlephTool::call_json`](crate::tools::AlephTool) hands the dispatcher a
//! `serde_json::Value::Object`. The dispatcher then flattens it with
//! `Value::to_string()`, which produces **compact single-line JSON**: every
//! `\n` inside `stdout` becomes the two-character escape `\` + `n`, and the
//! whole result — a 40 000-line `cargo test` log included — collapses onto
//! *one* line.
//!
//! Both of Aleph's content-aware cleaners key off line structure:
//!
//! - [`structured::classify`] requires `MIN_LINES` lines, so it returns `None`
//!   for any single-line input. The log / search / diff / json reducers were
//!   therefore **unreachable for every builtin tool** — they only ever fired on
//!   MCP tools, which return a bare `Value::String` with real newlines.
//! - [`distill_output`](super::distill::distill_output) iterates `text.lines()`.
//!   Given one line it reports `total_lines: 1` and keeps that single "line"
//!   capped at 400 chars — so the "surface key errors inline" feature showed the
//!   *head of the JSON envelope* (`{"success":false,"exit_code":101,"stdout":"…`)
//!   instead of the compile errors it promised.
//!
//! Running the reducers here — while `stdout` / `stderr` still hold real
//! newlines — is what makes them reachable. Nothing about the reducers changes;
//! they were simply being handed the wrong shape.
//!
//! # Discipline
//!
//! - **Never grows.** A field is replaced only when the reduction is strictly
//!   smaller than what it replaces, measured with the same estimator the budget
//!   uses.
//! - **Never silent.** [`structured::Reduction::render`] prefixes an honest
//!   `[compacted log: kept 43/812 lines]` header, so the note travels inside the
//!   field itself — no extra plumbing, and the model can re-run the command.
//! - **Never lossy-irreversible.** The caller
//!   ([`apply_result_budget`](crate::tools::result_processing::apply_result_budget))
//!   persists the untouched original whenever a field was reduced, so the dropped
//!   lines stay recoverable via `ctx_search` / `read_file`.
//! - **Opt-in by pressure.** The caller only runs this when the flattened result
//!   is already over the tool's declared token budget, so the overwhelming
//!   majority of tool calls are byte-for-byte unaffected.

use serde_json::Value;

use crate::context::budget::pressure::estimate_tokens_smart;

use super::distill::distill_output;
use super::sanitize::sanitize_command_output;
use super::structured::{self, ContentKind};

/// Token floor a single string field must clear before the content-type router
/// is worth running. Below this, a reduction's header can cost more than the
/// lines it drops.
const MIN_FIELD_TOKENS: usize = 150;

/// Deepest nesting walked when looking for reducible text fields.
///
/// Bounded so a pathologically nested tool result can't blow the stack (P7).
/// Depth 4 covers every real shape: top-level strings (MCP), one-level structs
/// (`stdout` / `stderr`), and the `{ data: { … } }` wrapper the desktop tools
/// use.
const MAX_DEPTH: usize = 4;

/// How a field was shortened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReductionMethod {
    /// A content-type-routed structural reduction (log / search / diff / json).
    Structured(ContentKind),
    /// An error + `file:line` digest. The fallback for output that carries clear
    /// failure signal but no structure the router recognizes — a build log with
    /// one compile error under a few hundred `Compiling …` lines is the common
    /// shape (two "loud" lines is below `looks_like_log`'s threshold, which is
    /// deliberately conservative so ordinary prose is never routed there).
    Distilled,
}

/// What one field reduction achieved. Returned for tracing and for the caller's
/// "was anything reduced?" decision — not serialized into the model's context
/// (the honest header inside the field already carries that).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldReduction {
    /// Dotted path of the reduced field (`stdout`, `data.output`, `items.0.log`).
    pub field: String,
    /// Which cleaner shortened it.
    pub method: ReductionMethod,
    /// Estimated tokens before / after, via the budget's own estimator.
    pub tokens_before: usize,
    pub tokens_after: usize,
}

/// Apply content-type-routed reduction to every reducible text field of a tool
/// result, in place. Returns one entry per field actually shortened (empty when
/// nothing matched, which is the common case and a strict no-op).
#[must_use]
pub fn clean_result_value(value: &mut Value) -> Vec<FieldReduction> {
    let mut out = Vec::new();
    let mut path = Vec::new();
    walk(value, &mut path, 0, &mut out);
    out
}

fn walk(value: &mut Value, path: &mut Vec<String>, depth: usize, out: &mut Vec<FieldReduction>) {
    if depth > MAX_DEPTH {
        return;
    }
    match value {
        Value::String(s) => {
            if let Some((method, before, after)) = reduce_field(s) {
                out.push(FieldReduction {
                    field: if path.is_empty() {
                        // A bare `Value::String` root — an MCP text result.
                        "<result>".to_string()
                    } else {
                        path.join(".")
                    },
                    method,
                    tokens_before: before,
                    tokens_after: after,
                });
            }
        }
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                path.push(key.clone());
                walk(child, path, depth + 1, out);
                path.pop();
            }
        }
        Value::Array(items) => {
            for (idx, child) in items.iter_mut().enumerate() {
                path.push(idx.to_string());
                walk(child, path, depth + 1, out);
                path.pop();
            }
        }
        _ => {}
    }
}

/// Reduce one string field in place. `None` (and the field untouched) when the
/// field is too small to bother with, carries neither recognizable structure nor
/// error signal, or when the reduction wouldn't actually be smaller.
fn reduce_field(field: &mut String) -> Option<(ReductionMethod, usize, usize)> {
    let tokens_before = estimate_tokens_smart(field);
    if tokens_before < MIN_FIELD_TOKENS {
        return None;
    }

    // Strip ANSI/VT100 escapes first. Colourised output carries several escape
    // runs per line (each worth a few tokens), and a leading colour reset ahead
    // of a `path:line:` match is exactly the kind of noise that can defeat the
    // search classifier. Borrowed (byte-identical) when already clean, so this
    // costs nothing for the common case. `bash` output reaches us already
    // sanitized; MCP text results do not.
    let cleaned = sanitize_command_output(field);

    // Tier 1: content-type-routed structural reduction. Preferred — it preserves
    // shape and works for successful output, not just failures.
    let candidate = match structured::reduce(&cleaned) {
        Some(reduction) => Some((
            ReductionMethod::Structured(reduction.kind),
            reduction.render(),
        )),
        // Tier 2: an error + path digest. Complements tier 1 rather than
        // duplicating it: the router needs recognizable *structure*, the
        // distiller needs an error *signal*, and plenty of real output has one
        // without the other.
        None => distill_output(&cleaned)
            .filter(|digest| digest.error_count > 0)
            .map(|digest| {
                (
                    ReductionMethod::Distilled,
                    digest.render(digest.salient.len()),
                )
            }),
    };

    let (method, rendered) = candidate?;
    let tokens_after = estimate_tokens_smart(&rendered);
    // Final guard: never grow the context, whatever either cleaner decided.
    if tokens_after >= tokens_before {
        return None;
    }
    *field = rendered;
    Some((method, tokens_before, tokens_after))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A realistic `cargo build` failure as the `bash` tool actually produces
    /// it: a typed struct whose `stdout` holds the multi-line log.
    fn failing_build_output() -> Value {
        let mut log = String::from("$ cargo build\n");
        for i in 0..400 {
            log.push_str(&format!("   Compiling dep-{i} v1.0.0\n"));
        }
        log.push_str("error[E0382]: borrow of moved value: `x`\n");
        log.push_str("  --> src/main.rs:10:5\n");
        log.push_str("   |\n");
        log.push_str("10 |     use(x);\n");
        for i in 0..200 {
            log.push_str(&format!("   more noise {i}\n"));
        }
        log.push_str("error: could not compile `app` due to 1 previous error\n");
        json!({
            "success": false,
            "exit_code": 101,
            "stdout": log,
            "stderr": "",
            "language": "shell",
        })
    }

    #[test]
    fn reduces_the_stdout_field_of_a_builtin_tool_result() {
        let mut value = failing_build_output();
        let before = value.to_string().len();

        let reductions = clean_result_value(&mut value);

        assert_eq!(reductions.len(), 1, "only `stdout` should reduce");
        assert_eq!(reductions[0].field, "stdout");
        assert!(reductions[0].tokens_after < reductions[0].tokens_before);

        let stdout = value["stdout"].as_str().expect("stdout stays a string");
        assert!(
            stdout.starts_with("[compacted ") || stdout.starts_with("[Output digest:"),
            "an honest header must lead the field; got: {}",
            &stdout[..stdout.len().min(80)]
        );
        assert!(
            stdout.contains("error[E0382]"),
            "the actual error must survive; got:\n{stdout}"
        );
        assert!(
            stdout.contains("src/main.rs:10:5"),
            "the file:line the model needs must survive"
        );
        assert!(
            stdout.contains("could not compile"),
            "the final verdict line must survive"
        );
        assert!(
            !stdout.contains("Compiling dep-200"),
            "the compile noise must be gone"
        );
        assert!(
            value.to_string().len() < before / 3,
            "the flattened result must shrink substantially"
        );
        // Sibling fields are untouched — this is a field-wise pass, not a rewrite.
        assert_eq!(value["exit_code"], 101);
        assert_eq!(value["success"], false);
    }

    /// The regression that motivated this module: flattening first makes the
    /// content-type router blind, because compact JSON is one single line.
    #[test]
    fn flattened_json_is_one_line_so_reducers_must_run_before_flattening() {
        let value = failing_build_output();
        let flattened = value.to_string();
        assert_eq!(
            flattened.lines().count(),
            1,
            "Value::to_string() escapes newlines — the whole log is one line"
        );
        assert!(
            structured::classify(&flattened).is_none(),
            "classify() cannot route single-line input, which is why the \
             reducers never fired for builtin tools"
        );
        // …whereas the same content reached field-wise is cleanable: the
        // distiller finds the compile error and its `file:line`, which it cannot
        // do once every newline is an escape and the whole log is one line.
        let field = value["stdout"].as_str().unwrap();
        let digest = super::distill_output(field).expect("field-wise distill finds the error");
        assert!(digest.error_count > 0);
        assert!(digest.paths.iter().any(|p| p.contains("src/main.rs")));
        assert_eq!(
            super::distill_output(&flattened).map(|d| d.total_lines),
            Some(1),
            "flattened JSON is one line, so the distiller sees one 'line'"
        );
    }

    #[test]
    fn small_fields_are_left_alone() {
        let mut value = json!({ "stdout": "ok\n", "stderr": "" });
        let before = value.clone();
        assert!(clean_result_value(&mut value).is_empty());
        assert_eq!(value, before, "no-op must be byte-identical");
    }

    #[test]
    fn prose_is_not_reduced() {
        let prose = "This is an ordinary paragraph of explanatory prose. ".repeat(200);
        let mut value = json!({ "message": prose });
        let before = value.clone();
        assert!(
            clean_result_value(&mut value).is_empty(),
            "unrecognized content must fall through to the caller's truncator"
        );
        assert_eq!(value, before);
    }

    #[test]
    fn reduces_a_bare_string_root_as_mcp_tools_return() {
        let mut log = String::from("running 3 tests\n");
        for i in 0..300 {
            log.push_str(&format!("test case_{i} ... ok\n"));
        }
        log.push_str("test case_bad ... FAILED\n");
        log.push_str("test result: FAILED. 300 passed; 1 failed\n");
        let mut value = Value::String(log);

        let reductions = clean_result_value(&mut value);
        assert_eq!(reductions.len(), 1);
        assert_eq!(reductions[0].field, "<result>", "root has no field name");
        let text = value.as_str().unwrap();
        assert!(text.contains("FAILED. 300 passed; 1 failed"));
    }

    #[test]
    fn strips_ansi_before_classifying() {
        // Colourised `rg` output: each line carries several escape runs.
        let mut hits = String::new();
        for i in 0..40 {
            hits.push_str(&format!(
                "\u{1b}[35msrc/lib.rs\u{1b}[0m\u{1b}[36m:\u{1b}[0m{i}\u{1b}[36m:\u{1b}[0m    let \u{1b}[1m\u{1b}[31mtarget\u{1b}[0m = 1;\n"
            ));
        }
        let mut value = json!({ "stdout": hits });
        let reductions = clean_result_value(&mut value);
        assert_eq!(reductions.len(), 1, "colourised search output must reduce");
        let stdout = value["stdout"].as_str().unwrap();
        assert!(
            !stdout.contains('\u{1b}'),
            "no escape byte may reach the model; got: {stdout}"
        );
    }

    #[test]
    fn walks_into_the_data_wrapper() {
        let mut log = String::from("$ pytest\n");
        for i in 0..300 {
            log.push_str(&format!("tests/test_{i}.py ..........\n"));
        }
        log.push_str("FAILED tests/test_9.py::test_boom - AssertionError\n");
        log.push_str("=== 1 failed, 299 passed in 12.4s ===\n");
        let mut value = json!({ "data": { "output": log } });

        let reductions = clean_result_value(&mut value);
        assert_eq!(reductions.len(), 1);
        assert_eq!(reductions[0].field, "data.output");
        assert!(value["data"]["output"]
            .as_str()
            .unwrap()
            .contains("1 failed, 299 passed"));
    }

    #[test]
    fn deeply_nested_fields_are_not_walked_forever() {
        // Build nesting past MAX_DEPTH; the pass must return rather than recurse.
        let mut value = json!({ "leaf": "x" });
        for _ in 0..64 {
            value = json!({ "n": value });
        }
        assert!(clean_result_value(&mut value).is_empty());
    }
}
