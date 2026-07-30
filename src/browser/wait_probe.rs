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
    let probe = wait_probe_func(condition);
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let out = backend.evaluate(tab_id, &probe).await?;
        // The probe's result value is the only thing echoed back (possibly
        // JSON-encoded with quotes — `contains` tolerates either); the
        // sentinels cannot appear unless the probe returned them.
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
}
