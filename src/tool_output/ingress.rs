//! The ingress pass: everything that happens to a tool's result **before** it
//! is flattened into the model's context.
//!
//! One entry point, [`clean_for_ingress`], run from
//! `tools::scoped::dispatch::apply_layer_two`. It owns the ordering the rest of
//! §3.14 depends on, and the ordering is the whole design:
//!
//! ```text
//!   value (serde_json::Value, text fields still hold real newlines)
//!     │
//!     ├─ 1. per-tool compression   (unconditional, field-wise)
//!     ├─ 2. content-type hygiene   (only when already over budget, field-wise)
//!     └─ 3. flatten → the caller's persist / inline / truncate cascade
//! ```
//!
//! Steps 1 and 2 are field-wise for the same reason: `Value::to_string()`
//! escapes every newline and collapses the result onto one line, and *every*
//! cleaner in this module tree routes on line structure. Run after the flatten,
//! each of them sees a single line and either declines silently or — worse —
//! matches something inside the JSON envelope and presents a slice of it as
//! though it were the signal.
//!
//! Living here rather than inline in the dispatcher also makes the policy
//! testable without standing up a `ScopedToolService`.

use serde_json::Value;

use crate::context::budget::pressure::estimate_tokens_smart;

use super::compressor;
use super::fence::rewrite_interior;
use super::hygiene::{self, FieldReduction};
use super::walk::for_each_text_field;

/// What the ingress pass decided.
#[derive(Debug, Clone)]
pub struct IngressOutcome {
    /// The flattened text Layer 2 should budget and show the model.
    pub model_facing: String,
    /// The untouched original, when `model_facing` is a **lossy** view of it.
    ///
    /// Two things hang off this being `Some` (see
    /// [`apply_result_budget`](crate::tools::result_processing::apply_result_budget)):
    /// it is what gets persisted, so the dropped detail stays recoverable; and
    /// it is the signal that the reduction was content-typed, so the reduced
    /// body is worth inlining above the recovery marker.
    ///
    /// `None` when nothing was dropped — including the case where the only
    /// change was stripping ANSI escapes, which removes bytes that were never
    /// content and so must not trigger an offload.
    pub full_original: Option<String>,
    /// One entry per field the hygiene pass shortened. Tracing only.
    pub reductions: Vec<FieldReduction>,
}

/// Run the ingress pass over a tool's structured result.
///
/// `value` is consumed in place (the caller overwrites it with the processed
/// text afterwards, so there is nothing to preserve — and cloning it first cost
/// a full deep copy of results that can be megabytes).
#[must_use]
pub fn clean_for_ingress(tool: &str, value: &mut Value, budget: Option<usize>) -> IngressOutcome {
    // The true original, captured before anything touches the value: this is
    // what "Full output persisted" has to mean.
    let raw = flatten(value);

    // Step 1 — per-tool compression, unconditional (as it has always been).
    // `None` means the value is untouched, so `raw` is still the model-facing
    // text and can be moved into the outcome rather than copied.
    let compressed = compress_in_place(tool, value).then(|| flatten(value));

    // Step 2 — content-type hygiene, only when the result is *already* over the
    // tool's declared budget, so the overwhelming majority of tool calls are
    // byte-for-byte unaffected.
    let mut reductions = Vec::new();
    let mut cleaned = None;
    let before = estimate_tokens_smart(compressed.as_deref().unwrap_or(&raw));
    if budget.is_some_and(|limit| before > limit) {
        reductions = hygiene::clean_result_value(value);
        if !reductions.is_empty() {
            let candidate = flatten(value);
            // Hygiene's own "never grow" guard measures each field against the
            // value it walked, which is not the string it is about to displace.
            // For the DevTools family compression has already cut hard, so a
            // reduction that is a genuine win over the raw field can still be
            // larger than what we would otherwise send. Compare against that.
            if estimate_tokens_smart(&candidate) < before {
                cleaned = Some(candidate);
            } else {
                reductions.clear();
            }
        }
    }

    // Offload only what was actually *shortened*. A sanitize-only pass changes
    // the bytes without dropping a line, so persisting for it would write a file
    // the model has no reason to read — and would flip the caller onto its
    // "inline the signal" arm for a result that has no signal to inline.
    let lossy = reductions.iter().any(|r| r.method.is_lossy());
    match cleaned {
        Some(model_facing) => IngressOutcome {
            full_original: (lossy && raw != model_facing).then_some(raw),
            model_facing,
            reductions,
        },
        None => IngressOutcome {
            model_facing: compressed.unwrap_or(raw),
            full_original: None,
            reductions,
        },
    }
}

/// Flatten a structured result to the text the model reads.
///
/// A bare string is the text; anything else is its compact JSON form. The one
/// place this conversion happens on the ingress path — every stage upstream of
/// it must therefore work on the `Value`.
fn flatten(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Apply the per-tool compressor to every text field. Returns whether anything
/// changed.
///
/// Fenced fields are compressed **inside** the boundary: the markers are
/// structure, and a compressor that selects lines would otherwise drop the
/// closing one and leave the model an untrusted region with no end.
fn compress_in_place(tool: &str, value: &mut Value) -> bool {
    if !compressor::compresses(tool) {
        return false;
    }
    let mut changed = false;
    for_each_text_field(value, |_, text| {
        let Some(out) = rewrite_interior(text, |payload| {
            let compacted = compressor::compress_tool_output(tool, payload);
            (compacted != payload).then_some(compacted)
        }) else {
            return;
        };
        *text = out;
        changed = true;
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::content_sanitizer::{
        split_external_fence, wrap_external_content, ContentSource,
    };
    use serde_json::json;

    fn big_log() -> String {
        let mut s = String::from("$ cargo test\n");
        for i in 0..3000 {
            s.push_str(&format!("test suite::case_{i} ... ok\n"));
        }
        s.push_str("test suite::boom ... FAILED\n");
        s.push_str("test result: FAILED. 3000 passed; 1 failed\n");
        s
    }

    #[test]
    fn a_result_under_budget_is_untouched() {
        let mut value = json!({ "stdout": "ok\n", "exit_code": 0 });
        let out = clean_for_ingress("bash", &mut value, Some(8_000));
        assert_eq!(
            out.model_facing,
            json!({"exit_code":0,"stdout":"ok\n"}).to_string()
        );
        assert!(out.full_original.is_none());
        assert!(out.reductions.is_empty());
    }

    #[test]
    fn an_over_budget_log_is_reduced_and_the_original_is_offered_for_offload() {
        let mut value = json!({ "success": false, "stdout": big_log() });
        let out = clean_for_ingress("bash", &mut value, Some(500));

        assert_eq!(out.reductions.len(), 1);
        assert!(out.model_facing.contains("FAILED. 3000 passed"));
        assert!(!out.model_facing.contains("case_1500"));
        let full = out
            .full_original
            .expect("a lossy reduction must offer the original for offload");
        assert!(
            full.contains("case_1500"),
            "the offloaded blob must be the untouched original"
        );
    }

    /// The budget gate is what keeps the common case free.
    #[test]
    fn no_budget_means_no_hygiene() {
        let mut value = json!({ "stdout": big_log() });
        let out = clean_for_ingress("read_file", &mut value, None);
        assert!(out.reductions.is_empty());
        assert!(out.full_original.is_none());
        assert!(out.model_facing.contains("case_1500"));
    }

    /// Sanitising removes bytes that were never content, so it must not make the
    /// caller write a recovery file.
    #[test]
    fn a_sanitize_only_pass_does_not_ask_for_an_offload() {
        let mut noisy = String::new();
        for i in 0..300 {
            noisy.push_str(&format!(
                "\u{1b}[2mordinary explanatory sentence number {i} carrying no signal\u{1b}[0m\n"
            ));
        }
        let mut value = json!({ "message": noisy });
        let out = clean_for_ingress("some_tool", &mut value, Some(100));

        assert_eq!(out.reductions.len(), 1);
        assert!(!out.model_facing.contains("\\u001b"));
        assert!(
            out.full_original.is_none(),
            "nothing was dropped, so nothing has to be recoverable"
        );
    }

    /// The compressor sees the payload, not an envelope around it — the whole
    /// reason it is applied field-wise.
    #[test]
    fn the_per_tool_compressor_reaches_the_payload() {
        let mut snapshot = String::from("root\n");
        for i in 0..500 {
            snapshot.push_str(&format!("  generic \"filler node {i}\"\n"));
        }
        snapshot.push_str("  button \"Submit\"\n");
        snapshot.push_str("  link \"Home\"\n");
        assert!(
            snapshot.len() > 4 * 1024,
            "precondition: over the 4KB floor"
        );

        let mut value = json!({ "content": [ { "type": "text", "text": snapshot } ] });
        let out = clean_for_ingress("chrome__take_snapshot", &mut value, Some(8_000));

        let text = value["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("button \"Submit\"") && text.contains("link \"Home\""),
            "interactive nodes must survive; got:\n{text}"
        );
        assert!(
            text.contains("Snapshot compressed: kept 2 interactive elements"),
            "the real strategy must have run, not the fallback; got:\n{text}"
        );
        assert!(
            !text.contains("filler node 250"),
            "the non-interactive bulk must be gone"
        );
        assert!(out.model_facing.len() < snapshot.len() / 4);
    }

    /// …and it must not break a fence to do it.
    #[test]
    fn compression_stays_inside_the_untrusted_boundary() {
        let mut snapshot = String::from("root\n");
        for i in 0..500 {
            snapshot.push_str(&format!("  generic \"filler node {i}\"\n"));
        }
        snapshot.push_str("  button \"Submit\"\n");
        let fenced = wrap_external_content(
            &snapshot,
            ContentSource::McpTool {
                server: "chrome".into(),
                tool: "take_snapshot".into(),
            },
        );
        let mut value = json!({ "content": [ { "type": "text", "text": fenced } ] });
        let _ = clean_for_ingress("chrome__take_snapshot", &mut value, Some(8_000));

        let text = value["content"][0]["text"].as_str().unwrap();
        let split = split_external_fence(text).expect("the boundary must survive compression");
        assert!(split.interior.contains("button \"Submit\""));
        assert!(!split.interior.contains("filler node 250"));
    }

    /// End-to-end on the real `web_fetch` shape: a focus marker of our own ahead
    /// of a fenced page body. Both halves have to come out the far side — the
    /// marker because it is ours, the boundary because it is the only thing
    /// telling the model where the untrusted region ends.
    #[test]
    fn a_web_fetch_page_keeps_its_boundary_through_the_whole_pass() {
        let mut page = String::from("Release notes\n");
        for i in 0..600 {
            page.push_str(&format!(
                "- changelog entry {i} about nothing in particular\n"
            ));
        }
        page.push_str("error: the build failed on 2026-08-04\n");
        page.push_str("Total: 3 errors, 1 warning across 600 entries\n");
        let field = format!(
            "[fetch_focus: what broke?]\n\n{}",
            wrap_external_content(
                &page,
                ContentSource::WebFetch {
                    url: "https://example.test/notes".into(),
                },
            )
        );
        let mut value = json!({ "url": "https://example.test/notes", "content": field });

        let out = clean_for_ingress("web_fetch", &mut value, Some(400));

        let content = value["content"].as_str().unwrap();
        assert!(
            content.starts_with("[fetch_focus: what broke?]"),
            "our own marker must survive; got: {}",
            &content[..content.len().min(80)]
        );
        let split = split_external_fence(content).expect("the boundary must survive the reduction");
        assert!(split.interior.contains("error: the build failed"));
        assert!(!split.interior.contains("changelog entry 300"));
        assert!(
            out.full_original
                .is_some_and(|f| f.contains("changelog entry 300")),
            "the dropped entries must stay recoverable"
        );
    }

    /// A tool with no compressor never even walks its result.
    #[test]
    fn a_tool_without_a_compressor_is_free() {
        let mut value = json!({ "stdout": "unchanged\n" });
        let before = value.clone();
        let out = clean_for_ingress("bash", &mut value, Some(8_000));
        assert_eq!(value, before);
        assert_eq!(out.model_facing, flatten(&before));
    }
}
