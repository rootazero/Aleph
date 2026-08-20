//! Test-only fake [`BrowserBackend`] — records every call, can be told to
//! fail at a chosen ordinal, and can be handed the page text it should answer
//! with, so tool-level sequencing code (`browser_exec`), the post-navigation
//! audit and the wait probe can all be tested without a live browser.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;

use super::backend::BrowserBackend;
use super::error::BrowserError;
use super::types::{
    ActionTarget, CookieOp, EmulateOptions, HistoryNav, ScreenshotOpts, ScreenshotOutput,
    ScrollDirection, SnapshotOutput, TabId, WaitCondition,
};

/// Compact target rendering for the recorded call log (matches the formats
/// the batch tool's tests assert on, e.g. `click:Ref{ref_id:"e5"}`).
fn fmt_target(target: &ActionTarget) -> String {
    match target {
        ActionTarget::Ref { ref_id } => format!("Ref{{ref_id:\"{ref_id}\"}}"),
        ActionTarget::Coordinates { x, y } => format!("Coords{{x:{x},y:{y}}}"),
    }
}

/// Default `list_tabs` answer — a single clean public tab.
const DEFAULT_TABS_TEXT: &str = "1: https://example.com";

/// Default text of the error the `fail_at` call returns.
const DEFAULT_FAILURE_MESSAGE: &str = "boom";

/// A [`BrowserBackend`] that never touches a browser: each method appends a
/// `verb:detail` entry to [`Self::calls`] and returns a trivial `Ok`.
///
/// `fail_at` (1-based ordinal over recorded calls) makes that one call return
/// `Err(BrowserError::ActionFailed(…))` instead — the `browser_exec` abort test
/// drives its failure path through this. The text defaults to
/// [`DEFAULT_FAILURE_MESSAGE`]; [`Self::with_failure_message`] replaces it, so a
/// test can hand the tool layer an error shaped like the real thing — raw
/// playwright-cli stderr, credential-bearing and unbounded — rather than a tidy
/// token that every egress transform is a no-op on.
///
/// The `with_*` builders override what the fake *answers*; every one of them
/// defaults to the value the fake returned before it existed, so a test that
/// does not call them sees the historical behaviour unchanged.
///
/// `evaluate` returns the [`super::wait_probe::WAIT_PROBE_FOUND`] sentinel by
/// default so any code path that polls a wait condition through `evaluate`
/// resolves on the first probe. `wait_for` itself is overridden to record
/// `wait:…` and resolve immediately: the fake must not really sleep out a
/// [`WaitCondition::Time`] delay or tests pay wall-clock time for nothing.
/// (Tests that exercise the polling loop itself call
/// `wait_probe::poll_wait_for` directly and steer it with
/// [`Self::with_evaluate_responses`].)
pub(crate) struct FakeBackend {
    calls: Mutex<Vec<String>>,
    fail_at: Option<usize>,
    failure_message: String,
    tabs_text: String,
    snapshot_text: String,
    screenshot_png: Vec<u8>,
    console_text: String,
    network_text: String,
    /// Queued `evaluate` answers. The last entry sticks once the queue is down
    /// to one, so a polling loop can be given a steady "absent" without
    /// guessing how many probes it will run.
    evaluate_responses: Mutex<VecDeque<String>>,
}

impl FakeBackend {
    pub(crate) fn new(fail_at: Option<usize>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            fail_at,
            failure_message: DEFAULT_FAILURE_MESSAGE.to_string(),
            tabs_text: DEFAULT_TABS_TEXT.to_string(),
            snapshot_text: String::new(),
            screenshot_png: Vec::new(),
            console_text: String::new(),
            network_text: String::new(),
            evaluate_responses: Mutex::new(VecDeque::new()),
        }
    }

    /// What the `fail_at` call's [`BrowserError::ActionFailed`] carries
    /// (default: [`DEFAULT_FAILURE_MESSAGE`]).
    pub(crate) fn with_failure_message(mut self, message: impl Into<String>) -> Self {
        self.failure_message = message.into();
        self
    }

    /// What `list_tabs` answers (default: [`DEFAULT_TABS_TEXT`]).
    pub(crate) fn with_tabs_text(mut self, text: impl Into<String>) -> Self {
        self.tabs_text = text.into();
        self
    }

    /// What `snapshot` puts in `snapshot_text` (default: empty).
    pub(crate) fn with_snapshot_text(mut self, text: impl Into<String>) -> Self {
        self.snapshot_text = text.into();
        self
    }

    /// What `screenshot` answers with (default: zero bytes).
    ///
    /// The bytes need not be a real PNG: `bound_screenshot_png` returns its
    /// input unchanged when decoding fails, so a caller can hand over a payload
    /// of a chosen *size* to drive the parts of the pipeline that care about
    /// size — the inline-image hoist's `> 256` base64 floor above all, which a
    /// zero-byte default silently sits below.
    pub(crate) fn with_screenshot_png(mut self, png: impl Into<Vec<u8>>) -> Self {
        self.screenshot_png = png.into();
        self
    }

    /// What `console_messages` answers with (default: empty).
    pub(crate) fn with_console_text(mut self, text: impl Into<String>) -> Self {
        self.console_text = text.into();
        self
    }

    /// What `network_log` answers with (default: empty).
    pub(crate) fn with_network_text(mut self, text: impl Into<String>) -> Self {
        self.network_text = text.into();
        self
    }

    /// Queue the `evaluate` answers, in order. Once one entry remains it is
    /// repeated for every further call; an empty queue falls back to the
    /// wait-probe "found" sentinel.
    pub(crate) fn with_evaluate_responses<I, S>(self, responses: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        {
            let mut q = self
                .evaluate_responses
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            q.clear();
            q.extend(responses.into_iter().map(Into::into));
        }
        self
    }

    /// Recorded calls, in order.
    pub(crate) fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Append `entry` to the call log; the `fail_at`-th recorded call fails.
    fn record(&self, entry: String) -> Result<(), BrowserError> {
        let mut calls = self.calls.lock().unwrap_or_else(|e| e.into_inner());
        calls.push(entry);
        if self.fail_at == Some(calls.len()) {
            return Err(BrowserError::ActionFailed(self.failure_message.clone()));
        }
        Ok(())
    }

    /// Next queued `evaluate` answer (see [`Self::with_evaluate_responses`]).
    fn next_evaluate_response(&self) -> String {
        let mut q = self
            .evaluate_responses
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match q.len() {
            0 => super::wait_probe::WAIT_PROBE_FOUND.to_string(),
            1 => q[0].clone(),
            _ => q.pop_front().unwrap_or_default(),
        }
    }
}

#[async_trait]
impl BrowserBackend for FakeBackend {
    async fn open_tab(&self, url: &str) -> Result<TabId, BrowserError> {
        self.record(format!("open_tab:{url}"))?;
        Ok("1".into())
    }

    async fn close_tab(&self, tab_id: &str) -> Result<(), BrowserError> {
        self.record(format!("close_tab:{tab_id}"))
    }

    async fn list_tabs(&self) -> Result<String, BrowserError> {
        self.record("list_tabs".into())?;
        Ok(self.tabs_text.clone())
    }

    async fn navigate(&self, tab_id: &str, url: &str) -> Result<(), BrowserError> {
        self.record(format!("navigate:{tab_id}:{url}"))
    }

    async fn click(&self, _tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        self.record(format!("click:{}", fmt_target(&target)))
    }

    async fn type_text(
        &self,
        _tab_id: &str,
        _target: ActionTarget,
        text: &str,
    ) -> Result<(), BrowserError> {
        self.record(format!("type_text:{text}"))
    }

    async fn fill(
        &self,
        _tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError> {
        self.record(format!("fill:{}:{value}", fmt_target(&target)))
    }

    async fn hover(&self, _tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        self.record(format!("hover:{}", fmt_target(&target)))
    }

    async fn scroll(
        &self,
        _tab_id: &str,
        _target: ActionTarget,
        direction: ScrollDirection,
    ) -> Result<(), BrowserError> {
        self.record(format!("scroll:{direction:?}"))
    }

    async fn screenshot(
        &self,
        _tab_id: &str,
        _opts: ScreenshotOpts,
    ) -> Result<ScreenshotOutput, BrowserError> {
        self.record("screenshot".into())?;
        Ok(ScreenshotOutput {
            png_bytes: self.screenshot_png.clone(),
        })
    }

    async fn snapshot(&self, _tab_id: &str) -> Result<SnapshotOutput, BrowserError> {
        self.record("snapshot".into())?;
        Ok(SnapshotOutput {
            snapshot_text: self.snapshot_text.clone(),
            page_url: "https://example.com".into(),
            page_title: "Example".into(),
        })
    }

    async fn evaluate(&self, _tab_id: &str, js: &str) -> Result<String, BrowserError> {
        self.record(format!("evaluate:{js}"))?;
        Ok(self.next_evaluate_response())
    }

    async fn select(
        &self,
        _tab_id: &str,
        target: ActionTarget,
        value: &str,
    ) -> Result<(), BrowserError> {
        self.record(format!("select:{}:{value}", fmt_target(&target)))
    }

    async fn history(&self, tab_id: &str, nav: HistoryNav) -> Result<(), BrowserError> {
        self.record(format!("history:{tab_id}:{nav:?}"))
    }

    async fn dblclick(&self, _tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        self.record(format!("dblclick:{}", fmt_target(&target)))
    }

    async fn press_key(&self, _tab_id: &str, key: &str) -> Result<(), BrowserError> {
        self.record(format!("press_key:{key}"))
    }

    /// Records the condition and resolves immediately — the fake never polls
    /// and never sleeps out a `Time` delay (see the type-level doc).
    async fn wait_for(
        &self,
        _tab_id: &str,
        condition: &WaitCondition,
        _timeout_ms: u64,
    ) -> Result<bool, BrowserError> {
        self.record(format!("wait:{condition:?}"))?;
        Ok(true)
    }

    async fn console_messages(&self, _tab_id: &str) -> Result<String, BrowserError> {
        self.record("console_messages".into())?;
        Ok(self.console_text.clone())
    }

    async fn network_log(&self, _tab_id: &str) -> Result<String, BrowserError> {
        self.record("network_log".into())?;
        Ok(self.network_text.clone())
    }

    async fn pdf(&self, _tab_id: &str, output_path: &Path) -> Result<(), BrowserError> {
        self.record(format!("pdf:{}", output_path.display()))
    }

    async fn switch_tab(&self, tab_id: &str) -> Result<(), BrowserError> {
        self.record(format!("switch_tab:{tab_id}"))
    }

    async fn handle_dialog(
        &self,
        _tab_id: &str,
        action: &str,
        prompt_text: Option<&str>,
    ) -> Result<(), BrowserError> {
        self.record(format!(
            "handle_dialog:{action}:{}",
            prompt_text.unwrap_or("")
        ))
    }

    async fn drag(
        &self,
        _tab_id: &str,
        from: ActionTarget,
        to: ActionTarget,
    ) -> Result<(), BrowserError> {
        self.record(format!("drag:{}:{}", fmt_target(&from), fmt_target(&to)))
    }

    async fn upload(
        &self,
        _tab_id: &str,
        target: Option<ActionTarget>,
        paths: &[String],
    ) -> Result<(), BrowserError> {
        let target = target.as_ref().map_or_else(String::new, fmt_target);
        self.record(format!("upload:{target}:{}", paths.join(",")))
    }

    async fn resize(&self, _tab_id: &str, width: u32, height: u32) -> Result<(), BrowserError> {
        self.record(format!("resize:{width}x{height}"))
    }

    async fn emulate(&self, _tab_id: &str, opts: &EmulateOptions) -> Result<(), BrowserError> {
        self.record(format!("emulate:{opts:?}"))
    }

    async fn save_state(&self, path: &Path) -> Result<(), BrowserError> {
        self.record(format!("save_state:{}", path.display()))
    }

    async fn load_state(&self, path: &Path) -> Result<(), BrowserError> {
        self.record(format!("load_state:{}", path.display()))
    }

    async fn cookies(&self, op: &CookieOp) -> Result<String, BrowserError> {
        self.record(format!("cookies:{op:?}"))?;
        Ok(String::new())
    }

    /// Recorded as one call (like the MCP backend's native `fill_form`) rather
    /// than delegating to the trait's per-field loop, so a test can tell the
    /// batch verb apart from N individual fills.
    async fn fill_form(
        &self,
        _tab_id: &str,
        fields: &[(ActionTarget, String)],
    ) -> Result<usize, BrowserError> {
        self.record(format!("fill_form:{}", fields.len()))?;
        Ok(fields.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `BrowserBackend` method must be answered by the fake.
    ///
    /// A method left on a trait default is a hole in the test double that the
    /// compiler cannot see: the tool layer calls it, gets the default's error
    /// (or the default's *implementation*), and the test observes something the
    /// real backends never do. Source-level because the check is "is it
    /// declared here", which no runtime reflection can answer in Rust.
    ///
    /// CRLF-safe: `\r` is stripped before any splitting and the split token is
    /// not anchored to a line boundary — on a Windows checkout an anchored
    /// `"\n…"` token matches nothing and the guard silently scans its own test
    /// module instead of the production code (CLAUDE.md §10).
    #[test]
    fn fake_backend_implements_every_backend_method() {
        let trait_src = include_str!("backend.rs").replace('\r', "");
        let fake_src = include_str!("testkit.rs").replace('\r', "");
        // Only the production half of testkit.rs counts — a name mentioned in
        // this very test must not satisfy the census.
        let fake_prod = fake_src
            .split("#[cfg(test)]")
            .next()
            .unwrap_or(&fake_src)
            .to_string();

        let methods: Vec<String> = trait_src
            .match_indices("async fn ")
            .filter_map(|(idx, _)| {
                let rest = &trait_src[idx + "async fn ".len()..];
                let end = rest.find(|c: char| !c.is_alphanumeric() && c != '_')?;
                Some(rest[..end].to_string())
            })
            .collect();
        assert!(
            methods.len() > 20,
            "the trait scan found only {} methods — the extractor, not the trait, is broken",
            methods.len()
        );

        let missing: Vec<&String> = methods
            .iter()
            .filter(|m| !fake_prod.contains(&format!("async fn {m}(")))
            .collect();
        assert!(
            missing.is_empty(),
            "FakeBackend leaves these BrowserBackend methods on the trait default: {missing:?}"
        );
    }

    #[tokio::test]
    async fn builders_default_to_the_historical_answers() {
        let fake = FakeBackend::new(None);
        assert_eq!(fake.list_tabs().await.unwrap(), DEFAULT_TABS_TEXT);
        assert_eq!(fake.snapshot("1").await.unwrap().snapshot_text, "");
        assert_eq!(
            fake.evaluate("1", "() => 1").await.unwrap(),
            super::super::wait_probe::WAIT_PROBE_FOUND
        );
        // The reads that gained a builder later: their defaults are the empty
        // answers every pre-existing test was written against.
        assert!(fake
            .screenshot("1", ScreenshotOpts::default())
            .await
            .unwrap()
            .png_bytes
            .is_empty());
        assert_eq!(fake.console_messages("1").await.unwrap(), "");
        assert_eq!(fake.network_log("1").await.unwrap(), "");
        // The failure text is a builder too, so its default is pinned here with
        // the rest: the existing abort tests match on `boom`.
        let failing = FakeBackend::new(Some(1));
        let err = failing.click(
            "1",
            ActionTarget::Ref {
                ref_id: "e1".into(),
            },
        );
        assert!(err.await.unwrap_err().to_string().contains("boom"));
    }

    #[tokio::test]
    async fn the_failure_message_is_replaceable() {
        let fake = FakeBackend::new(Some(1)).with_failure_message("playwright: 401 for token X");
        let err = fake
            .click(
                "1",
                ActionTarget::Ref {
                    ref_id: "e1".into(),
                },
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("401 for token X"));
    }

    #[tokio::test]
    async fn evaluate_queue_replays_then_sticks_on_its_last_entry() {
        let fake = FakeBackend::new(None).with_evaluate_responses(["first", "absent"]);
        assert_eq!(fake.evaluate("1", "p").await.unwrap(), "first");
        assert_eq!(fake.evaluate("1", "p").await.unwrap(), "absent");
        assert_eq!(fake.evaluate("1", "p").await.unwrap(), "absent");
    }
}
