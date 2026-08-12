//! `BrowserRuntimeProbe` — gates the `browser_*` tool family on the
//! presence of a usable browser runtime.
//!
//! Aleph routes browser work to one of two backends (see
//! `browser::manager::get_backend`), and each has its own prerequisite:
//!   * `BrowserDriver::ExistingSession` → `ChromeMcpBackend`, which attaches to
//!     a locally-installed Chromium by launching `npx chrome-devtools-mcp`. It
//!     needs **both** a Chromium binary ([`find_chromium`]) **and** `npx`.
//!   * `BrowserDriver::Managed` → `PlaywrightCliBackend`, which runs the
//!     ledger-provisioned `playwright-cli` binary (`browser::playwright_cli`
//!     resolves it through `runtimes::ensure_capability("playwright-cli", …)`)
//!     and brings its own Chromium. It does **not** run `npx`.
//!
//! Both profiles always exist — `ProfileManager::new` auto-injects a `default`
//! (managed) and a `user` (existing-session) profile — so the probe answers for
//! both drivers and reports `Healthy` when either one could actually run.
//!
//! # The question this used to ask, and why it was the wrong one
//!
//! Stage 2 used to be `which("npx")`, on the reasoning that the managed backend
//! "shells out to `npx playwright`". It does not, and has not since the CLI
//! moved into the capability ledger. On any machine with Node installed and no
//! browser provisioned — a plain developer laptop — `npx` resolved, the gate
//! opened, and 26 unusable browser tools shipped on every request. Asking the
//! ledger whether `playwright-cli` is `Ready` is the question that decides
//! whether a managed call can succeed.
//!
//! With neither driver runnable the family is withheld, which also means the
//! managed driver's own bootstrap (fnm → node → playwright-cli, a network
//! download) can no longer be triggered by a tool call. That is the intended
//! trade: the install belongs to the Panel's Runtimes page, which is where the
//! Browser settings banner sends the operator, not to a turn that pays 8.9 KB
//! for 26 tools whose first call would stall on a download.
//!
//! The one prerequisite this cannot see is an operator's explicit
//! `[browser.playwright_cli] binary_path`, because the probe is constructed
//! without config (`BrowserRuntimeProbe::new()` in `tool_catalog_init`). A
//! `playwright-cli` on `PATH` is accepted for that reason, which covers a
//! system-installed CLI; a CLI at a bespoke path with no ledger entry still
//! reads as absent.
//!
//! # Blocking work
//!
//! Every probe here is filesystem/`PATH` IO with no `.await` in it. Run inline
//! it would hold the runtime thread through the health cache's 200 ms
//! `PROBE_DEADLINE` — a deadline that could never fire, because nothing yielded
//! for the timer to preempt. It goes on `spawn_blocking`, matching the twin
//! doctor check (`diagnostics::checks::browser_runtime`), which makes the
//! deadline spendable. The default TTL is bumped to 5 min because a browser
//! install appearing or disappearing mid-session is rare.

use std::borrow::Cow;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;

use crate::browser::find_chromium;
use crate::runtimes::{get_runtimes_dir, CapabilityLedger};
use crate::tool_metadata::{HealthReason, ProbeResult, ToolHealthProbe};

/// TTL for browser-runtime probe results. Longer than the default because a
/// browser binary / provisioned CLI rarely appears or vanishes within a session.
const BROWSER_PROBE_TTL: Duration = Duration::from_secs(300);

/// Capability name the managed driver resolves through the runtime ledger —
/// the same string `browser::playwright_cli` passes to `ensure_capability`.
pub(crate) const MANAGED_CAPABILITY: &str = "playwright-cli";

/// The managed driver's CLI, if a call could reach one right now: a
/// system-installed `playwright-cli` on `PATH`, else the ledger's `Ready`
/// entry. `None` means a managed call would have to bootstrap (a network
/// download) before it could do anything.
///
/// `pub(crate)` because the doctor twin (`diagnostics::checks::browser_runtime`)
/// asks the same question for its own finding, and two subsystems answering
/// "is the managed driver provisioned?" differently is precisely how the
/// `which("npx")` mistake survived four rounds.
///
/// `load_or_create` is the read used deliberately: it re-validates every
/// `Ready` entry against the filesystem, so a binary deleted since the last
/// bootstrap reads as absent instead of as installed. It never installs
/// anything and never creates the ledger file — a sensor must not manufacture
/// what it measures.
pub(crate) fn managed_cli_path() -> Option<PathBuf> {
    if let Ok(path) = which::which(MANAGED_CAPABILITY) {
        return Some(path);
    }
    let dir = get_runtimes_dir().ok()?;
    CapabilityLedger::load_or_create(dir.join("ledger.json"))
        .executable(MANAGED_CAPABILITY)
        .map(std::path::Path::to_path_buf)
}

/// Whether the managed driver could run without bootstrapping first.
fn managed_driver_ready() -> bool {
    managed_cli_path().is_some()
}

/// Whether the existing-session driver could run: a system Chromium to attach
/// to, and the `npx` launcher that starts `chrome-devtools-mcp`. Either one
/// missing leaves the driver unable to open a page, so both are required.
pub(crate) fn existing_session_driver_ready() -> bool {
    find_chromium().is_ok() && which::which("npx").is_ok()
}

/// Reports whether *any* browser driver is reachable. Stateless — the shared
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
        // A join error means the blocking pool could not answer; treat that as
        // "no runtime" rather than as health, for the same reason the doctor
        // twin does — an unknown must never be read as healthy.
        let usable = tokio::task::spawn_blocking(|| {
            managed_driver_ready() || existing_session_driver_ready()
        })
        .await
        .unwrap_or(false);

        if usable {
            return ProbeResult::Healthy;
        }
        ProbeResult::Unhealthy {
            reason: HealthReason::DependencyDown(Cow::Borrowed(
                "no provisioned playwright-cli (managed driver) and no Chromium + npx \
                 (existing-session driver)",
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
    async fn probe_agrees_with_the_per_driver_prerequisites() {
        // The host's toolchain is not assumed: assert the probe agrees with the
        // same discovery primitives it delegates to. What this pins is that the
        // verdict is derived from BOTH drivers' real prerequisites — a machine
        // with `npx` but no provisioned CLI and no Chromium must read unhealthy,
        // which is exactly the case the old `which("npx")` stage got wrong.
        let runtime_present = managed_driver_ready() || existing_session_driver_ready();
        match BrowserRuntimeProbe::new().probe().await {
            ProbeResult::Healthy => assert!(
                runtime_present,
                "probe said Healthy but neither driver's prerequisites were met"
            ),
            ProbeResult::Unhealthy { reason, .. } => {
                assert!(
                    !runtime_present,
                    "probe said Unhealthy but a driver was runnable"
                );
                assert!(reason.short_label().contains("playwright-cli"));
            }
        }
    }

    #[test]
    fn npx_alone_is_not_a_managed_runtime() {
        // The defect in one line: `npx` says nothing about the managed driver,
        // which runs a ledger-provisioned binary. Whatever this host has, the
        // managed verdict must not be readable off the `npx` lookup.
        if which::which("npx").is_ok() && which::which(MANAGED_CAPABILITY).is_err() {
            let ledger_says_ready = get_runtimes_dir().is_ok_and(|dir| {
                CapabilityLedger::load_or_create(dir.join("ledger.json"))
                    .executable(MANAGED_CAPABILITY)
                    .is_some()
            });
            assert_eq!(
                managed_driver_ready(),
                ledger_says_ready,
                "the managed verdict must come from the ledger, not from `npx`"
            );
        }
    }

    #[test]
    fn existing_session_needs_chromium_and_npx_together() {
        // `chrome-devtools-mcp` is launched with `npx`, so a Chromium without a
        // Node launcher cannot attach — the driver's gate is a conjunction.
        assert_eq!(
            existing_session_driver_ready(),
            find_chromium().is_ok() && which::which("npx").is_ok()
        );
    }

    #[test]
    fn ttl_is_longer_than_default() {
        // Default cache TTL is 30 s; a browser install rarely changes, so the
        // probe opts into a coarser refresh cadence.
        assert!(BrowserRuntimeProbe::new().ttl() > Duration::from_secs(30));
    }
}
