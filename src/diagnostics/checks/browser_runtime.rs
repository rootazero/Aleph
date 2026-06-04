//! `browser/runtime` — the browser subsystem's external prerequisites.
//!
//! Aleph drives two browser backends and each has a distinct host dependency:
//! the **existing-session** driver attaches to the user's real Chrome via
//! `chrome-devtools-mcp` (needs a system Chromium *and* `npx`/Node), while the
//! **managed** driver provisions its own Chromium through Playwright and only
//! benefits from a headed display on Linux. When any of these is absent the
//! failure today surfaces deep inside a tool call ("Chromium binary not found",
//! "Failed to attach to browser") with no up-front signal. This check makes the
//! browser runtime as observable as the core data dir / lock / config — the same
//! ground both `openclaw browser-doctor` and hermes' `_browser_cdp_check` cover.
//!
//! All probes are read-only environment/binary lookups (no process spawns, no
//! network), so the check is non-repairable: installing a browser or Node is a
//! network download, not a mechanical fix, so we surface a `fix_hint` instead.
//! The two binary probes run **concurrently** via `tokio::join!` — surpassing
//! the reference doctors' sequential probing.

use async_trait::async_trait;

use crate::browser::find_chromium;
use crate::diagnostics::check::{HealthCheck, Posture};
use crate::diagnostics::finding::Finding;

const ID: &str = "browser/runtime";

/// Outcome of the Chromium binary probe.
enum ChromiumProbe {
    Found(String),
    Missing,
}

/// Outcome of the Node/`npx` probe (the existing-session driver shells out to
/// `npx chrome-devtools-mcp`).
enum NodeProbe {
    Found(String),
    Missing,
}

#[derive(Default)]
pub struct BrowserRuntimeCheck;

impl BrowserRuntimeCheck {
    pub fn new() -> Self {
        Self
    }

    /// Locate a system Chromium-family browser. Runs the sync discovery on a
    /// blocking thread so it never stalls the diagnostics runtime.
    async fn probe_chromium() -> ChromiumProbe {
        tokio::task::spawn_blocking(|| match find_chromium() {
            Ok(path) => ChromiumProbe::Found(path.display().to_string()),
            Err(_) => ChromiumProbe::Missing,
        })
        .await
        .unwrap_or(ChromiumProbe::Missing)
    }

    /// Locate `npx` on `PATH` — the existing-session driver launches
    /// `npx chrome-devtools-mcp@latest` (see `ChromeMcpConfig` defaults).
    async fn probe_node() -> NodeProbe {
        tokio::task::spawn_blocking(|| match which::which("npx") {
            Ok(path) => NodeProbe::Found(path.display().to_string()),
            Err(_) => NodeProbe::Missing,
        })
        .await
        .unwrap_or(NodeProbe::Missing)
    }

    /// On Linux a *headed* managed browser needs an X11/Wayland display; with
    /// none set Playwright silently falls back to headless. Returns `true` when
    /// a display server is reachable. Non-Linux platforms always have a native
    /// window server, so this is `true` there.
    fn has_display() -> bool {
        if cfg!(target_os = "linux") {
            std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty())
                || std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
        } else {
            true
        }
    }
}

#[async_trait]
impl HealthCheck for BrowserRuntimeCheck {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> &'static str {
        "Browser runtime"
    }

    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        // Both binary probes are independent — run them concurrently.
        let (chromium, node) = tokio::join!(Self::probe_chromium(), Self::probe_node());

        let mut findings = Vec::with_capacity(3);

        // Every finding is Info-level (advisory). The browser is an *optional*
        // subsystem and both drivers have legitimate prerequisite-free setups
        // (the managed driver provisions its own Chromium; a managed-only
        // headless server needs neither system Chrome nor Node), so a missing
        // prerequisite must never flip `aleph doctor`'s exit gate. The full
        // `detail` + `fix_hint` still reach the JSON / `doctor`-tool surface.
        findings.push(match chromium {
            ChromiumProbe::Found(path) => Finding::ok(
                ID,
                "System browser detected",
                format!("Chromium-family binary at {path}."),
            ),
            ChromiumProbe::Missing => Finding::ok(
                ID,
                "System browser not detected (managed driver unaffected)",
                "No Chrome/Chromium/Brave/Edge on PATH or in the well-known install \
                 locations. The existing-session driver (attach to your own Chrome) \
                 needs one; the managed driver provisions its own Chromium via \
                 Playwright and works without it.",
            )
            .with_fix_hint(
                "Install Chrome/Chromium, or set ALEPH_CHROME_PATH, to enable the \
                 existing-session (attach) driver. Managed-only setups can ignore this.",
            ),
        });

        findings.push(match node {
            NodeProbe::Found(path) => Finding::ok(
                ID,
                "Node runtime detected",
                format!("npx at {path} (used by the existing-session chrome-devtools-mcp driver)."),
            ),
            NodeProbe::Missing => Finding::ok(
                ID,
                "Node/npx not detected (existing-session driver unavailable)",
                "The existing-session driver launches `npx chrome-devtools-mcp`; \
                 without Node on PATH it cannot attach to your Chrome. The managed \
                 Playwright driver is unaffected.",
            )
            .with_fix_hint(
                "Install Node.js (which provides `npx`) to enable the existing-session \
                 browser driver.",
            ),
        });

        // Linux-only: note when a headed managed browser would degrade to headless.
        if !Self::has_display() {
            findings.push(
                Finding::ok(
                    ID,
                    "No display server detected (managed browsing runs headless)",
                    "Neither DISPLAY nor WAYLAND_DISPLAY is set; a headed managed \
                     browser falls back to headless on this host.",
                )
                .with_fix_hint(
                    "Expected on headless servers. Set headless=true for managed \
                     profiles, or run under an X11/Wayland session for headed browsing.",
                ),
            );
        }

        findings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn always_reports_chromium_and_node_findings() {
        let check = BrowserRuntimeCheck::new();
        let findings = check.run(Posture::Inspect).await;
        // Chromium + Node probes always emit exactly one finding each; the
        // display finding is conditional, so there are 2 or 3 total.
        assert!(findings.len() == 2 || findings.len() == 3, "got {}", findings.len());
        assert!(findings.iter().all(|f| f.check_id == ID));
    }

    #[tokio::test]
    async fn never_repairs_in_fix_posture() {
        // Installing a browser/Node is a network download, never a mechanical
        // repair — Fix posture must leave every finding non-repairable and
        // produce no repair outcome.
        let check = BrowserRuntimeCheck::new();
        let findings = check.run(Posture::Fix).await;
        assert!(findings.iter().all(|f| !f.repairable));
        assert!(findings.iter().all(|f| f.repair_outcome.is_none()));
    }

    #[tokio::test]
    async fn findings_are_advisory_never_gating() {
        // The browser is an optional subsystem; missing prerequisites must never
        // flip `aleph doctor`'s exit gate (which keys off `Finding::is_problem`).
        // Every finding this check emits is therefore Info-level. This invariant
        // is the non-breaking guarantee — guard it.
        let check = BrowserRuntimeCheck::new();
        let findings = check.run(Posture::Inspect).await;
        assert!(
            findings.iter().all(|f| !f.is_problem()),
            "browser/runtime findings must stay advisory (Info) to avoid a false doctor-gate failure"
        );
    }

    #[test]
    fn has_display_is_true_on_non_linux() {
        // On macOS / Windows the native window server is always present.
        if !cfg!(target_os = "linux") {
            assert!(BrowserRuntimeCheck::has_display());
        }
    }
}
