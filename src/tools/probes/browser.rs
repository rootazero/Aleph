//! `BrowserRuntimeProbe` — gates the `browser_*` tool family on the
//! presence of a usable browser runtime.
//!
//! `BrowserDriver` (`browser::profile`) has THREE variants and
//! `browser::manager::get_backend` has three match arms, one per variant. All
//! three route to a real backend and this probe now asks after the
//! prerequisites of **all three**:
//!   * `BrowserDriver::ExistingSession` → `ChromeMcpBackend`, which attaches to
//!     a locally-installed Chromium by launching `npx chrome-devtools-mcp`. It
//!     needs **both** a Chromium binary ([`find_chromium`]) **and** `npx`.
//!   * `BrowserDriver::Managed` → `PlaywrightCliBackend`, which runs the
//!     ledger-provisioned `playwright-cli` binary (`browser::playwright_cli`
//!     resolves it through `runtimes::ensure_capability("playwright-cli", …)`)
//!     and brings its own Chromium. It does **not** run `npx`.
//!   * `BrowserDriver::Cdp` → `CdpBackend`, over Aleph's own CDP client. Its
//!     prerequisite is an engine binary this process can launch and speak CDP
//!     to: obscura (through the launcher's own `resolve_obscura_binary`) or a
//!     system Chromium. No `npx` — that is the existing-session driver's
//!     launcher, not this one's.
//!
//! # Why the third question was added, and what it cost to not have it
//!
//! This file used to ask two of the three, on the stated ground that "no
//! auto-injected profile uses `Cdp`, so a `cdp` profile exists only where an
//! operator wrote one". That was **true when it was written and false from
//! `71d973920`**, which flipped `ProfileManager::new`'s injected `default`
//! profile to `driver = cdp` / `engine = obscura`. Nothing re-read the
//! sentence, because it describes another module's behaviour and that module
//! had no reason to tell this one (判据 §1, fourth form).
//!
//! What that cost, concretely: a machine provisioned exactly the way the
//! dual-engine branch intends — obscura installed, no `playwright-cli`, no
//! `npx` — ran the default profile perfectly and had all 26 `browser_*` tools
//! withheld, refused with a sentence naming the two runtimes the branch exists
//! to stop requiring (判据 §14 — a closed gate naming a door that is now the
//! wrong door).
//!
//! # What `Healthy` here does and does not claim
//!
//! It is a DISJUNCTION over drivers: "some profile on this host could run", not
//! "every profile could". The mirror case is real and deliberate — with
//! `playwright-cli` provisioned and no obscura, this says `Healthy` while a
//! default-profile call cannot launch an engine. Per-profile readiness is
//! `browser/obscura-missing` and `browser/chromium-missing`'s question
//! (`diagnostics::checks::engine_missing`), which answer per engine and name
//! the install command; this gate only decides whether the family is worth
//! offering at all.
//!
//! [`self::tests::every_auto_injected_profile_uses_a_driver_this_probe_asks_about`]
//! is the guard that would have caught the flip: it derives its question from
//! the profiles `ProfileManager::new` actually injects rather than from a
//! remembered sentence, so the default moving into an uncovered set reddens
//! this file. `covered_drivers_is_every_driver` ties the coverage claim to
//! `BrowserDriver::ALL` — together they catch both "a fourth variant appeared"
//! and "the default moved", which is the pair the previous guard could only
//! half answer (判据 §3).
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

use crate::browser::engine::obscura::resolve_obscura_binary;
use crate::browser::find_chromium;
use crate::browser::profile::ObscuraRuntimeConfig;
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

/// Whether the CDP driver could launch an engine: obscura, or a system
/// Chromium. A disjunction because `ProfileConfig::resolved_engine` picks per
/// profile and this probe gates the family, not a profile.
///
/// The obscura half goes through the LAUNCHER's own resolver
/// (`browser::engine::obscura::resolve_obscura_binary`) rather than through a
/// second `which` written here — the rule `managed_cli_path`'s doc states, and
/// the one the `npx` mistake broke. It is pure PATH/filesystem IO: a pin
/// check, a `which`, a ledger read, no subprocess, matching this module's
/// "blocking work" contract below.
///
/// The Chromium half is [`find_chromium`] — the pin plus the system browser —
/// and deliberately NOT `chromium_resolve::resolve_binary`, which spawns
/// `playwright-cli install-browser --dry-run`. A sensor that shells out on
/// every 5-minute refresh to gate a tool list is the wrong trade; the Playwright
/// route is already covered by `managed_driver_ready`, whose `Some` implies a
/// CLI that brings its own Chromium.
fn cdp_driver_ready(obscura: &ObscuraRuntimeConfig) -> bool {
    resolve_obscura_binary(obscura).is_ok() || find_chromium().is_ok()
}

/// What a host needs for the `browser_*` family to be offered at all — one
/// clause per `BrowserDriver`, in the order [`BrowserDriver::ALL`] declares
/// them.
///
/// `pub(crate)` and quoted verbatim by the doctor twin
/// (`diagnostics::checks::browser_runtime`) rather than re-worded there. The
/// twin is what EXPLAINS a withheld family to the operator, and a twin that
/// names its own set of prerequisites is how the family spent four rounds
/// being explained by a sentence about `npx` (判据 §1, §16). One sentence, one
/// author, and a driver added to the gate reaches the explanation with it.
pub(crate) const GATE_REQUIREMENTS: &str = "no provisioned playwright-cli (managed driver), \
     no Chromium + npx (existing-session driver), and no obscura or Chromium binary \
     (cdp driver)";

/// Reports whether *any* browser driver is reachable. The shared
/// `Arc<BrowserRuntimeProbe>` is registered under every `browser_*` tool name.
///
/// Carries the obscura runtime config because the CDP driver's prerequisite is
/// a pinned path an operator may have written (`[general.browser.obscura]
/// binary_path`), and a sensor blind to the pin would withhold 26 tools on a
/// machine that is correctly provisioned — the same failure this probe's third
/// question exists to remove, one config key further in.
#[derive(Default)]
pub struct BrowserRuntimeProbe {
    obscura: ObscuraRuntimeConfig,
}

impl BrowserRuntimeProbe {
    #[must_use]
    pub const fn new(obscura: ObscuraRuntimeConfig) -> Self {
        Self { obscura }
    }
}

#[async_trait]
impl ToolHealthProbe for BrowserRuntimeProbe {
    async fn probe(&self) -> ProbeResult {
        // A join error means the blocking pool could not answer; treat that as
        // "no runtime" rather than as health, for the same reason the doctor
        // twin does — an unknown must never be read as healthy.
        let obscura = self.obscura.clone();
        let usable = tokio::task::spawn_blocking(move || {
            managed_driver_ready() || existing_session_driver_ready() || cdp_driver_ready(&obscura)
        })
        .await
        .unwrap_or(false);

        if usable {
            return ProbeResult::Healthy;
        }
        ProbeResult::Unhealthy {
            reason: HealthReason::DependencyDown(Cow::Borrowed(GATE_REQUIREMENTS)),
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
    use crate::browser::profile::BrowserDriver;

    /// The drivers this probe's verdict covers — every one of them, since the
    /// CDP question was added.
    const COVERED_DRIVERS: [BrowserDriver; 3] = [
        BrowserDriver::Managed,
        BrowserDriver::ExistingSession,
        BrowserDriver::Cdp,
    ];

    /// Ties the module doc's coverage claim to `BrowserDriver::ALL`, so a
    /// fourth driver reddens this file instead of leaving the doc quietly
    /// wrong (the exact shape K4 fixed: the doc said "two backends... total"
    /// against a three-armed `get_backend`).
    ///
    /// This catches ONE of the two ways the doc can rot — a variant appearing.
    /// It is structurally blind to the other, "the default moved into a set
    /// this probe does not ask about", which is what actually happened at
    /// `71d973920`; that one is
    /// [`every_auto_injected_profile_uses_a_driver_this_probe_asks_about`]'s
    /// job. Two guards because they recognise two different shapes (判据 §3).
    #[test]
    fn covered_drivers_is_every_driver() {
        assert_eq!(
            BrowserDriver::ALL.len(),
            COVERED_DRIVERS.len(),
            "BrowserDriver gained or lost a variant; re-check what this \
             probe covers and update COVERED_DRIVERS and the module doc \
             together"
        );
        for d in BrowserDriver::ALL {
            assert!(
                COVERED_DRIVERS.contains(&d),
                "{d:?} is a driver this probe asks nothing about, so `Healthy` \
                 says nothing about a profile using it"
            );
        }
    }

    /// **The guard that would have caught the flip.**
    ///
    /// It does not ask "how many drivers are there" — it asks the profiles
    /// `ProfileManager::new` actually injects which driver they use, and
    /// requires each answer to be one this probe has a prerequisite question
    /// for. A default profile moving to a driver this gate does not ask about
    /// means every install ships a working browser and a withheld tool family,
    /// which is exactly what `71d973920` produced and what nothing here was
    /// able to see: the previous pair of guards recognised "a fourth variant
    /// appeared" and "the split is still the split", and both stayed green
    /// (判据 §3, §5).
    ///
    /// Derived from the injected profiles rather than from a list of names, so
    /// it cannot be satisfied by re-stating today's answer.
    #[test]
    fn every_auto_injected_profile_uses_a_driver_this_probe_asks_about() {
        use crate::browser::manager::ProfileManager;
        use crate::browser::profile::BrowserSystemConfig;

        let manager = ProfileManager::new(BrowserSystemConfig::default());
        let injected: Vec<String> = manager
            .list_profiles()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert!(
            !injected.is_empty(),
            "ProfileManager::new injected nothing, so this census certifies \
             the gate by looking at no profiles (判据 §2)"
        );
        for name in injected {
            let driver = manager
                .get_config(&name)
                .unwrap_or_else(|| panic!("profile {name} vanished between two reads"))
                .driver;
            assert!(
                COVERED_DRIVERS.contains(&driver),
                "the auto-injected profile {name:?} uses driver {driver:?}, which \
                 this probe asks no prerequisite question about — so on a machine \
                 provisioned for it the family is withheld while it runs fine"
            );
        }
    }

    /// The premise pin for the module doc, inverted twice now.
    ///
    /// It first read `get_backend_still_refuses_cdp_by_name`, pinning "`Cdp`
    /// has no backend in this build". Task 14 gave that arm a real
    /// `CdpBackend` and it went red as intended. It then pinned "…and this
    /// probe asks nothing about it" — a sentence that became a DEFECT rather
    /// than a description at `71d973920`, at which point this guard was
    /// actively holding the defect in place. It is inverted again rather than
    /// deleted: a sentence with no guard is a sentence that rots.
    ///
    /// `covered_drivers_is_every_driver` does not cover this. It catches a
    /// fourth variant appearing and nothing else — reverting the `Cdp` arm to
    /// a refusal would leave `BrowserDriver::ALL` and `COVERED_DRIVERS`
    /// untouched, and it green.
    #[test]
    fn a_cdp_profile_routes_to_a_backend_this_probe_asks_about() {
        use crate::browser::manager::ProfileManager;
        use crate::browser::profile::{BrowserSystemConfig, ProfileConfig};

        let mut config = BrowserSystemConfig::default();
        config.profiles.insert(
            "cdp-profile".to_string(),
            ProfileConfig {
                driver: BrowserDriver::Cdp,
                // Explicit so the routing never derives a path from
                // `ALEPH_HOME`: this test lives outside `src/browser/`, so the
                // class guard that serialises those does not cover it, and a
                // probe test must not touch the developer's browser storage.
                user_data_dir: Some("/nonexistent/aleph-probe-test".into()),
                ..Default::default()
            },
        );
        let manager = ProfileManager::new(config);
        // `Arc<dyn BrowserBackend>` isn't `Debug`, so `.expect(..)` can't be
        // used here — match instead.
        match manager.get_backend("cdp-profile") {
            Ok(_) => {}
            Err(e) => panic!(
                "a driver=cdp profile must route to a backend — if this now \
                 refuses, the Cdp arm was reverted and this module's doc \
                 (which says all three arms route) is false: {e}"
            ),
        }

        // …and the probe asks after its prerequisite. Both halves in one test
        // on purpose — the doc's sentence is the CONJUNCTION, and either half
        // alone can be true while the sentence is false.
        assert!(
            COVERED_DRIVERS.contains(&BrowserDriver::Cdp),
            "the doc says this probe asks after the CDP driver's prerequisite; \
             COVERED_DRIVERS now claims otherwise"
        );
    }

    #[tokio::test]
    async fn probe_agrees_with_the_per_driver_prerequisites() {
        // The host's toolchain is not assumed: assert the probe agrees with the
        // same discovery primitives it delegates to. What this pins is that the
        // verdict is derived from BOTH drivers' real prerequisites — a machine
        // with `npx` but no provisioned CLI and no Chromium must read unhealthy,
        // which is exactly the case the old `which("npx")` stage got wrong.
        let obscura = ObscuraRuntimeConfig::default();
        let runtime_present =
            managed_driver_ready() || existing_session_driver_ready() || cdp_driver_ready(&obscura);
        match BrowserRuntimeProbe::new(obscura).probe().await {
            ProbeResult::Healthy => assert!(
                runtime_present,
                "probe said Healthy but no driver's prerequisites were met"
            ),
            ProbeResult::Unhealthy { reason, .. } => {
                assert!(
                    !runtime_present,
                    "probe said Unhealthy but a driver was runnable"
                );
                assert_eq!(reason.short_label(), GATE_REQUIREMENTS);
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
        assert!(
            BrowserRuntimeProbe::new(ObscuraRuntimeConfig::default()).ttl()
                > Duration::from_secs(30)
        );
    }
}
