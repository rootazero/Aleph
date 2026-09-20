//! `browser/<engine>-missing` — can this engine's driver actually start one?
//!
//! One implementation, one id per engine, because the question has the same
//! shape for both and a second file would be a second answer to it (判据 §16
//! — the twin gets the fix carried over, not rediscovered).
//!
//! Distinct from `browser/runtime`, which asks three prerequisite questions
//! (system browser / Node for the existing-session driver / is `playwright-cli`
//! provisioned) and answers them from read-only lookups.
//!
//! **Chromium** answers by running the same resolver the launch path runs
//! (`browser::chromium_resolve::resolve_binary`), which is why this check
//! spawns a process (`playwright-cli install-browser chromium --dry-run`). A
//! doctor that re-derived the search order would be a second answer to a
//! question the driver already answers, and the two would disagree exactly
//! when it matters (判据 §1, §9).
//! **obscura** answers from a config pin plus `runtimes::probe` — a `which`
//! PATH walk over the enriched search path and a `--version` call. No network,
//! no download.
//!
//! Both are bounded by [`RESOLVE_TIMEOUT`], and a probe that does not answer
//! in time produces [`crate::diagnostics::check::unknown_finding`]
//! (`src/diagnostics/check.rs:214`) — the house style for "this check could
//! not determine its own subject" (`Severity::Warning`, titled `"<subject>
//! unknown"`, spelled once so unknown keeps meaning the same severity
//! everywhere). Never "not installed": unknown is neither healthy nor failed
//! (判据 §8). `settle_probe` is at `:249`; the single `205-225` range the old
//! module doc gave covered neither.

use std::path::PathBuf;

use async_trait::async_trait;

// `ChromiumSource` is NOT imported here: after `found_finding` started taking a
// `&str`, the only remaining mention is in `mod tests`, and a top-level import
// would be an `unused_imports` warning this task's own clippy step surfaces.
use crate::browser::chromium_resolve::{resolve_binary, ResolvedChromium};
use crate::browser::engine::Engine;
use crate::browser::profile::BrowserType;
use crate::browser::BrowserError;
use crate::diagnostics::check::{settle_probe, unknown_finding, HealthCheck, Posture};
use crate::diagnostics::finding::Finding;
// No `TargetOs` here, deliberately: the platform question is
// `specs::supported_on_current_os`'s, and importing the enum would be the
// first half of composing a second answer to it. (`specs::TargetOs` would not
// resolve anyway — `specs.rs` imports it privately, and only its own
// descendant `mod tests` inherits that binding.)
use crate::runtimes::{specs, OBSCURA_RUNTIME, OBSCURA_TAG};

const CHROMIUM_ID: &str = "browser/chromium-missing";
const OBSCURA_ID: &str = "browser/obscura-missing";

/// The check id for an engine. A `match` rather than
/// `format!("browser/{}-missing", …)` so a new `Engine` variant is a compile
/// error here instead of a silently coined id that nothing registers and
/// `doctor --only` cannot name.
const fn id_for(engine: Engine) -> &'static str {
    match engine {
        Engine::Chromium => CHROMIUM_ID,
        Engine::Obscura => OBSCURA_ID,
    }
}

const fn subject_for(engine: Engine) -> &'static str {
    match engine {
        Engine::Chromium => "Managed browser",
        Engine::Obscura => "Obscura engine",
    }
}

const fn title_for(engine: Engine) -> &'static str {
    match engine {
        Engine::Chromium => "Managed browser (Chromium)",
        Engine::Obscura => "Browser engine (obscura)",
    }
}

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

/// "There is no engine", spelled once per engine so the doctor, the tool error
/// and the QA fixture all check the same sentence.
///
/// `detail` is a sentence about WHAT WAS LOOKED FOR, wrapped in this
/// function's own sentence — not a fix hint. The two are kept apart
/// deliberately: `BrowserError::EngineUnavailable` carries its own remedy in
/// `install_hint`, and splicing that into this slot produced "…could not find
/// one (run playwright-cli …)." followed by a second, different
/// `.with_fix_hint`. One finding, two answers to "how do I fix this"
/// (判据 §1). The fix hint has one author: this function.
fn missing_finding(engine: Engine, detail: impl std::fmt::Display) -> Finding {
    match engine {
        Engine::Chromium => Finding::ok(
            CHROMIUM_ID,
            "No Chromium for the managed browser driver",
            format!(
                "The managed driver launches Chromium itself and could not find one ({detail}). \
                 Browser tools will refuse ON MANAGED PROFILES until this is fixed; the default \
                 profile (driver = cdp, engine = obscura) and the existing-session driver \
                 (attach to your own Chrome) are unaffected."
            ),
        )
        .with_fix_hint(
            "Run `playwright-cli install-browser chromium`, ask Aleph to run \
             `runtime_manage{action:\"install\", capability:\"chromium\"}`, or pin an \
             installed browser with [general.browser.runtime] binary_path. On a network that \
             blocks Playwright's CDN, set [general.browser.runtime] download_host to a mirror first.",
        ),
        Engine::Obscura => Finding::ok(
            OBSCURA_ID,
            "No obscura binary for the default browser engine",
            format!(
                "obscura is the default browser engine and there is none installed ({detail}). \
                 Browser tools will refuse on obscura profiles until this is fixed; profiles \
                 configured with engine = \"chromium\" are unaffected."
            ),
        )
        .with_fix_hint(
            "Ask Aleph to run `runtime_manage{action:\"install\", capability:\"obscura\"}` \
             (a ~90 MB download, so it runs as a background job you poll with \
             `bash{process_action:\"wait\", process_id:<id>}`), or pin an already-extracted \
             binary with [general.browser.obscura] binary_path. On a network that blocks \
             GitHub release assets, set [general.browser.obscura] download_host to a mirror first.",
        ),
    }
}

/// "There is one", naming where it came from. Three different sources mean
/// three different fixes, so a bare "available" is a finding nobody can act on.
fn found_finding(engine: Engine, path: &std::path::Path, source: &str) -> Finding {
    Finding::ok(
        id_for(engine),
        match engine {
            Engine::Chromium => "Managed browser available",
            Engine::Obscura => "Obscura engine available",
        },
        format!("{} — {}.", path.display(), source),
    )
}

/// The platform the upstream release matrix does not build.
///
/// Neither "missing" (which reads as *install it*) nor "available". A third
/// answer, because the operator's only action is to configure the other engine
/// — and offering `runtime_manage{install}` here would be a door onto a wall
/// (判据 §14).
fn unavailable_platform_finding(engine: Engine, os: &str, arch: &str) -> Finding {
    Finding::ok(
        id_for(engine),
        "Obscura is not published for this platform",
        format!(
            "There is no {} release asset for {arch}-{os}, so this host can only run the \
             chromium engine. Set [general.browser] default_engine = \"chromium\" (or \
             engine = \"chromium\" on the profiles you use) to stop browser tools refusing.",
            engine.as_str()
        ),
    )
}

/// The fix-hint sentence, reachable from `builtin_tools::runtime_manage`'s test
/// so the tool it names can be pinned to a tool that exists. Exposing the
/// finding rather than the string keeps one author for the sentence.
#[cfg(test)]
pub(crate) fn missing_finding_for_test() -> Finding {
    missing_finding(Engine::Chromium, "no system browser")
}

/// Maps a completed resolution onto the doctor's three-way answer — the ONLY
/// decision the Chromium arm makes. Pure and synchronous on purpose:
/// `run_chromium`'s async body is just wiring (probe the CLI, load the config,
/// call the real resolver under a timeout) around this one match, so the
/// decision itself can be exercised with a hand-built [`ResolvedChromium`] /
/// [`BrowserError`] instead of needing a real Chromium, a real
/// `playwright-cli`, or a particular machine's install state to reach every
/// arm in a test.
fn classify_resolution(probe: Result<ResolvedChromium, BrowserError>) -> Finding {
    match probe {
        Ok(r) => found_finding(Engine::Chromium, &r.path, r.source.label()),
        Err(BrowserError::EngineUnavailable { engine, tried, .. }) => {
            debug_assert_eq!(engine, Engine::Chromium);
            // `tried`, NOT `install_hint`: `missing_finding` wraps its argument
            // in its own sentence and appends its own fix hint, so passing the
            // built hint would nest a whole remedy inside a parenthetical and
            // state it twice from two authors.
            missing_finding(Engine::Chromium, tried)
        }
        // Any other error is the resolver failing to look, not a verdict.
        Err(e) => unknown_finding(
            CHROMIUM_ID,
            subject_for(Engine::Chromium),
            format!("the lookup failed: {e}"),
        ),
    }
}

pub struct EngineMissingCheck {
    engine: Engine,
}

impl EngineMissingCheck {
    #[must_use]
    pub const fn for_engine(engine: Engine) -> Self {
        Self { engine }
    }
}

#[async_trait]
impl HealthCheck for EngineMissingCheck {
    fn id(&self) -> &'static str {
        id_for(self.engine)
    }

    fn title(&self) -> &'static str {
        title_for(self.engine)
    }

    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        match self.engine {
            Engine::Chromium => Self::run_chromium().await,
            Engine::Obscura => Self::run_obscura().await,
        }
    }
}

impl EngineMissingCheck {
    /// Byte-for-byte the previous `ChromiumMissingCheck::run`.
    async fn run_chromium() -> Vec<Finding> {
        // The CLI is the resolver's third route AND the thing that would run
        // the install. Without it there is nothing to ask and nothing to fix
        // here — `browser/runtime`'s managed probe owns that sentence, so this
        // check defers to it rather than printing a second copy.
        // Off the async worker, mirroring the twin probe at
        // `browser_runtime.rs`, which wraps the identical call for the
        // identical reason: it does a `which` PATH walk plus a JSON file read
        // (判据 §16 — fix it on both sides). The `JoinError` → `Finding` mapping
        // is `check::settle_probe`'s job, not a second copy of it: a panicked
        // probe must produce the same "<subject> unknown" sentence every check
        // in this directory produces, with one author.
        let cli = match settle_probe(
            CHROMIUM_ID,
            subject_for(Engine::Chromium),
            tokio::task::spawn_blocking(crate::tools::probes::browser::managed_cli_path).await,
        ) {
            Ok(v) => v,
            Err(finding) => return vec![finding],
        };
        let Some(cli) = cli else {
            return vec![Finding::ok(
                CHROMIUM_ID,
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
                    CHROMIUM_ID,
                    subject_for(Engine::Chromium),
                    format!("the config could not be read, so the browser pin is unknown: {e}"),
                )]
            }
        };
        let probe = tokio::time::timeout(
            RESOLVE_TIMEOUT,
            resolve_binary(&runtime, &BrowserType::default(), Some(&cli)),
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
                CHROMIUM_ID,
                subject_for(Engine::Chromium),
                format!(
                    "the chromium lookup did not answer within {}s (engine ceiling is {}s): {e}",
                    RESOLVE_TIMEOUT.as_secs(),
                    crate::diagnostics::check::DEFAULT_CHECK_TIMEOUT.as_secs()
                ),
            ),
        }]
    }

    async fn run_obscura() -> Vec<Finding> {
        // The platform question first: on aarch64-windows there is nothing to
        // look for and no install to offer.
        // Two different facts, and they must not collapse into each other:
        // "the ledger has no obscura spec" is this check losing its subject
        // (unknown), while "the spec exists and this platform has no asset" is
        // a verdict the operator can act on.
        if specs::find_spec(OBSCURA_RUNTIME).is_none() {
            return vec![unknown_finding(
                OBSCURA_ID,
                subject_for(Engine::Obscura),
                "there is no obscura runtime spec, so this check has no subject".to_string(),
            )];
        }
        // The platform question goes through the ONE derivation, which
        // `runtime_manage`'s `obscura_row` also reads. Composing
        // `TargetOs::current` + `select_install` + `strategy_supported_here`
        // here would be a second author for the same predicate (判据 §1).
        if !specs::supported_on_current_os(OBSCURA_RUNTIME) {
            return vec![unavailable_platform_finding(
                Engine::Obscura,
                std::env::consts::OS,
                std::env::consts::ARCH,
            )];
        }

        // A pin beats everything, and a pin that does not exist is a stated
        // failure rather than a silent fall-through to the ledger copy —
        // launching a different binary than the one named is worse than
        // refusing, which is the rule `ObscuraRuntimeConfig::binary_path`
        // already states.
        let pinned = match crate::config::Config::load() {
            Ok(cfg) => cfg
                .general
                .browser
                .obscura
                .pinned_binary()
                .map(PathBuf::from),
            Err(e) => {
                return vec![unknown_finding(
                    OBSCURA_ID,
                    subject_for(Engine::Obscura),
                    format!("the config could not be read, so the obscura pin is unknown: {e}"),
                )]
            }
        };
        if let Some(path) = pinned {
            return vec![if path.is_file() {
                found_finding(
                    Engine::Obscura,
                    &path,
                    "pinned by [general.browser.obscura] binary_path",
                )
            } else {
                missing_finding(
                    Engine::Obscura,
                    format!(
                        "[general.browser.obscura] binary_path names {}, which is not a file",
                        path.display()
                    ),
                )
            }];
        }

        // No pin: ask the prober, off the async worker (a `which` PATH walk
        // plus a `--version` subprocess), under the same deadline.
        let probe = tokio::time::timeout(
            RESOLVE_TIMEOUT,
            tokio::task::spawn_blocking(|| crate::runtimes::probe::probe(OBSCURA_RUNTIME)),
        )
        .await;
        let joined = match probe {
            Ok(j) => j,
            Err(e) => {
                return vec![unknown_finding(
                    OBSCURA_ID,
                    subject_for(Engine::Obscura),
                    format!(
                        "the obscura lookup did not answer within {}s (engine ceiling is {}s): {e}",
                        RESOLVE_TIMEOUT.as_secs(),
                        crate::diagnostics::check::DEFAULT_CHECK_TIMEOUT.as_secs()
                    ),
                )]
            }
        };
        let result = match settle_probe(OBSCURA_ID, subject_for(Engine::Obscura), joined) {
            Ok(v) => v,
            Err(finding) => return vec![finding],
        };
        vec![match (result.found, result.bin_path) {
            (true, Some(path)) => {
                let mut f = found_finding(
                    Engine::Obscura,
                    &path,
                    &format!(
                        "ledger {OBSCURA_TAG}, version {}",
                        result.version.as_deref().unwrap_or("unknown")
                    ),
                );
                if let Some(w) = result.version_warning {
                    f = f.with_fix_hint(format!(
                        "{w} — `runtime_manage{{action:\"install\", capability:\"obscura\"}}` \
                         reinstalls the pinned {OBSCURA_TAG} release."
                    ));
                }
                f
            }
            _ => missing_finding(
                Engine::Obscura,
                format!("no `obscura` on PATH and none under the ledger's {OBSCURA_TAG} directory"),
            ),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::chromium_resolve::ChromiumSource;

    /// The fail-closed contract: when there is no browser, the finding must name
    /// the command that fixes it. A gate that closes without saying how to open
    /// it is fail-dead (判据 §14), and this is the one surface an operator
    /// reaches for when the managed driver answers "no Chromium".
    #[test]
    fn the_missing_finding_names_every_way_out() {
        let f = missing_finding(
            Engine::Chromium,
            "no system browser; playwright's chromium is not installed",
        );
        assert_eq!(f.check_id, CHROMIUM_ID);
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
            Engine::Chromium,
            std::path::Path::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
            ChromiumSource::System.label(),
        );
        assert_eq!(f.check_id, CHROMIUM_ID);
        assert!(f.detail.contains("Google Chrome"), "{}", f.detail);
        assert!(
            f.detail.contains("system Chromium-family browser"),
            "{}",
            f.detail
        );
    }

    /// The arm mapping `run_chromium` delegates to `classify_resolution` — the
    /// only production logic that arm owns, and (before this test) the only
    /// piece of it nothing ever executed. Swapping the `Ok(r) =>` and
    /// `Err(EngineUnavailable{..}) =>` arms in `classify_resolution` still
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
        assert_eq!(found.check_id, CHROMIUM_ID);
        assert_eq!(found.title, "Managed browser available");
        assert_eq!(found.severity, crate::diagnostics::finding::Severity::Info);

        let missing = classify_resolution(Err(crate::browser::error::engine_unavailable(
            Engine::Chromium,
            "pin: none; system: none; playwright: not installed",
        )));
        assert_eq!(missing.check_id, CHROMIUM_ID);
        assert_eq!(missing.title, "No Chromium for the managed browser driver");
        assert_eq!(
            missing.severity,
            crate::diagnostics::finding::Severity::Info
        );
    }

    /// Only `EngineUnavailable` is the resolver's considered "I looked
    /// everywhere and there is nothing" answer. Any other `BrowserError` means
    /// the resolver failed to look (a launch-stage error, a timeout inside the
    /// dry-run, …) and must render as `unknown`, never as `missing` — the same
    /// fail-closed reading `browser/runtime`'s probes use for a `JoinError`.
    #[test]
    fn classify_resolution_reports_any_other_error_as_unknown_not_a_verdict() {
        let f = classify_resolution(Err(BrowserError::ChromiumNotFound));
        assert_eq!(f.check_id, CHROMIUM_ID);
        assert_eq!(f.severity, crate::diagnostics::finding::Severity::Warning);
        assert!(f.title.ends_with("unknown"), "{}", f.title);
    }

    /// The defect `browser/runtime`'s twin probes were converted to remove: a
    /// probe task that never came back rendered as a confident "not present".
    /// Uses a real `JoinError` from a real panicked task, exercising the exact
    /// `settle_probe` call `run_chromium` makes, rather than a hand-built error.
    #[tokio::test]
    async fn a_probe_that_could_not_run_is_never_reported_as_missing() {
        let joined: Result<Option<std::path::PathBuf>, tokio::task::JoinError> =
            tokio::task::spawn_blocking(|| panic!("probe blew up")).await;
        assert!(joined.is_err(), "precondition: the task must have failed");

        let finding = settle_probe(CHROMIUM_ID, subject_for(Engine::Chromium), joined)
            .expect_err("a task that did not complete must not be settled into a probe outcome");
        assert_eq!(finding.check_id, CHROMIUM_ID);
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
    fn the_three_timeouts_stay_strictly_nested_so_this_checks_own_arm_is_reachable() {
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

    /// One implementation, two ids. A check that hardcoded
    /// `browser/chromium-missing` while serving both engines would report
    /// obscura's verdict under Chromium's name, and `doctor --only` could
    /// never reach it.
    #[test]
    fn each_engine_gets_its_own_check_id_and_its_own_install_command() {
        let chromium = EngineMissingCheck::for_engine(Engine::Chromium);
        let obscura = EngineMissingCheck::for_engine(Engine::Obscura);
        assert_eq!(chromium.id(), "browser/chromium-missing");
        assert_eq!(obscura.id(), "browser/obscura-missing");
        assert_ne!(chromium.title(), obscura.title());

        let c = missing_finding(Engine::Chromium, "no system browser");
        let o = missing_finding(Engine::Obscura, "no binary at the pinned path");
        let ctext = format!("{} {}", c.detail, c.fix_hint.clone().unwrap_or_default());
        let otext = format!("{} {}", o.detail, o.fix_hint.clone().unwrap_or_default());
        assert!(
            ctext.contains("playwright-cli install-browser chromium"),
            "{ctext}"
        );
        assert!(
            otext.contains("capability:\"obscura\""),
            "the obscura remedy must name the capability the tool takes: {otext}"
        );
        assert!(
            otext.contains("[general.browser.obscura] binary_path"),
            "and the pin that bypasses the download: {otext}"
        );
        assert!(
            !otext.contains("playwright-cli"),
            "obscura is not supplied by playwright-cli; naming it sends the \
             operator to a command that cannot help: {otext}"
        );
        // Info on both, for the reason the Chromium check already states: an
        // engine-less host must not turn `aleph-server doctor`'s exit code
        // into a constant.
        assert_eq!(c.severity, crate::diagnostics::finding::Severity::Info);
        assert_eq!(o.severity, crate::diagnostics::finding::Severity::Info);
    }

    /// The obscura remedy must name obscura's OWN mirror key. The Playwright
    /// key next door (`[general.browser.runtime] download_host`) is
    /// `PLAYWRIGHT_DOWNLOAD_HOST`, which serves no GitHub release tree —
    /// naming it here would send an operator to set a value
    /// `github_release::configured_host` deliberately does not read (判据 §17:
    /// 错的标签比缺的贵).
    #[test]
    fn the_obscura_mirror_hint_names_the_obscura_section_not_the_playwright_one() {
        let hint = missing_finding(Engine::Obscura, "nothing found")
            .fix_hint
            .expect("the obscura missing finding carries a fix hint");
        assert!(
            hint.contains("[general.browser.obscura] download_host"),
            "{hint}"
        );
        assert!(
            !hint.contains("[general.browser.runtime]"),
            "the Playwright CDN mirror cannot serve a GitHub release: {hint}"
        );
    }

    /// On a platform with no release asset the answer is neither "missing"
    /// (which reads as *install it*) nor "available". A gate that closes must
    /// name a door that opens (判据 §14) — and here the door is the other
    /// engine, not an install that cannot succeed.
    #[test]
    fn the_unavailable_platform_finding_says_so_instead_of_offering_an_install() {
        let f = unavailable_platform_finding(Engine::Obscura, "windows", "aarch64");
        assert_eq!(f.check_id, "browser/obscura-missing");
        assert!(f.detail.contains("aarch64"), "{}", f.detail);
        assert!(
            f.detail.contains("chromium"),
            "names what IS available here: {}",
            f.detail
        );
        assert!(
            !format!("{} {}", f.detail, f.fix_hint.clone().unwrap_or_default())
                .contains("runtime_manage"),
            "an install command that cannot succeed must not be offered"
        );
    }

    /// Registered, not merely defined. `default_registry` is what the offline
    /// `aleph-server doctor` builds; a check absent from it never runs and
    /// nothing else in the tree notices (判据 §7 — both ends present, no wire).
    ///
    /// **Derived from `Engine::ALL`, not two literals.** The two-literal
    /// version could not see a third engine arriving without a check: the
    /// compiler forces a new arm into `id_for` and into `locate_all`, but the
    /// registration list compiled fine, so the engine silently got no doctor
    /// check and this test stayed green. `ALL` is itself emitted by
    /// `declare_engines!` from the enum's own variant list, so iterating it is
    /// iterating the declared set rather than a second copy of it.
    #[test]
    fn every_engine_gets_a_registered_check_in_the_default_registry() {
        let engine = crate::diagnostics::DiagnosticEngine::default_registry()
            .expect("the default registry must build");
        let ids = engine.check_ids();
        for e in Engine::ALL {
            assert!(
                ids.contains(&id_for(e)),
                "{} has no registered doctor check: {ids:?}",
                e.as_str()
            );
        }
        // The two ids are also user-facing wire values (`doctor --only`, the
        // Part 4 settlement), so they are pinned as literals as well —
        // renaming a file may not rename these.
        assert!(ids.contains(&"browser/chromium-missing"), "{ids:?}");
        assert!(ids.contains(&"browser/obscura-missing"), "{ids:?}");
    }
}
