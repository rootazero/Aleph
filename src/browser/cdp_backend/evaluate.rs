//! `evaluate` — the VALUE, never a transcript.
//!
//! `BrowserBackend::evaluate`'s contract (`backend.rs`) is load-bearing rather
//! than stylistic: `wait_probe::poll_wait_for` searches this string for
//! `WAIT_PROBE_FOUND`, a literal inside every probe it builds, so a backend
//! that echoed the script back would make every `wait_for` on this driver
//! report "found" on its first poll — which is exactly what the managed driver
//! did for as long as it existed.

use aleph_cdp::methods::runtime;

use crate::browser::error::BrowserError;

use super::{map_cdp_err, CdpBackend};

/// The wrapper that makes an arrow-function script actually run.
///
/// Every producer in the tool layer writes functions: `wait_probe_func` builds
/// `() => …` and the QA driver's page helper sends `() => (expr)`. CDP's
/// `Runtime.evaluate` on that source yields a *function object*, and
/// `returnByValue` cannot serialise one — so an unwrapped call would answer
/// every probe with the same empty value.
///
/// The "is it a function" test is made **in the page**, by the JS engine that
/// already has a parser, rather than by pattern-matching the source in Rust:
/// `(1 + 1)` and `(x) => x` both start with a parenthesis, and a Rust-side
/// guess would get one of them wrong.
const WRAP_PREFIX: &str = "(() => { const __aleph_v = (";
const WRAP_SUFFIX: &str =
    "); return typeof __aleph_v === 'function' ? __aleph_v() : __aleph_v; })()";

#[must_use]
pub(crate) fn as_call_expression(js: &str) -> String {
    format!("{WRAP_PREFIX}{}{WRAP_SUFFIX}", js.trim())
}

/// Render one CDP result value as the text the tool layer parses.
///
/// JSON, because `browser_tools::process_evaluate_result` parses this string as
/// JSON and unwraps a `Value::String` — the same shape
/// `playwright_cli::parse_result_value` produces, so the two drivers hand the
/// model the same thing for the same page.
fn render_value(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

/// A thrown script's diagnostic, with the contract enforced rather than hoped
/// for: the text must not carry anything a wait probe would read as its answer.
///
/// **Two predicates, and the second is the one that matters.** A full restatement
/// of the script is the obvious case and the easy one to catch. The real hazard
/// is a PARTIAL echo: V8 quotes a fragment around the syntax error
/// (`SyntaxError: Unexpected token near "…ALEPH_WAIT_FOUND…"`), which contains
/// no full copy of the script, sails past a whole-script check, reaches
/// `poll_wait_for`, satisfies `out.contains(WAIT_PROBE_FOUND)` — and re-opens
/// the exact silent "every `wait_for` reports found" failure this contract
/// exists to close. So the sentinels are checked directly, and they are the
/// authority: an engine that quotes one of them has, by definition, produced
/// text that cannot be handed back (判据 §3 — a guard only covers the shapes it
/// recognises, and the full-script shape is the one it recognised).
fn sanitise_exception(detail: &str, js: &str) -> String {
    use crate::browser::wait_probe::{WAIT_PROBE_ERROR, WAIT_PROBE_FOUND};

    if detail.contains(WAIT_PROBE_FOUND) || detail.contains(WAIT_PROBE_ERROR) {
        return "script threw; the engine's diagnostic quoted a wait-probe \
                sentinel and was withheld (returning it would make the next \
                wait_for report 'found' for a condition that never held)"
            .to_string();
    }
    let trimmed = js.trim();
    if !trimmed.is_empty() && detail.contains(trimmed) {
        return "script threw; the engine's diagnostic restated the script and \
                was withheld"
            .to_string();
    }
    format!("script threw: {detail}")
}

pub(super) async fn evaluate(
    be: &CdpBackend,
    tab_id: &str,
    js: &str,
) -> Result<String, BrowserError> {
    let handle = be.handle().await?;
    let session = handle.ensure_tab(tab_id).await?;
    let res = runtime::evaluate(
        &handle.conn,
        Some(&session),
        &as_call_expression(js),
        // Await a promise the script returns, so `async () => …` answers with
        // its value rather than with a pending-promise placeholder.
        true,
    )
    .await
    .map_err(|e| map_cdp_err(be.engine(), "Runtime.evaluate", e))?;

    match res.exception {
        Some(detail) => Ok(sanitise_exception(&detail, js)),
        None => Ok(render_value(&res.value)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::wait_probe::WAIT_PROBE_FOUND;

    /// The positive half of the transcript rule: a thrown script's diagnostic
    /// is useful, and must survive.
    #[test]
    fn an_exception_that_does_not_echo_the_script_is_passed_through() {
        let text = sanitise_exception(
            "TypeError: x is not a function\n    at <anonymous>:1:22",
            "() => x()",
        );
        assert!(text.contains("TypeError"), "{text}");
    }

    /// The negative half, written against a probe so the failure it prevents is
    /// the real one: an engine that echoed the script into its diagnostic would
    /// hand `poll_wait_for` the sentinel and make the wait report "found".
    #[test]
    fn an_exception_that_echoes_the_script_is_withheld() {
        let probe = crate::browser::wait_probe::wait_probe_func(
            &crate::browser::types::WaitCondition::Text("nope".into()),
        );
        let hostile = format!("SyntaxError while evaluating: {probe}");
        assert!(
            hostile.contains(WAIT_PROBE_FOUND),
            "precondition: the raw diagnostic carries the sentinel"
        );
        let text = sanitise_exception(&hostile, &probe);
        assert!(
            !text.contains(WAIT_PROBE_FOUND),
            "the sentinel must not reach the caller: {text}"
        );
    }

    /// The shape a whole-script check does NOT catch, and the one V8 actually
    /// produces: a fragment quoted around the error, carrying the sentinel but
    /// no full copy of the script. This is the test that keeps the guard honest
    /// — the one above only exercises the shape the guard already recognised.
    #[test]
    fn an_exception_that_echoes_only_a_fragment_is_still_withheld() {
        let probe = crate::browser::wait_probe::wait_probe_func(
            &crate::browser::types::WaitCondition::Text("nope".into()),
        );
        let fragment =
            format!("SyntaxError: Unexpected token near \"? {WAIT_PROBE_FOUND} : 'absent'\"");
        assert!(
            !fragment.contains(probe.trim()),
            "precondition: this diagnostic does NOT restate the whole script, \
             so a whole-script check would pass it through"
        );
        assert!(fragment.contains(WAIT_PROBE_FOUND));

        let text = sanitise_exception(&fragment, &probe);
        assert!(
            !text.contains(WAIT_PROBE_FOUND),
            "a fragment carrying the sentinel is as dangerous as a full echo: {text}"
        );
    }

    /// The four value shapes the tool layer has to round-trip.
    #[test]
    fn values_render_as_the_json_the_tool_layer_parses() {
        assert_eq!(render_value(&serde_json::json!("absent")), "\"absent\"");
        assert_eq!(render_value(&serde_json::json!(900)), "900");
        assert_eq!(render_value(&serde_json::json!(true)), "true");
        assert_eq!(render_value(&serde_json::json!(null)), "null");
    }

    /// The wrapper has to survive a script that is an EXPRESSION as well as one
    /// that is a function, and the discrimination happens in the page. Both
    /// forms must come out as one syntactically complete call expression —
    /// which is what `ends_with(")()")` on the wire assertion is checking, and
    /// this is the unit-level half of it.
    #[test]
    fn the_wrapper_calls_a_function_and_leaves_an_expression_alone() {
        let fn_form = as_call_expression("() => 3 + 4");
        let expr_form = as_call_expression("1 + 1");
        for wrapped in [&fn_form, &expr_form] {
            assert!(wrapped.starts_with("(() =>"), "{wrapped}");
            assert!(wrapped.ends_with(")()"), "{wrapped}");
            assert!(
                wrapped.contains("typeof __aleph_v === 'function'"),
                "the decision is made by the page's own parser: {wrapped}"
            );
        }
        assert!(fn_form.contains("() => 3 + 4"));
        assert!(expr_form.contains("1 + 1"));
        // Leading/trailing whitespace must not survive into the wrapper's
        // parenthesis — a trailing newline inside `( … )` is harmless, but a
        // caller reading the expression back should see its own script.
        assert_eq!(as_call_expression("  1 + 1\n"), expr_form);
    }
}
