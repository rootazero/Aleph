//! `BrowserRuntimeProbe` — gates the `browser_*` tool family on the
//! presence of a usable browser runtime.
//!
//! Aleph routes browser work to one of two backends (see
//! `browser::manager::get_backend`):
//!   * `BrowserDriver::ExistingSession` → `ChromeMcpBackend`, which drives a
//!     locally-installed Chromium over CDP (needs [`find_chromium`]).
//!   * `BrowserDriver::Managed` → `PlaywrightCliBackend`, which shells out to
//!     `npx playwright` and manages its own bundled browser.
//!
//! A probe keyed purely on [`find_chromium`] would therefore false-hide the
//! whole toolset whenever the user relies on the Playwright-managed backend.
//! To stay correct we report `Healthy` when **either** a local Chromium binary
//! **or** an `npx` launcher is discoverable — mirroring the doctor check's
//! two-pronged probe (`diagnostics::checks::browser_runtime`). Only when both
//! are absent is there genuinely no way to drive a browser, and the LLM should
//! not be offered the tools.
//!
//! Both lookups are cheap synchronous filesystem stats, comfortably inside the
//! cache's 200 ms probe deadline. The default TTL is bumped to 5 min because a
//! browser install appearing or disappearing mid-session is rare.

use std::borrow::Cow;
use std::time::Duration;

use async_trait::async_trait;

use crate::browser::find_chromium;
use crate::tool_metadata::{HealthReason, ProbeResult, ToolHealthProbe};

/// TTL for browser-runtime probe results. Longer than the default because a
/// browser binary / `npx` launcher rarely appears or vanishes within a session.
const BROWSER_PROBE_TTL: Duration = Duration::from_secs(300);

/// Reports whether *any* browser runtime is reachable. Stateless — the shared
/// `Arc<BrowserRuntimeProbe>` is registered under every `browser_*` tool name.
#[derive(Default)]
pub struct BrowserRuntimeProbe;

impl BrowserRuntimeProbe {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ToolHealthProbe for BrowserRuntimeProbe {
    async fn probe(&self) -> ProbeResult {
        // Stage 1: a locally-installed Chromium (CDP / ExistingSession path).
        if find_chromium().is_ok() {
            return ProbeResult::Healthy;
        }
        // Stage 2: an `npx` launcher (Playwright-managed path bundles its own
        // browser, so a Chromium binary is not required).
        if which::which("npx").is_ok() {
            return ProbeResult::Healthy;
        }
        ProbeResult::Unhealthy {
            reason: HealthReason::DependencyDown(Cow::Borrowed(
                "no Chromium binary and no `npx` launcher on PATH",
            )),
            retry_after: None,
        }
    }

    fn ttl(&self) -> Duration {
        BROWSER_PROBE_TTL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn probe_reports_healthy_when_a_runtime_exists() {
        // On any dev / CI host at least one of Chromium or `npx` is present;
        // when neither is, the probe correctly reports unhealthy. Assert the
        // probe agrees with the same discovery primitives it delegates to,
        // rather than assuming the host's toolchain.
        let runtime_present = find_chromium().is_ok() || which::which("npx").is_ok();
        match BrowserRuntimeProbe::new().probe().await {
            ProbeResult::Healthy => assert!(
                runtime_present,
                "probe said Healthy but neither Chromium nor npx was found"
            ),
            ProbeResult::Unhealthy { reason, .. } => {
                assert!(
                    !runtime_present,
                    "probe said Unhealthy but a runtime was discoverable"
                );
                assert!(reason.short_label().contains("Chromium"));
            }
        }
    }

    #[test]
    fn ttl_is_longer_than_default() {
        // Default cache TTL is 30 s; a browser install rarely changes, so the
        // probe opts into a coarser refresh cadence.
        assert!(BrowserRuntimeProbe::new().ttl() > Duration::from_secs(30));
    }
}
