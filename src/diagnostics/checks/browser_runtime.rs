//! `browser/runtime` — the browser subsystem's external prerequisites.
//!
//! Aleph drives two browser backends and each has a distinct host dependency:
//! the **existing-session** driver attaches to the user's real Chrome via
//! `chrome-devtools-mcp` (needs a system Chromium *and* `npx`/Node), while the
//! **managed** driver runs the ledger-provisioned `playwright-cli` (which
//! brings its own Chromium, and only benefits from a headed display on Linux).
//! When any of these is absent the failure surfaces either deep inside a tool
//! call ("Chromium binary not found", "Failed to attach to browser") or as a
//! `browser_*` family that is not offered at all — both with no up-front
//! signal. This check makes the browser runtime as observable as the core data
//! dir / lock / config — the same ground both `openclaw browser-doctor` and
//! hermes' `_browser_cdp_check` cover.
//!
//! All probes are read-only environment/binary/ledger lookups (no process
//! spawns, no network), so the check is non-repairable: installing a browser,
//! Node or the managed CLI is a network download, not a mechanical fix, so we
//! surface a `fix_hint` instead. The three binary probes run **concurrently**
//! via `tokio::join!` — surpassing the reference doctors' sequential probing.
//!
//! The managed probe deliberately calls the *same* function the tool-gating
//! probe uses (`tools::probes::browser::managed_cli_path`). These two are twins
//! — one hides the `browser_*` tools, the other explains why — and a twin that
//! answers "is the managed driver provisioned?" with its own private lookup is
//! how the family spent four rounds gated on `npx`, a binary the managed driver
//! never runs.

use async_trait::async_trait;

use crate::browser::{find_chromium, BrowserError};
use crate::diagnostics::check::{unknown_finding, HealthCheck, Posture};
use crate::diagnostics::finding::Finding;

const ID: &str = "browser/runtime";

/// Subjects for the "could not determine" findings, so the sentence a probe
/// failure produces is spelled once per probe rather than once per arm.
const SUBJECT_CHROMIUM: &str = "System browser";
const SUBJECT_NODE: &str = "Node runtime";
const SUBJECT_MANAGED: &str = "Managed browser runtime";

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

/// Outcome of the managed-driver probe: whether a `playwright-cli` is already
/// provisioned, so a managed call can run instead of first bootstrapping one.
enum ManagedProbe {
    Found(String),
    Missing,
}

/// Which `find_chromium` outcomes mean "absent" and which mean "I could not
/// look".
///
/// `BrowserError::ChromiumNotFound` is NAMED rather than caught by an `Err(_)`
/// arm: it is the one variant that means absence. `find_chromium` has only that
/// one error path today, so the two spellings agree — but `Err(_)` agrees by
/// luck, and would go on calling a future IO error "not installed" without
/// anybody editing this file.
fn classify_chromium(
    found: Result<std::path::PathBuf, BrowserError>,
) -> Result<ChromiumProbe, String> {
    match found {
        Ok(path) => Ok(ChromiumProbe::Found(path.display().to_string())),
        Err(BrowserError::ChromiumNotFound) => Ok(ChromiumProbe::Missing),
        Err(e) => Err(format!("the Chromium lookup failed: {e}")),
    }
}

/// Which `which` outcomes mean "absent".
///
/// `which` distinguishes "not on PATH" from "there was nowhere to search" and
/// "the hit could not be canonicalised". Only the first is absence; the other
/// two are this round's `try_exists` distinction wearing another crate's error
/// type.
fn classify_npx(found: Result<std::path::PathBuf, which::Error>) -> Result<NodeProbe, String> {
    match found {
        Ok(path) => Ok(NodeProbe::Found(path.display().to_string())),
        Err(which::Error::CannotFindBinaryPath) => Ok(NodeProbe::Missing),
        Err(e) => Err(format!("the `npx` lookup could not be performed: {e}")),
    }
}

/// Fold the two ways a probe can fail to answer into the one finding that says
/// so: the blocking task did not complete (a panic, or the runtime shutting
/// down), or the lookup itself could not be performed.
///
/// Shared by all three probes rather than spelled per probe, because the
/// mistake this replaces was spelled per probe too — `.unwrap_or(X::Missing)`
/// appeared three times, and each one turned a task failure into a reassuring
/// `[ok] … not detected`. One settler means the next probe added here inherits
/// the right answer instead of copying the nearest neighbour.
// The `Err` IS the finding pushed for this probe; see `check::Presence::of`.
#[allow(clippy::result_large_err)]
fn settle<T>(
    subject: &str,
    probe: Result<Result<T, String>, tokio::task::JoinError>,
) -> Result<T, Finding> {
    match probe {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(why)) => Err(unknown_finding(ID, subject, why)),
        Err(e) => Err(unknown_finding(
            ID,
            subject,
            format!("the {subject} probe task did not complete: {e}"),
        )),
    }
}

#[derive(Default)]
pub struct BrowserRuntimeCheck;

impl BrowserRuntimeCheck {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Locate a system Chromium-family browser. Runs the sync discovery on a
    /// blocking thread so it never stalls the diagnostics runtime.
    ///
    /// # Errors
    ///
    /// The ready-made "unknown" finding when the lookup could not be performed
    /// — see [`Self::run`]'s severity note for why that is not `Missing`.
    // The `Err` IS the finding pushed for this probe; see `check::Presence::of`.
    #[allow(clippy::result_large_err)]
    async fn probe_chromium() -> Result<ChromiumProbe, Finding> {
        settle(
            SUBJECT_CHROMIUM,
            tokio::task::spawn_blocking(|| classify_chromium(find_chromium())).await,
        )
    }

    /// Locate `npx` on `PATH` — the existing-session driver launches
    /// `npx chrome-devtools-mcp@latest` (see `ChromeMcpConfig` defaults).
    ///
    /// # Errors
    ///
    /// The ready-made "unknown" finding when the lookup could not be performed.
    // The `Err` IS the finding pushed for this probe; see `check::Presence::of`.
    #[allow(clippy::result_large_err)]
    async fn probe_node() -> Result<NodeProbe, Finding> {
        settle(
            SUBJECT_NODE,
            tokio::task::spawn_blocking(|| classify_npx(which::which("npx"))).await,
        )
    }

    /// Locate the managed driver's `playwright-cli`. Delegates to the
    /// tool-gating probe's resolver so the doctor and the gate can never give
    /// different answers about the same driver.
    ///
    /// # Errors
    ///
    /// The ready-made "unknown" finding when the probe task did not complete.
    /// The resolver itself returns `Option`, so there is no error of its own to
    /// misread here — only the task's.
    // The `Err` IS the finding pushed for this probe; see `check::Presence::of`.
    #[allow(clippy::result_large_err)]
    async fn probe_managed() -> Result<ManagedProbe, Finding> {
        settle(
            SUBJECT_MANAGED,
            tokio::task::spawn_blocking(|| {
                Ok(match crate::tools::probes::browser::managed_cli_path() {
                    Some(path) => ManagedProbe::Found(path.display().to_string()),
                    None => ManagedProbe::Missing,
                })
            })
            .await,
        )
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
        // All three binary probes are independent — run them concurrently.
        let (chromium, node, managed) = tokio::join!(
            Self::probe_chromium(),
            Self::probe_node(),
            Self::probe_managed()
        );

        let mut findings = Vec::with_capacity(4);

        // Every finding about a PREREQUISITE is Info-level (advisory). The
        // browser is an *optional* subsystem and both drivers have legitimate
        // prerequisite-free setups (the managed driver provisions its own
        // Chromium; a managed-only headless server needs neither system Chrome
        // nor Node), so a missing prerequisite must never flip `aleph doctor`'s
        // exit gate. The full `detail` + `fix_hint` still reach the JSON /
        // `doctor`-tool surface.
        //
        // A probe that could not RUN is a different fact and is deliberately
        // `Warning` (`check::unknown_finding`). "This prerequisite is absent,
        // which is fine" and "I could not find out" are not the same sentence,
        // and the second one used to be rendered as the first: a panicked
        // `spawn_blocking` task rendered as a reassuring `[ok] System browser
        // not detected`. That is the class this directory's presence probes
        // were converted to remove, and the argument above — about optional
        // prerequisites — says nothing about it.
        findings.push(match chromium {
            Err(unknown) => unknown,
            Ok(ChromiumProbe::Found(path)) => Finding::ok(
                ID,
                "System browser detected",
                format!("Chromium-family binary at {path}."),
            ),
            Ok(ChromiumProbe::Missing) => Finding::ok(
                ID,
                "System browser not detected (managed driver unaffected)",
                "No Chrome/Chromium/Brave/Edge on PATH or in the well-known install \
                 locations. The existing-session driver (attach to your own Chrome) \
                 needs one; the managed driver brings its own Chromium and works \
                 without it — see the managed-runtime finding for whether that one \
                 is provisioned.",
            )
            .with_fix_hint(
                "Install Chrome/Chromium, or set ALEPH_CHROME_PATH, to enable the \
                 existing-session (attach) driver. Managed-only setups can ignore this.",
            ),
        });

        findings.push(match node {
            Err(unknown) => unknown,
            Ok(NodeProbe::Found(path)) => Finding::ok(
                ID,
                "Node runtime detected",
                format!("npx at {path} (used by the existing-session chrome-devtools-mcp driver)."),
            ),
            Ok(NodeProbe::Missing) => Finding::ok(
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

        findings.push(match managed {
            Err(unknown) => unknown,
            Ok(ManagedProbe::Found(path)) => Finding::ok(
                ID,
                "Managed browser runtime provisioned",
                format!("playwright-cli at {path} (used by the managed driver)."),
            ),
            Ok(ManagedProbe::Missing) => Finding::ok(
                ID,
                "Managed browser runtime not provisioned",
                "No `playwright-cli` on PATH or marked Ready in the capability ledger \
                 (~/.aleph/runtimes/ledger.json). Browsing is available only through \
                 the existing-session driver until one is provisioned — and with \
                 neither driver runnable the `browser_*` tools are withheld from the \
                 model entirely (`tools::probes::browser`), so the bootstrap will not \
                 be triggered by a tool call.",
            )
            .with_fix_hint(
                "Install the managed runtime from the Panel's Runtimes page (Browser \
                 settings links to it), or install Chrome + Node to use the \
                 existing-session driver.",
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
    use crate::diagnostics::finding::Severity;

    #[tokio::test]
    async fn always_reports_chromium_node_and_managed_findings() {
        let check = BrowserRuntimeCheck::new();
        let findings = check.run(Posture::Inspect).await;
        // Chromium + Node + managed probes always emit exactly one finding
        // each; the display finding is conditional, so there are 3 or 4 total.
        assert!(
            findings.len() == 3 || findings.len() == 4,
            "got {}",
            findings.len()
        );
        assert!(findings.iter().all(|f| f.check_id == ID));
    }

    /// The twin invariant: the doctor's managed finding and the tool gate must
    /// be reading the same fact. A doctor that says "provisioned" while the
    /// gate hides the tools (or the reverse) is worse than no finding — it
    /// sends the user looking for a browser problem that is really a gating
    /// problem.
    #[tokio::test]
    async fn the_managed_finding_agrees_with_the_tool_gate() {
        let provisioned = crate::tools::probes::browser::managed_cli_path().is_some();
        let findings = BrowserRuntimeCheck::new().run(Posture::Inspect).await;
        let reported = findings
            .iter()
            .any(|f| f.title == "Managed browser runtime provisioned");
        assert_eq!(reported, provisioned);
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
    async fn prerequisite_findings_are_advisory_never_gating() {
        // The browser is an optional subsystem; a missing PREREQUISITE must
        // never flip `aleph doctor`'s exit gate (which keys off
        // `Finding::is_problem`), so every finding about one is Info-level.
        //
        // That is not the same as "every finding this check can emit". A probe
        // that could not RUN is deliberately `Warning` — see
        // [`a_probe_that_could_not_run_is_not_a_missing_prerequisite`]. This
        // test still asserts the whole battery because on any machine that can
        // run it, all three probes complete: `find_chromium` has one error
        // path (`ChromiumNotFound`), `which` returns `CannotFindBinaryPath`
        // when `npx` is absent, and the managed resolver returns `Option`. So a
        // `Warning` here means a probe task actually failed, which is a real
        // defect and a true red, not a false gate.
        let check = BrowserRuntimeCheck::new();
        let findings = check.run(Posture::Inspect).await;
        assert!(
            findings.iter().all(|f| !f.is_problem()),
            "browser/runtime prerequisite findings must stay advisory (Info) to avoid a \
             false doctor-gate failure; a problem here means a probe task did not complete"
        );
    }

    /// The defect this file's three-way probes replaced: `.unwrap_or(X::Missing)`
    /// on the `spawn_blocking` `JoinError` meant a probe task that PANICKED
    /// rendered as a reassuring `[ok] … not detected` — "no browser installed",
    /// stated confidently, by a check that had not looked.
    ///
    /// Uses a real `JoinError` from a real panicked task rather than a
    /// hand-built one, so it exercises the arm production takes.
    #[tokio::test]
    async fn a_probe_that_could_not_run_is_not_a_missing_prerequisite() {
        let joined: Result<Result<NodeProbe, String>, tokio::task::JoinError> =
            tokio::task::spawn_blocking(|| panic!("probe blew up")).await;
        assert!(joined.is_err(), "precondition: the task must have failed");

        let finding = settle(SUBJECT_NODE, joined)
            .err()
            .expect("a task that did not complete must not be settled into a probe outcome");
        assert_eq!(finding.severity, Severity::Warning);
        assert_eq!(finding.title, "Node runtime unknown");
        assert!(finding.is_problem(), "an unknown must never render as [ok]");
    }

    /// Only the not-found verdict is absence. An `Err(_)` arm would agree with
    /// this today by luck — `find_chromium` has one error path — and would go on
    /// agreeing after it grew a second one.
    #[test]
    fn only_chromium_not_found_means_the_browser_is_absent() {
        assert!(matches!(
            classify_chromium(Err(BrowserError::ChromiumNotFound)),
            Ok(ChromiumProbe::Missing)
        ));
        assert!(
            classify_chromium(Err(BrowserError::LaunchFailed("io".into()))).is_err(),
            "a lookup that could not be performed is not the same as absence"
        );
        assert!(matches!(
            classify_chromium(Ok(std::path::PathBuf::from("/x/chrome"))),
            Ok(ChromiumProbe::Found(_))
        ));
    }

    /// `which` has three error variants and only one of them is "not on PATH".
    #[test]
    fn only_cannot_find_binary_path_means_npx_is_absent() {
        assert!(matches!(
            classify_npx(Err(which::Error::CannotFindBinaryPath)),
            Ok(NodeProbe::Missing)
        ));
        for e in [
            which::Error::CannotGetCurrentDirAndPathListEmpty,
            which::Error::CannotCanonicalize,
        ] {
            assert!(
                classify_npx(Err(e)).is_err(),
                "a PATH lookup that could not be performed is not the same as absence"
            );
        }
        assert!(matches!(
            classify_npx(Ok(std::path::PathBuf::from("/x/npx"))),
            Ok(NodeProbe::Found(_))
        ));
    }

    #[test]
    fn has_display_is_true_on_non_linux() {
        // On macOS / Windows the native window server is always present.
        if !cfg!(target_os = "linux") {
            assert!(BrowserRuntimeCheck::has_display());
        }
    }
}
