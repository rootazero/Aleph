//! Test-only fake [`BrowserBackend`] — records every call and can be told to
//! fail at a chosen ordinal, so tool-level sequencing code (the batch tool)
//! can be tested without a live browser.

use std::sync::Mutex;

use async_trait::async_trait;

use super::backend::BrowserBackend;
use super::error::BrowserError;
use super::types::{
    ActionTarget, HistoryNav, ScreenshotOpts, ScreenshotOutput, ScrollDirection, SnapshotOutput,
    TabId, WaitCondition,
};

/// Compact target rendering for the recorded call log (matches the formats
/// the batch tool's tests assert on, e.g. `click:Ref{ref_id:"e5"}`).
fn fmt_target(target: &ActionTarget) -> String {
    match target {
        ActionTarget::Ref { ref_id } => format!("Ref{{ref_id:\"{ref_id}\"}}"),
        ActionTarget::Coordinates { x, y } => format!("Coords{{x:{x},y:{y}}}"),
    }
}

/// A [`BrowserBackend`] that never touches a browser: each method appends a
/// `verb:detail` entry to [`Self::calls`] and returns a trivial `Ok`.
///
/// `fail_at` (1-based ordinal over recorded calls) makes that one call return
/// `Err(BrowserError::ActionFailed("boom"))` instead — the batch abort test
/// drives its failure path through this.
///
/// `evaluate` returns the [`super::wait_probe::WAIT_PROBE_FOUND`] sentinel so
/// any code path that polls a wait condition through `evaluate` resolves on
/// the first probe. `wait_for` itself is overridden to record `wait:…` and
/// resolve immediately: the fake must not really sleep out a
/// [`WaitCondition::Time`] delay or tests pay wall-clock time for nothing.
pub(crate) struct FakeBackend {
    calls: Mutex<Vec<String>>,
    fail_at: Option<usize>,
}

impl FakeBackend {
    pub(crate) fn new(fail_at: Option<usize>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            fail_at,
        }
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
            return Err(BrowserError::ActionFailed("boom".into()));
        }
        Ok(())
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
        Ok("1: https://example.com".into())
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
            png_bytes: Vec::new(),
        })
    }

    async fn snapshot(&self, _tab_id: &str) -> Result<SnapshotOutput, BrowserError> {
        self.record("snapshot".into())?;
        Ok(SnapshotOutput {
            snapshot_text: String::new(),
            page_url: "https://example.com".into(),
            page_title: "Example".into(),
        })
    }

    async fn evaluate(&self, _tab_id: &str, js: &str) -> Result<String, BrowserError> {
        self.record(format!("evaluate:{js}"))?;
        // The wait-probe sentinel: default `wait_for` polling resolves on the
        // first probe instead of spinning until the timeout.
        Ok(super::wait_probe::WAIT_PROBE_FOUND.into())
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

    /// Overridden (the trait default returns Unsupported) because the batch
    /// tool needs it.
    async fn dblclick(&self, _tab_id: &str, target: ActionTarget) -> Result<(), BrowserError> {
        self.record(format!("dblclick:{}", fmt_target(&target)))
    }

    /// Overridden (the trait default returns Unsupported) because the batch
    /// tool needs it.
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
}
