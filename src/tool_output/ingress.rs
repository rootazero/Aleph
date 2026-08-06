//! The single ingress entry point: the local clean/trim/summarise pass a
//! tool's structured result goes through before it is flattened into the
//! model's context.
//!
//! Two stages run here, in this order, both on the `serde_json::Value` while
//! its text fields still carry real newlines:
//!
//! 1. [`compressor::compress_result_value`] — per-tool compression (the
//!    `DevTools` family). Field-level, because the per-string strategies read
//!    lines or bare JSON arrays and go blind on a serialized envelope.
//! 2. [`hygiene::clean_result_value`] — content-aware reduction, and only when
//!    the result is already over the tool's declared budget, so the
//!    overwhelming majority of tool calls are byte-for-byte unaffected.
//!
//! The contract the rest of the pipeline relies on:
//!
//! - **Persisted is always the pre-ingress original.** Compression is itself a
//!   lossy cut (a head/tail byte slice for `compress_generic`), so the moment
//!   a field is rewritten the pre-compression text is locked into
//!   [`IngressOutcome::reduced_from`]. Persisting the compressed copy used to
//!   make the reduction irreversible while still calling the file "Full
//!   output" — `ctx_search` could never dig the dropped nodes back.
//! - **Sanitising is not loss.** A `Sanitized` reduction stripped bytes that
//!   were never content (ANSI escapes, control bytes) and dropped no line, so
//!   it must not by itself trigger an offload + recovery footer. Only a
//!   genuinely lossy method ([`ReductionMethod::is_lossy`]) earns a persist.
//! - **The accepted replacement must be smaller than what it displaces**, not
//!   merely smaller than the raw field — for the `DevTools` family the
//!   compressor has already cut hard, and a hygiene reduction that is a real
//!   win over the raw field can still be several times larger than the
//!   compressed text it would replace.

use serde_json::Value;

use crate::context::budget::pressure::estimate_tokens_smart;

use super::compressor;
use super::hygiene::{self, FieldReduction};

/// What the ingress pass decided about one tool result.
pub(crate) struct IngressOutcome {
    /// The text the model should see, post compression and (when over budget)
    /// post hygiene. This is what the caller hands to
    /// [`apply_result_budget`](crate::tools::result_processing::apply_result_budget)
    /// and ultimately installs as the tool result.
    pub model_facing: String,
    /// The pre-ingress original, when (and only when) something lossy happened
    /// to it: compression, or a hygiene reduction whose method
    /// [`is_lossy`](hygiene::ReductionMethod::is_lossy). `None` means the
    /// model-facing text *is* the original (modulo bytes that were never
    /// content), so there is nothing to offload.
    pub reduced_from: Option<String>,
    /// One entry per field hygiene shortened — for tracing. Empty when hygiene
    /// did not run or its result was rejected (the fields it touched are then
    /// not part of `model_facing`, so reporting them would be a lie).
    pub reductions: Vec<FieldReduction>,
    /// Whether the per-tool compressor rewrote any field. When true,
    /// `reduced_from` is the pre-compression original by construction.
    pub compressed: bool,
}

/// Run the ingress clean over a tool result, in place, and return what the
/// rest of the pipeline needs.
///
/// `budget` is the tool's declared result budget; it both gates hygiene (which
/// only runs over budget) and sizes every compression/reduction knob through
/// [`scale_to_budget`](super::scale_to_budget).
///
/// # Why `value` is mutated even when hygiene's result is rejected
///
/// Hygiene edits fields in place and only afterwards is its flattened result
/// compared against the text it would displace; a rejected pass leaves the
/// mutations in `value`. That is safe because of how the caller uses this
/// function: `apply_layer_two` installs `outcome.model_facing` as the tool
/// result wholesale, so the value's post-rejection state is never observed.
/// Not cloning the value first is deliberate — an over-budget result is by
/// definition large, and deep-cloning it on the hot path was pure waste.
pub(crate) fn clean_for_ingress(
    tool_name: &str,
    value: &mut Value,
    budget: Option<usize>,
) -> IngressOutcome {
    let raw = flatten(value);

    // Compression first: it is per-tool and unconditional (cheap for anything
    // but the DevTools family), and a field rewrite is lossy by definition —
    // lock the pre-compression text as the original immediately.
    let compressed = compressor::compress_result_value(tool_name, value, budget);
    let (mut model_facing, mut original) = if compressed {
        (flatten(value), Some(raw))
    } else {
        (raw, None)
    };

    let mut reductions = Vec::new();
    if let Some(limit) = budget {
        // One estimate, two uses: the over-budget gate and the "must not grow"
        // comparison below. The previous shape estimated the same string twice.
        let before = estimate_tokens_smart(&model_facing);
        if before > limit {
            // The tool's own declared budget sizes the reduction — a tool that
            // declares 6 000 tokens gets a reduction sized for 6 000 tokens,
            // not a fixed-cap reduction finished off by a blind truncator.
            let rs = hygiene::clean_result_value(value, Some(limit));
            if !rs.is_empty() {
                let flattened = flatten(value);
                // Hygiene's own "never grow" guard measures each field against
                // the RAW value it walked, which is not the string it is about
                // to displace — for the DevTools family the compressor has
                // already cut hard, so a genuine win over the raw field can
                // still be larger than `model_facing`. Compare against what we
                // would otherwise send.
                if estimate_tokens_smart(&flattened) < before {
                    let prior = std::mem::replace(&mut model_facing, flattened);
                    // A sanitize-only reduction dropped no content (see
                    // `ReductionMethod::is_lossy`), so it must not flip
                    // `reduced_from` to `Some` and force an offload + recovery
                    // footer for bytes that were never content.
                    if original.is_none() && rs.iter().any(|r| r.method.is_lossy()) {
                        original = Some(prior);
                    }
                    reductions = rs;
                }
            }
        }
    }

    IngressOutcome {
        model_facing,
        reduced_from: original,
        reductions,
        compressed,
    }
}

/// Flatten a tool result the way the model-facing channel always has: a bare
/// string passes through untouched (MCP text results arrive this way, real
/// newlines and all); anything else is compact JSON via `Value::to_string()`.
fn flatten(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Cheap upper-bound-ish size of a tool result, for the caller's "is this big
/// enough to be worth a blocking worker" decision.
///
/// Sums string-field lengths plus a small per-node constant, without
/// serializing: `Value::to_string()` on a large object allocates the whole
/// flattened string, which is exactly the work the worker exists to offload —
/// paying it here to decide whether to pay it there would defeat the point.
/// Recursion depth is bounded by `serde_json`'s own parse-depth cap, so this
/// cannot overflow the stack on anything that was ever parsed.
pub(crate) fn size_hint(value: &Value) -> usize {
    match value {
        Value::String(s) => s.len(),
        Value::Array(items) => items.iter().map(size_hint).sum(),
        Value::Object(map) => map.iter().map(|(k, v)| k.len() + size_hint(v)).sum(),
        _ => 16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use crate::security::content_sanitizer::{
        split_external_fence, wrap_external_content, ContentSource,
    };

    /// Spec ①: a fenced `DevTools` snapshot inside the MCP content-block
    /// envelope. The old pipeline fed the flattened envelope to
    /// `compress_tool_output`, which saw one escaped line, fell into the
    /// structural-summary arm, and `cap_line` amputated the whole snapshot at
    /// 500 chars. Field-level ingress must keep the interactive nodes, keep
    /// the fence, and never amputate.
    #[test]
    fn a_fenced_devtools_snapshot_compresses_inside_its_fence() {
        let mut lines: Vec<String> = vec!["uid=1_0 RootWebArea \"QA\" url=\"http://x/\"".into()];
        for i in 0..120 {
            lines.push(format!(
                "  uid=1_{} StaticText \"Paragraph {i} of filler prose\"",
                i * 3 + 1
            ));
            lines.push(format!("  uid=1_{} button \"Apply {i}\"", i * 3 + 2));
            lines.push(format!(
                "  uid=1_{} link \"Note {i}\" url=\"http://x/#{i}\"",
                i * 3 + 3
            ));
        }
        let snapshot = lines.join("\n");
        assert!(snapshot.len() > 4 * 1024);
        let fenced = wrap_external_content(&snapshot, ContentSource::BrowserContent);
        let mut value = json!({ "content": [ { "type": "text", "text": fenced } ] });
        let raw = value.to_string();

        let outcome = clean_for_ingress("chrome_devtools__take_snapshot", &mut value, Some(8_000));

        assert!(outcome.compressed, "a 360-node snapshot must compress");
        // `model_facing` is the flattened envelope — quotes inside the payload
        // are JSON-escaped there, so the quoted-node assertions below read the
        // field itself; what model_facing proves is that the *envelope* is no
        // longer what got amputated at 500 chars.
        assert!(
            outcome.model_facing.contains("Apply 100"),
            "a node past the old 500-char amputation point must survive:\n{}",
            &outcome.model_facing[..outcome.model_facing.len().min(400)]
        );
        assert!(
            outcome
                .model_facing
                .contains("Snapshot compressed: kept 240 interactive"),
            "the interactive-node arm must run: {}",
            &outcome.model_facing[..outcome.model_facing.len().min(400)]
        );
        assert!(
            outcome
                .model_facing
                .contains("<<<END_EXTERNAL_UNTRUSTED_CONTENT"),
            "the fence must survive compression"
        );
        // Compression is lossy: the pre-compression original is locked in.
        assert_eq!(outcome.reduced_from.as_deref(), Some(raw.as_str()));
        // The text block itself still parses as a well-formed fence, with the
        // interactive nodes kept inside it.
        let text = value["content"][0]["text"]
            .as_str()
            .expect("stays a string");
        let split = split_external_fence(text).expect("the field's fence is intact");
        assert!(split.interior.contains("Snapshot compressed"));
        assert!(split.interior.contains("button \"Apply 100\""));
    }

    /// Spec ②: under budget, nothing may change — byte-for-byte.
    #[test]
    fn an_under_budget_result_is_byte_identical() {
        let mut value = json!({ "stdout": "error: one line\n", "exit_code": 1 });
        let before = value.clone();

        let outcome = clean_for_ingress("bash", &mut value, Some(8_000));

        assert_eq!(outcome.model_facing, before.to_string());
        assert!(outcome.reductions.is_empty());
        assert_eq!(outcome.reduced_from, None);
        assert!(!outcome.compressed);
        assert_eq!(value, before, "a rejected/absent pass mutates nothing here");
    }

    /// Spec ③: a sanitize-only reduction over budget must not trigger an
    /// offload — nothing a model could want back was dropped.
    #[test]
    fn a_sanitize_only_reduction_does_not_set_reduced_from() {
        // Colourised prose: over budget, no structure and no error signal, so
        // hygiene's only win is stripping the escapes (ReductionMethod::Sanitized).
        let mut noisy = String::new();
        for i in 0..2_000 {
            noisy.push_str(&format!(
                "\u{1b}[2m\u{1b}[38;5;244mordinary explanatory sentence number {i} with no signal at all\u{1b}[0m\n"
            ));
        }
        let mut value = json!({ "message": noisy });

        let outcome = clean_for_ingress("bash", &mut value, Some(8_000));

        assert!(
            !outcome.reductions.is_empty(),
            "stripping that many escapes is a reduction"
        );
        assert!(
            outcome.reductions.iter().all(|r| !r.method.is_lossy()),
            "every reduction must be sanitize-only: {:?}",
            outcome.reductions
        );
        assert_eq!(
            outcome.reduced_from, None,
            "sanitize-only must not force a persist"
        );
        assert!(
            !outcome.model_facing.contains('\u{1b}'),
            "the escapes are gone from what the model sees"
        );
        assert!(
            outcome.model_facing.contains("sentence number 1999"),
            "not one line may be dropped by the sanitize-only path"
        );
    }

    /// Spec ④: a lossy hygiene reduction persists the pre-hygiene original —
    /// the text the model could otherwise never get back.
    #[test]
    fn a_lossy_reduction_locks_the_pre_hygiene_text() {
        let mut log = String::from("$ cargo test --lib\n\nrunning 2001 tests\n");
        for i in 0..2_000 {
            log.push_str(&format!("test suite::case_{i} ... ok\n"));
        }
        log.push_str("test suite::the_broken_one ... FAILED\n");
        log.push_str("thread 'suite::the_broken_one' panicked at src/widget.rs:42:9:\n");
        log.push_str("test result: FAILED. 2000 passed; 1 failed; 0 ignored\n");
        let mut value = json!({ "success": false, "stdout": log.clone(), "exit_code": 101 });
        let raw = value.to_string();

        let outcome = clean_for_ingress("bash", &mut value, Some(8_000));

        assert!(!outcome.compressed);
        assert!(
            outcome.reductions.iter().any(|r| r.method.is_lossy()),
            "a 2000-line test log must reduce lossily"
        );
        assert_eq!(
            outcome.reduced_from.as_deref(),
            Some(raw.as_str()),
            "the persisted blob is the pre-ingress original, not the reduction"
        );
        assert!(
            estimate_tokens_smart(&outcome.model_facing) < estimate_tokens_smart(&raw),
            "the model-facing text actually shrank"
        );
        assert!(
            outcome.model_facing.contains("the_broken_one"),
            "the failing test's name must survive:\n{}",
            &outcome.model_facing[..outcome.model_facing.len().min(400)]
        );
    }

    /// Spec ⑤: when compression fires, `reduced_from` is the pre-COMPRESSION
    /// original even if hygiene never runs.
    #[test]
    fn compression_alone_locks_the_pre_compression_text() {
        let base64 = format!("data:image/png;base64,{}", "A".repeat(50_000));
        let mut value = json!({ "content": [ { "type": "text", "text": base64 } ] });
        let raw = value.to_string();

        let outcome = clean_for_ingress("take_screenshot", &mut value, Some(8_000));

        assert!(outcome.compressed);
        assert!(
            outcome
                .model_facing
                .contains("[Screenshot captured successfully]"),
            "got: {}",
            &outcome.model_facing[..outcome.model_facing.len().min(200)]
        );
        assert_eq!(outcome.reduced_from.as_deref(), Some(raw.as_str()));
        assert!(outcome.reductions.is_empty(), "hygiene never ran");
    }

    #[test]
    fn size_hint_tracks_the_flattened_size_without_serializing() {
        let value = json!({ "a": "xxxx", "b": ["yy", { "c": "z" }], "n": 7 });
        let hint = size_hint(&value);
        assert!(hint >= 7, "every string byte is counted: {hint}");
        assert!(
            hint < value.to_string().len(),
            "and it skips the syntax: {hint}"
        );
    }
}
