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
//! - `structured::classify` requires `MIN_LINES` lines, so it returns `None`
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

use std::borrow::Cow;

use serde_json::Value;

use crate::context::budget::pressure::estimate_tokens_smart;

use super::distill::distill_output;
use super::fence::rewrite_interior;
use super::sanitize::sanitize_command_output;
use super::structured::{self, ContentKind};
use super::walk::for_each_text_field;

/// Token floor a single string field must clear before the content-type router
/// is worth running. Below this, a reduction's header can cost more than the
/// lines it drops.
const MIN_FIELD_TOKENS: usize = 150;

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
    /// Neither cleaner recognized the content, but stripping ANSI escapes and
    /// stray control bytes was itself a win. The text is otherwise untouched —
    /// no line was dropped, so nothing has to be recovered from disk.
    Sanitized,
}

impl ReductionMethod {
    /// Whether this method dropped content (as opposed to only removing bytes
    /// that were never content). Drives the caller's "offload the original so
    /// the reduction stays reversible" decision: sanitising loses nothing a
    /// model could want back, so it must not trigger a persist.
    #[must_use]
    pub const fn is_lossy(self) -> bool {
        !matches!(self, Self::Sanitized)
    }
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
    for_each_text_field(value, |field, text| {
        if let Some((method, before, after)) = reduce_field(text) {
            out.push(FieldReduction {
                field: field.to_string(),
                method,
                tokens_before: before,
                tokens_after: after,
            });
        }
    });
    out
}

/// Reduce one string field in place. `None` (and the field untouched) when the
/// field is too small to bother with, carries neither recognizable structure nor
/// error signal, or when the reduction wouldn't actually be smaller.
///
/// A field that is a fenced untrusted payload is reduced **inside the fence**:
/// the boundary markers are structure, not content (see
/// [`split_external_fence`]). Rewriting the field wholesale used to drop them —
/// on `web_fetch`, the browser tools and MCP results, i.e. precisely the
/// payloads the fence exists for, and precisely the large ones that reach this
/// pass at all.
fn reduce_field(field: &mut String) -> Option<(ReductionMethod, usize, usize)> {
    let tokens_before = estimate_tokens_smart(field);
    if tokens_before < MIN_FIELD_TOKENS {
        return None;
    }

    let mut method = None;
    let rendered = rewrite_interior(field, |payload| {
        // Strip ANSI/VT100 escapes first. Colourised output carries several
        // escape runs per line (each worth a few tokens), and a leading colour
        // reset ahead of a `path:line:` match is exactly the kind of noise that
        // can defeat the search classifier. Borrowed (byte-identical) when
        // already clean, so this costs nothing for the common case. `bash`
        // output reaches us already sanitized; MCP text results do not.
        let cleaned = sanitize_command_output(payload);

        // Tier 1: content-type-routed structural reduction. Preferred — it
        // preserves shape and works for successful output, not just failures.
        let candidate = match structured::reduce(&cleaned) {
            Some(reduction) => Some((
                ReductionMethod::Structured(reduction.kind),
                reduction.render(),
            )),
            // Tier 2: an error + path digest. Complements tier 1 rather than
            // duplicating it: the router needs recognizable *structure*, the
            // distiller needs an error *signal*, and plenty of real output has
            // one without the other.
            None => distill_output(&cleaned)
                .filter(|digest| digest.error_count > 0)
                .map(|digest| {
                    (
                        ReductionMethod::Distilled,
                        digest.render(digest.salient.len()),
                    )
                }),
        };

        // Both cleaners declined. The escape stripping still stands on its own:
        // an escape sequence is never content, and a colourised wall of output
        // that neither reducer recognizes would otherwise carry every `ESC[…m`
        // run into the context. Only taken when it actually bought something, so
        // clean input stays byte-identical.
        let (picked, body) = match candidate {
            Some(pair) => pair,
            None => match cleaned {
                Cow::Borrowed(_) => return None,
                Cow::Owned(s) => (ReductionMethod::Sanitized, s),
            },
        };
        method = Some(picked);
        Some(body)
    })?;

    let method = method.expect("set on every accepting path of the rewrite");
    let tokens_after = estimate_tokens_smart(&rendered);
    // Final guard: never grow the context, whatever either cleaner decided.
    // Re-wrapping a fence costs nothing (the markers come back byte-identical),
    // but a reduction that barely shrinks still has to earn its header here.
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
        // The log / search / diff reducers select *lines*; given one line there
        // is nothing to select, which is why they never fired for builtin tools.
        // (The JSON reducer is the exception — it parses rather than selects, so
        // it can still see a flattened envelope. It just cannot see the log
        // *inside* the envelope, which is the signal that matters here.)
        assert_ne!(
            structured::classify(&flattened),
            Some(ContentKind::Log),
            "a flattened envelope cannot be routed to the log reducer"
        );
        let flat_reduced = structured::reduce(&flattened);
        assert!(
            flat_reduced.is_none_or(|r| !r.body.contains("E0382")),
            "reducing the envelope cannot recover the compile error buried in \
             an escaped string leaf"
        );
        // …whereas the same content reached field-wise is cleanable: the
        // distiller finds the compile error and its `file:line`, which it cannot
        // do once every newline is an escape and the whole log is one line.
        let field = value["stdout"].as_str().unwrap();
        let digest = super::distill_output(field).expect("field-wise distill finds the error");
        assert!(digest.error_count > 0);
        assert!(digest.paths.iter().any(|p| p.contains("src/main.rs")));
        assert_eq!(
            super::distill_output(&flattened),
            None,
            "the distiller declines a single line outright — it used to report \
             `total_lines: 1` and render a 400-char prefix of the envelope as \
             though it were the error"
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

    /// The boundary markers are what tell the model the interior is untrusted.
    /// Reducing the field wholesale dropped them — on exactly the payloads the
    /// fence exists for (`web_fetch`, the browser tools, MCP), and only once
    /// they were big enough to reach this pass.
    #[test]
    fn a_fenced_payload_keeps_its_boundary_markers() {
        use crate::security::content_sanitizer::{wrap_external_content, ContentSource};

        let mut page = String::from("$ build log embedded in a fetched page\n");
        for i in 0..400 {
            page.push_str(&format!("   Compiling dep-{i} v1.0.0\n"));
        }
        page.push_str("error[E0382]: borrow of moved value\n");
        page.push_str("  --> src/main.rs:10:5\n");
        page.push_str("error: could not compile due to 1 previous error\n");
        let fenced = wrap_external_content(
            &page,
            ContentSource::WebFetch {
                url: "https://example.test/log".into(),
            },
        );
        let mut value = json!({ "content": fenced });

        let reductions = clean_result_value(&mut value);
        assert_eq!(reductions.len(), 1, "the fenced page must reduce");
        let out = value["content"].as_str().unwrap();

        let split = crate::security::content_sanitizer::split_external_fence(out)
            .expect("the fence must survive the reduction intact");
        assert!(
            split.interior.contains("error[E0382]"),
            "the interior is what gets reduced; got:\n{}",
            split.interior
        );
        assert!(
            !split.interior.contains("Compiling dep-200"),
            "the reduction must still have happened inside the fence"
        );
        assert!(
            out.len() < fenced.len(),
            "rewrapping must not cost more than it saved"
        );
    }

    /// A fence whose ids do not match is not one fence, and must never be
    /// re-stitched around a rewritten interior.
    #[test]
    fn a_mismatched_fence_is_left_alone() {
        let mut body = String::from(
            "<<<EXTERNAL_UNTRUSTED_CONTENT id=\"aaaa\" source=\"web_fetch\">\nerror: boom\n",
        );
        for i in 0..400 {
            body.push_str(&format!("   Compiling dep-{i} v1.0.0\n"));
        }
        body.push_str("error: could not compile\n");
        body.push_str("<<<END_EXTERNAL_UNTRUSTED_CONTENT id=\"bbbb\">");
        let mut value = json!({ "content": body.clone() });

        let reductions = clean_result_value(&mut value);
        // Reduction is allowed (the whole field is then the payload), but the
        // splitter must not have claimed it as a well-formed fence: what matters
        // is that we never emit a *new* pairing of the two ids.
        if !reductions.is_empty() {
            let out = value["content"].as_str().unwrap();
            assert!(
                crate::security::content_sanitizer::split_external_fence(out).is_none(),
                "a mismatched pair must not be re-emitted as a valid fence: {out}"
            );
        }
    }

    /// Escapes are never content. When neither cleaner recognizes the shape, the
    /// stripped text still stands on its own — otherwise a colourised wall of
    /// unrecognized output carries every `ESC[…m` run into the context.
    #[test]
    fn unrecognized_colourised_output_still_loses_its_escapes() {
        // Ordinary prose (no error signal, no structure) wearing colour codes.
        let mut noisy = String::new();
        for i in 0..200 {
            noisy.push_str(&format!(
                "\u{1b}[2m\u{1b}[38;5;244mordinary explanatory sentence number {i} with no signal at all\u{1b}[0m\n"
            ));
        }
        let mut value = json!({ "message": noisy });

        let reductions = clean_result_value(&mut value);
        assert_eq!(reductions.len(), 1, "stripping alone is a reduction");
        assert_eq!(reductions[0].method, ReductionMethod::Sanitized);
        assert!(
            !reductions[0].method.is_lossy(),
            "sanitising drops no content, so it must not force an offload"
        );
        let out = value["message"].as_str().unwrap();
        assert!(
            !out.contains('\u{1b}'),
            "no escape byte may reach the model"
        );
        assert!(
            out.contains("ordinary explanatory sentence number 199"),
            "not one line may be dropped by the sanitize-only path"
        );
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
