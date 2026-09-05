//! `browser/chromium-missing` — can the MANAGED driver launch a browser?
//!
//! Distinct from `browser/runtime`, which asks three prerequisite questions
//! (system browser / Node for the existing-session driver / is `playwright-cli`
//! provisioned) and answers them from read-only lookups. This one asks the
//! single question the launch-chain flip created — *is there a Chromium for
//! Aleph to spawn* — and it answers it by running **the same resolver the
//! launch path runs** (`browser::chromium_resolve::resolve_binary`). A doctor
//! that re-derived the search order would be a second answer to a question the
//! driver already answers, and the two would disagree exactly when it matters
//! (判据 §1, §9).
//!
//! That means this check DOES spawn a process (`playwright-cli install-browser
//! chromium --dry-run`), unlike its sibling. It is bounded by
//! [`RESOLVE_TIMEOUT`], and a probe that does not answer in time produces
//! [`crate::diagnostics::check::unknown_finding`] — the house style for "this
//! check could not determine its own subject" (`src/diagnostics/check.rs:205-225`:
//! `Severity::Warning`, titled `"<subject> unknown"`, spelled once so unknown
//! keeps meaning the same severity everywhere). Never "not installed": unknown
//! is neither healthy nor failed (判据 §8).

use async_trait::async_trait;

use crate::browser::chromium_resolve::{resolve_binary, ChromiumSource, ResolvedChromium};
use crate::browser::profile::BrowserType;
use crate::browser::BrowserError;
use crate::diagnostics::check::{settle_probe, unknown_finding, HealthCheck, Posture};
use crate::diagnostics::finding::Finding;

const ID: &str = "browser/chromium-missing";
const SUBJECT: &str = "Managed browser";

/// The outer bound on the whole resolution, and the number is chosen by two
/// constraints, not by taste.
///
/// **Below** it: `chromium_resolve::DRY_RUN_TIMEOUT` is 6 s, the only thing in
/// the resolution that can block. **Above** it: `check::DEFAULT_CHECK_TIMEOUT`
/// is 20 s (`src/diagnostics/check.rs:27`), and past that the ENGINE abandons
/// the check and emits a `Warning` of its own. A check whose inner deadline
/// sits at or above the engine's is a 恒假 arm (判据 §2) plus an amber
/// `doctor` on every slow probe — and `src/diagnostics/checks/mod.rs:6-10`
/// names exactly that as the way this command's exit code becomes a constant.
/// Three budgets, strictly nested: 6 < 8 < 20, so this check always gets to
/// answer for itself and never needs a `HealthCheck::timeout()` override.
const RESOLVE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// The finding for "there is no browser", spelled once so the doctor, the tool
/// error and the QA fixture can all be checked against the same sentence.
fn missing_finding(tried: impl std::fmt::Display) -> Finding {
    Finding::ok(
        ID,
        "No Chromium for the managed browser driver",
        format!(
            "The managed driver launches Chromium itself and could not find one ({tried}). \
             Browser tools will refuse until this is fixed; the existing-session driver \
             (attach to your own Chrome) is unaffected."
        ),
    )
    .with_fix_hint(
        "Run `playwright-cli install-browser chromium`, ask Aleph to run \
         `runtime_manage{action:\"install\", capability:\"chromium\"}`, or pin an \
         installed browser with [browser.runtime] binary_path. On a network that \
         blocks Playwright's CDN, set [browser.runtime] download_host to a mirror first.",
    )
}

/// The finding for "there is one", naming which of the three routes answered.
fn found_finding(path: &std::path::Path, source: ChromiumSource) -> Finding {
    Finding::ok(
        ID,
        "Managed browser available",
        format!("{} — {}.", path.display(), source.label()),
    )
}

/// The fix-hint sentence, reachable from `builtin_tools::runtime_manage`'s test
/// so the tool it names can be pinned to a tool that exists. Exposing the
/// finding rather than the string keeps one author for the sentence.
// TODO(plan-1 task 8): remove this allow. Task 8 (builtin_tools::runtime_manage)
// is the only consumer; until then this fn has no non-test caller and
// `cargo clippy --all-targets` (rust-doctor.yml:129) sees it under `--tests`,
// which `cargo clippy -p alephcore --lib` (CI:345) does not compile.
#[allow(dead_code)]
#[cfg(test)]
pub(crate) fn missing_finding_for_test() -> Finding {
    missing_finding("no system browser")
}

/// Maps a completed resolution onto the doctor's three-way answer — the ONLY
/// decision this check makes. Pure and synchronous on purpose: `run()`'s
/// async body is just wiring (probe the CLI, load the config, call the real
/// resolver under a timeout) around this one match, so the decision itself
/// can be exercised with a hand-built [`ResolvedChromium`] / [`BrowserError`]
/// instead of needing a real Chromium, a real `playwright-cli`, or a
/// particular machine's install state to reach every arm in a test.
fn classify_resolution(probe: Result<ResolvedChromium, BrowserError>) -> Finding {
    match probe {
        Ok(r) => found_finding(&r.path, r.source),
        Err(BrowserError::ChromiumUnavailable { tried }) => missing_finding(tried),
        // Any other error is the resolver failing to look, not a verdict.
        Err(e) => unknown_finding(ID, SUBJECT, format!("the lookup failed: {e}")),
    }
}

#[derive(Default)]
pub struct ChromiumMissingCheck;

impl ChromiumMissingCheck {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl HealthCheck for ChromiumMissingCheck {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> &'static str {
        "Managed browser (Chromium)"
    }

    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        // The CLI is the resolver's third route AND the thing that would run
        // the install. Without it there is nothing to ask and nothing to fix
        // here — `browser/runtime`'s managed probe owns that sentence, so this
        // check defers to it rather than printing a second copy.
        // Off the async worker, mirroring the twin probe at
        // `browser_runtime.rs:230-236`, which wraps the identical call for the
        // identical reason: it does a `which` PATH walk plus a JSON file read
        // (判据 §16 — fix it on both sides). The `JoinError` → `Finding` mapping
        // is `check::settle_probe`'s job, not a second copy of it (M1 in the
        // Task 7 review): a panicked probe must produce the same "<subject>
        // unknown" sentence every check in this directory produces, with one
        // author.
        let cli = match settle_probe(
            ID,
            SUBJECT,
            tokio::task::spawn_blocking(crate::tools::probes::browser::managed_cli_path).await,
        ) {
            Ok(v) => v,
            Err(finding) => return vec![finding],
        };
        let Some(cli) = cli else {
            return vec![Finding::ok(
                ID,
                "Managed browser not checked (no playwright-cli)",
                "The managed driver's CLI is not provisioned, so there is nothing to \
                 attach a browser to yet. See the `browser/runtime` finding for that.",
            )];
        };
        let runtime = match crate::config::Config::load() {
            Ok(cfg) => cfg.general.browser.runtime.clone(),
            // A config we cannot read is not a config with default settings: a
            // pinned binary_path we failed to see would make every answer below
            // wrong. Say "I could not look".
            Err(e) => {
                return vec![unknown_finding(
                    ID,
                    SUBJECT,
                    format!("the config could not be read, so the browser pin is unknown: {e}"),
                )]
            }
        };
        let probe = tokio::time::timeout(
            RESOLVE_TIMEOUT,
            resolve_binary(&runtime, &BrowserType::default(), &cli),
        )
        .await;
        vec![match probe {
            Ok(resolution) => classify_resolution(resolution),
            // The check's OWN "could not verify" answer, which is why
            // RESOLVE_TIMEOUT sits under the engine's ceiling: if the engine
            // got here first, this arm would be unreachable and the operator
            // would read the engine's abandonment Warning instead of a sentence
            // naming what was being probed.
            Err(e) => unknown_finding(
                ID,
                SUBJECT,
                format!(
                    "the chromium lookup did not answer within {}s (engine ceiling is {}s): {e}",
                    RESOLVE_TIMEOUT.as_secs(),
                    crate::diagnostics::check::DEFAULT_CHECK_TIMEOUT.as_secs()
                ),
            ),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fail-closed contract: when there is no browser, the finding must name
    /// the command that fixes it. A gate that closes without saying how to open
    /// it is fail-dead (判据 §14), and this is the one surface an operator
    /// reaches for when the managed driver answers "no Chromium".
    #[test]
    fn the_missing_finding_names_every_way_out() {
        let f = missing_finding("no system browser; playwright's chromium is not installed");
        assert_eq!(f.check_id, ID);
        let text = format!("{} {}", f.detail, f.fix_hint.clone().unwrap_or_default());
        assert!(
            text.contains("playwright-cli install-browser chromium"),
            "{text}"
        );
        assert!(text.contains("runtime_manage"), "{text}");
        assert!(text.contains("binary_path"), "{text}");
        // Info, not Error: the browser subsystem is optional and a
        // managed-browser-less host must not turn `aleph-server doctor`'s exit
        // code into a constant. Same argument `browser/runtime` states.
        assert_eq!(f.severity, crate::diagnostics::finding::Severity::Info);
    }

    /// A found browser says WHICH of the three routes answered. "Chromium is
    /// available" without the source is the finding an operator cannot act on:
    /// pinning, installing and the system browser are three different fixes.
    #[test]
    fn the_ok_finding_names_the_source_and_the_path() {
        let f = found_finding(
            std::path::Path::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
            crate::browser::chromium_resolve::ChromiumSource::System,
        );
        assert_eq!(f.check_id, ID);
        assert!(f.detail.contains("Google Chrome"), "{}", f.detail);
        assert!(
            f.detail.contains("system Chromium-family browser"),
            "{}",
            f.detail
        );
    }

    /// The arm mapping `run()` delegates to `classify_resolution` — the only
    /// production logic this check owns, and (before this test) the only
    /// piece of it nothing ever executed. Swapping the `Ok(r) =>` and
    /// `Err(ChromiumUnavailable{..}) =>` arms in `classify_resolution` still
    /// compiles and every other test in this file still passes; this is the
    /// one that must go red for it (verified by hand while writing this test:
    /// swapping the two arms turned this test red with the found/missing
    /// titles exchanged, then reverted).
    #[test]
    fn classify_resolution_maps_found_to_the_ok_finding_and_unavailable_to_the_gap_finding() {
        let found = classify_resolution(Ok(ResolvedChromium {
            path: std::path::PathBuf::from("/opt/chromium/chrome"),
            source: ChromiumSource::System,
            engine: None,
        }));
        assert_eq!(found.check_id, ID);
        assert_eq!(found.title, "Managed browser available");
        assert_eq!(found.severity, crate::diagnostics::finding::Severity::Info);

        let missing = classify_resolution(Err(BrowserError::ChromiumUnavailable {
            tried: "pin: none; system: none; playwright: not installed".into(),
        }));
        assert_eq!(missing.check_id, ID);
        assert_eq!(missing.title, "No Chromium for the managed browser driver");
        assert_eq!(
            missing.severity,
            crate::diagnostics::finding::Severity::Info
        );
    }

    /// Only `ChromiumUnavailable` is the resolver's considered "I looked
    /// everywhere and there is nothing" answer. Any other `BrowserError` means
    /// the resolver failed to look (a launch-stage error, a timeout inside the
    /// dry-run, …) and must render as `unknown`, never as `missing` — the same
    /// fail-closed reading `browser/runtime`'s probes use for a `JoinError`.
    #[test]
    fn classify_resolution_reports_any_other_error_as_unknown_not_a_verdict() {
        let f = classify_resolution(Err(BrowserError::ChromiumNotFound));
        assert_eq!(f.check_id, ID);
        assert_eq!(f.severity, crate::diagnostics::finding::Severity::Warning);
        assert!(f.title.ends_with("unknown"), "{}", f.title);
    }

    /// The defect `browser/runtime`'s twin probes were converted to remove: a
    /// probe task that never came back rendered as a confident "not present".
    /// Uses a real `JoinError` from a real panicked task, exercising the exact
    /// `settle_probe` call `run()` makes, rather than a hand-built error.
    #[tokio::test]
    async fn a_probe_that_could_not_run_is_never_reported_as_missing() {
        let joined: Result<Option<std::path::PathBuf>, tokio::task::JoinError> =
            tokio::task::spawn_blocking(|| panic!("probe blew up")).await;
        assert!(joined.is_err(), "precondition: the task must have failed");

        let finding = settle_probe(ID, SUBJECT, joined)
            .expect_err("a task that did not complete must not be settled into a probe outcome");
        assert_eq!(finding.check_id, ID);
        assert_eq!(
            finding.severity,
            crate::diagnostics::finding::Severity::Warning
        );
        assert!(finding.title.ends_with("unknown"), "{}", finding.title);
        assert_ne!(finding.title, "No Chromium for the managed browser driver");
    }

    /// The three budgets that must stay nested, asserted rather than described.
    /// If any one of them moves, this test names which invariant broke instead
    /// of leaving an unreachable arm and an amber doctor to be discovered.
    #[test]
    fn the_check_answers_before_the_engine_abandons_it() {
        assert!(
            crate::browser::chromium_resolve::DRY_RUN_TIMEOUT < RESOLVE_TIMEOUT,
            "the inner probe must finish before this check's own deadline"
        );
        assert!(
            RESOLVE_TIMEOUT < crate::diagnostics::check::DEFAULT_CHECK_TIMEOUT,
            "this check must answer before the engine abandons it and emits its \
             own Warning — otherwise the timeout arm here is unreachable"
        );
    }
}
