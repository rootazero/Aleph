//! Shared polling implementation for `BrowserBackend::wait_for`.
//!
//! Neither backend has a native multi-condition wait primitive (the Chrome
//! `DevTools` MCP `wait_for` tool covers text only; playwright-cli has no wait
//! command at all), so the trait's default `wait_for` polls the page through
//! `evaluate` with a JS probe built here. The Chrome MCP backend overrides the
//! `Text` arm with its native tool and falls back to [`poll_wait_for`] for the
//! other conditions.

use std::time::Duration;

use super::backend::BrowserBackend;
use super::error::BrowserError;
use super::types::WaitCondition;

/// Sentinel the wait probe returns when the condition holds. A bare `true`
/// would be ambiguous — a boolean-valued page or the CLI's status lines could
/// also print a lone `true`; this token cannot occur by accident (the probe's
/// result value is the only thing echoed, never the page text).
pub(crate) const WAIT_PROBE_FOUND: &str = "ALEPH_WAIT_FOUND";

/// Sentinel the selector probe returns when `querySelector` throws — a
/// malformed selector is a caller error, not a "not yet present" condition,
/// so the polling loop surfaces it as an `Err` instead of timing out.
pub(crate) const WAIT_PROBE_ERROR: &str = "ALEPH_WAIT_ERROR";

/// Poll interval for the evaluate-based wait. Matches the interval the
/// Playwright backend used for its text-only wait before the probe was
/// generalized.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Build the JS probe `evaluate` runs to test whether `condition` holds on the
/// page. The probe is an arrow function (both drivers' eval primitives expect
/// one) resolving to [`WAIT_PROBE_FOUND`], `'absent'`, or — for a selector
/// that throws — [`WAIT_PROBE_ERROR`]. `serde_json::to_string` renders every
/// needle as a quoted, fully-escaped string literal, so arbitrary text (quotes,
/// backslashes, newlines) can never break out of the expression.
pub(crate) fn wait_probe_func(condition: &WaitCondition) -> String {
    match condition {
        WaitCondition::Text(text) => {
            let needle = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into());
            format!(
                "() => (!!document.body && document.body.innerText.includes({needle})) \
                 ? {WAIT_PROBE_FOUND:?} : 'absent'"
            )
        }
        WaitCondition::TextGone(text) => {
            let needle = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into());
            // No body yet ⇒ the text is trivially absent ⇒ found. That is the
            // correct polarity for "wait until the spinner disappears" — the
            // alternative (absent until a body exists) would hang on pages
            // that legitimately render no body text at all.
            format!(
                "() => (!!document.body && document.body.innerText.includes({needle})) \
                 ? 'absent' : {WAIT_PROBE_FOUND:?}"
            )
        }
        WaitCondition::Selector(selector) => {
            let needle = serde_json::to_string(selector).unwrap_or_else(|_| "\"\"".into());
            format!(
                "() => {{ try {{ return document.querySelector({needle}) !== null \
                 ? {WAIT_PROBE_FOUND:?} : 'absent'; }} catch (e) {{ return {WAIT_PROBE_ERROR:?}; }} }}"
            )
        }
        WaitCondition::UrlContains(substr) => {
            let needle = serde_json::to_string(substr).unwrap_or_else(|_| "\"\"".into());
            format!("() => location.href.includes({needle}) ? {WAIT_PROBE_FOUND:?} : 'absent'")
        }
        // `Time` never reaches this probe — `poll_wait_for` short-circuits it
        // into a plain sleep. This arm exists for match exhaustiveness only.
        WaitCondition::Time(_) => format!("() => {WAIT_PROBE_FOUND:?}"),
    }
}

/// Poll `backend.evaluate` with the condition's probe until it holds or the
/// budget elapses. Returns `Ok(false)` on timeout (absence is an answer, not
/// an error) and `Err` when the probe reports a malformed selector or an
/// evaluate call itself fails. This is the body of the trait's default
/// `wait_for`, factored out so the Chrome MCP backend's non-`Text` arms can
/// reuse it verbatim.
pub(crate) async fn poll_wait_for<B: BrowserBackend + ?Sized>(
    backend: &B,
    tab_id: &str,
    condition: &WaitCondition,
    timeout_ms: u64,
) -> Result<bool, BrowserError> {
    // A plain delay never touches the page: sleep it out and report found.
    // `ms` is pre-clamped into the safe window by the tool layer, and a delay
    // IS the condition — `timeout_ms` does not apply to it.
    if let WaitCondition::Time(ms) = condition {
        // BROWSER-R4-14: clamp the Time arm to timeout_ms so a caller
        // passing timeout_ms=60_000 with ms=120_000 does not silently
        // double its budget. The tool layer pre-clamps today, but
        // keeping the invariant at the predicate means a future direct
        // caller of poll_wait_for cannot bypass it either.
        let clamped_ms = (*ms).min(timeout_ms);
        tokio::time::sleep(Duration::from_millis(clamped_ms)).await;
        return Ok(true);
    }
    let probe = wait_probe_func(condition);
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let out = backend.evaluate(tab_id, &probe).await?;
        // This search is only sound because `evaluate` is contracted to return
        // the script's VALUE, not a transcript of the call. Both sentinels are
        // literals inside every probe built above, so a backend that echoed the
        // script back would satisfy `contains` on the first poll of every wait —
        // which is exactly what `playwright-cli eval` does (`### Ran Playwright
        // code`) and exactly what this code used to be handed. Every wait on the
        // default driver reported "found" immediately, for conditions that never
        // held, with no error anywhere; the guard for it lives in this file's
        // tests (`a_transcript_that_echoes_the_probe_does_not_read_as_found`)
        // because the invariant belongs to the predicate, not to one backend.
        if out.contains(WAIT_PROBE_FOUND) {
            return Ok(true);
        }
        if out.contains(WAIT_PROBE_ERROR) {
            return Err(BrowserError::ActionFailed(format!(
                "wait_for: invalid CSS selector {condition:?} — page rejected it"
            )));
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(POLL_INTERVAL.min(deadline - now)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::testkit::FakeBackend;

    /// The failure this file shipped with, pinned at the layer that owns it.
    ///
    /// Feeds `poll_wait_for`'s own predicate the shape a REAL `playwright-cli
    /// eval` returns — result section plus an echo of the script — and requires
    /// that the value extracted from it is the probe's answer, not a token
    /// lifted out of the probe's own source. Written against a condition that
    /// is FALSE, so a regression makes it red rather than merely less precise.
    #[test]
    fn a_transcript_that_echoes_the_probe_does_not_read_as_found() {
        use crate::browser::playwright_cli::parse_result_value;

        let probe = wait_probe_func(&WaitCondition::Text("never on this page".into()));
        // Pre-condition of the whole problem: the sentinel IS a literal in the
        // script. If that ever stops being true this test still passes, but the
        // assertion below stops being the interesting one — so state it.
        assert!(
            probe.contains(WAIT_PROBE_FOUND),
            "the probe embeds the sentinel; that is why the transcript is dangerous"
        );

        let transcript = format!(
            "### Result\n\"absent\"\n### Ran Playwright code\n```js\nawait page.evaluate('{probe}');\n```\n"
        );
        assert!(
            transcript.contains(WAIT_PROBE_FOUND),
            "the raw transcript contains the sentinel — searching it is the bug"
        );

        let value = parse_result_value(&transcript).expect("transcript has a ### Result section");
        assert!(
            !value.contains(WAIT_PROBE_FOUND),
            "the extracted value must not carry the echoed script: {value:?}"
        );
        assert_eq!(value, "\"absent\"");
    }

    /// The positive half: a transcript whose script really did return the
    /// sentinel must still read as found, or the fix would have turned a
    /// permanent "found" into a permanent "timed out".
    #[test]
    fn a_transcript_whose_script_returned_the_sentinel_reads_as_found() {
        use crate::browser::playwright_cli::parse_result_value;

        let probe = wait_probe_func(&WaitCondition::UrlContains("example".into()));
        let transcript = format!(
            "### Result\n\"{WAIT_PROBE_FOUND}\"\n### Ran Playwright code\n```js\nawait page.evaluate('{probe}');\n```\n"
        );
        let value = parse_result_value(&transcript).expect("transcript has a ### Result section");
        assert!(value.contains(WAIT_PROBE_FOUND), "got {value:?}");
    }

    #[test]
    fn text_probe_is_arrow_function_with_escaped_needle() {
        // Plain text embeds as a quoted literal inside an arrow function that
        // resolves to the sentinel (never a bare boolean — see WAIT_PROBE_FOUND).
        let f = wait_probe_func(&WaitCondition::Text("Loading done".into()));
        assert!(f.starts_with("() => "));
        assert!(f.contains("document.body.innerText.includes(\"Loading done\")"));
        assert!(f.contains("\"ALEPH_WAIT_FOUND\""));
        assert!(f.contains("'absent'"));
        // Quotes/backslashes/newlines must stay inside the string literal.
        let f = wait_probe_func(&WaitCondition::Text("she said \"hi\\\" \n".into()));
        assert!(f.starts_with("() => "));
        assert!(f.contains("\\\"hi\\\\\\\""));
        assert!(f.contains("\\n"));
        assert!(!f.contains('\n'), "raw newline would break the expression");
    }

    #[test]
    fn selector_probe_escapes_and_reports_syntax_errors() {
        let f = wait_probe_func(&WaitCondition::Selector("div.a > span[title=\"x\"]".into()));
        assert!(f.starts_with("() => "));
        // The selector is a JSON-escaped literal — quotes cannot break out.
        assert!(f.contains("document.querySelector(\"div.a > span[title=\\\"x\\\"]\")"));
        assert!(f.contains("\"ALEPH_WAIT_FOUND\""));
        // A throwing selector must surface as the error sentinel, not 'absent'.
        assert!(f.contains("catch (e)"));
        assert!(f.contains("\"ALEPH_WAIT_ERROR\""));
    }

    #[test]
    fn url_probe_matches_location_href() {
        let f = wait_probe_func(&WaitCondition::UrlContains("/dashboard?u=1".into()));
        assert!(f.starts_with("() => "));
        assert!(f.contains("location.href.includes(\"/dashboard?u=1\")"));
        assert!(f.contains("\"ALEPH_WAIT_FOUND\""));
    }

    // --- the three exits of the polling loop, plus the Time short-circuit ---

    #[tokio::test]
    async fn found_sentinel_resolves_true_on_the_first_probe() {
        let backend = FakeBackend::new(None);
        let found = poll_wait_for(&backend, "1", &WaitCondition::Text("hi".into()), 5_000)
            .await
            .expect("a probe that answers must not error");
        assert!(found);
        assert_eq!(backend.calls().len(), 1, "one probe, no spinning");
    }

    #[tokio::test]
    async fn error_sentinel_is_an_error_not_a_timeout() {
        // A malformed selector is a caller error: it must surface as Err even
        // though the budget has not elapsed — otherwise the model reads
        // "element never appeared" and waits again with the same bad selector.
        let backend = FakeBackend::new(None).with_evaluate_responses([WAIT_PROBE_ERROR]);
        let err = poll_wait_for(
            &backend,
            "1",
            &WaitCondition::Selector("div[".into()),
            60_000,
        )
        .await
        .expect_err("a rejected selector must be an error");
        assert!(
            err.to_string().contains("invalid CSS selector"),
            "got: {err}"
        );
        assert_eq!(
            backend.calls().len(),
            1,
            "must not keep polling a bad probe"
        );
    }

    #[tokio::test]
    async fn absent_until_the_budget_elapses_is_ok_false() {
        // Absence is an answer, not an error. A zero budget still probes once.
        let backend = FakeBackend::new(None).with_evaluate_responses(["absent"]);
        let found = poll_wait_for(&backend, "1", &WaitCondition::Text("nope".into()), 0)
            .await
            .expect("a timeout is not an error");
        assert!(!found);
        assert_eq!(backend.calls().len(), 1);
    }

    #[tokio::test]
    async fn time_condition_never_touches_the_page() {
        // A plain delay IS the condition: it must sleep, not probe.
        let backend = FakeBackend::new(None);
        let found = poll_wait_for(&backend, "1", &WaitCondition::Time(1), 60_000)
            .await
            .expect("a delay always resolves");
        assert!(found);
        assert!(
            backend.calls().is_empty(),
            "Time must short-circuit before any evaluate — got {:?}",
            backend.calls()
        );
    }
}
