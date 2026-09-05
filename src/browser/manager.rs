// Browser profile lifecycle manager.
// Manages profile instances: registration, state tracking, idle reclamation.

use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::Duration;

use arc_swap::ArcSwap;

use crate::sync_primitives::{AtomicBool, Mutex, Ordering, RwLock};

use super::backend::BrowserBackend;
use super::chrome_mcp::ChromeMcpDriver;
use super::chrome_mcp_backend::ChromeMcpBackend;
use super::error::BrowserError;
use super::network_policy::{BrowserSsrfGuard, PolicyViolation, SsrfConfig};
use super::playwright_cli::PlaywrightCliDriver;
use super::playwright_cli_backend::PlaywrightCliBackend;
use super::playwright_launch::{LaunchPolicy, SessionLaunch};
use super::profile::{
    BrowserDriver, BrowserSystemConfig, BrowserType, PlaywrightCliConfig, ProfileConfig,
};
use super::tab_registry::{parse_tab_ids, TabRegistry};

/// The manager the running daemon actually serves browser tools from, so a
/// config write can reach it (see [`apply_policy_live`]).
///
/// A `Weak` on purpose: the handle must not keep a manager alive past its
/// owner, and a stale entry must fail to upgrade rather than silently apply a
/// policy to a manager nobody uses. Published by [`ProfileManager::spawn_idle_reaper`].
static LIVE_MANAGER: Mutex<Option<Weak<ProfileManager>>> = Mutex::new(None);

/// Hot-apply a new SSRF policy onto the running browser manager.
///
/// Returns `true` when it landed. `false` means no manager is published (a CLI
/// process, a test, or before boot wired one up) and the caller must NOT report
/// the change as live — the same honest-downgrade contract as
/// [`crate::config::live_apply::apply_live_sections`], whose `route` arm this
/// mirrors: a process-global handle, poked from wherever the config write lands,
/// and a no-op that says so when it is absent.
///
/// Scope is the SSRF policy only. Everything else in `BrowserSystemConfig`
/// (per-profile drivers, the Playwright CLI timeouts, the Chrome launch
/// snapshot the MCP driver holds) is still captured at construction and needs a
/// restart; claiming otherwise here would just move the lie.
pub fn apply_policy_live(policy: SsrfConfig) -> bool {
    let handle = LIVE_MANAGER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    apply_policy_to(handle.as_ref(), policy)
}

/// Body of [`apply_policy_live`] against an explicit handle, so both arms —
/// including "the published manager is gone" — are testable without racing
/// whatever else this process published into the global.
fn apply_policy_to(handle: Option<&Weak<ProfileManager>>, policy: SsrfConfig) -> bool {
    match handle.and_then(Weak::upgrade) {
        Some(mgr) => {
            mgr.apply_policy(policy);
            true
        }
        None => {
            tracing::debug!(
                "browser SSRF policy saved but not hot-applied: no live ProfileManager is published"
            );
            false
        }
    }
}

/// Stop the browsers of the manager the running daemon serves.
///
/// Shaped exactly like [`crate::builtin_tools::bash_exec::kill_all_running_background`],
/// for the same reason its comment gives at the shutdown call site: an
/// automatic teardown is best-effort once the runtime itself is being torn
/// down, so the daemon calls this explicitly. Returns 0 — honestly — when no
/// manager is published (a CLI process, a test, or before boot wired one up).
pub fn shutdown_browsers_global() -> usize {
    let handle = LIVE_MANAGER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    handle
        .as_ref()
        .and_then(Weak::upgrade)
        .map_or(0, |mgr| mgr.shutdown_browsers())
}

/// Manages the lifecycle of browser profiles.
pub struct ProfileManager {
    profiles: RwLock<HashMap<String, ManagedProfile>>,
    /// Live SSRF policy. Swappable because `browser.update` writes a new one at
    /// runtime and every backend is built per call — a boot-time snapshot meant
    /// the RPC reported success while the running guard never changed.
    ssrf_guard: ArcSwap<BrowserSsrfGuard>,
    config: BrowserSystemConfig,
    chrome_mcp_driver: Arc<ChromeMcpDriver>,
    playwright_cli_driver: Arc<PlaywrightCliDriver>,
    idle_reaper_started: AtomicBool,
    /// Per-tab lifecycle tracking for Managed profiles (idle reclamation + cap).
    tab_registry: TabRegistry,
}

struct ManagedProfile {
    config: ProfileConfig,
    last_activity: std::time::Instant,
}

impl ProfileManager {
    #[must_use]
    pub fn new(config: BrowserSystemConfig) -> Self {
        let ssrf_guard = ArcSwap::from_pointee(BrowserSsrfGuard::new(config.policy.clone()));
        let playwright_cli_driver = Arc::new(PlaywrightCliDriver::new(
            config.playwright_cli.clone(),
            config.runtime.clone(),
        ));

        let mut profiles = HashMap::new();

        if config.profiles.is_empty() {
            // Create default profile with Managed driver if none configured.
            profiles.insert(
                "default".into(),
                ManagedProfile {
                    config: ProfileConfig::default(),
                    last_activity: std::time::Instant::now(),
                },
            );
        } else {
            for (name, profile_config) in &config.profiles {
                profiles.insert(
                    name.clone(),
                    ManagedProfile {
                        config: profile_config.clone(),
                        last_activity: std::time::Instant::now(),
                    },
                );
            }
        }

        // Auto-inject "default" profile with Managed driver if not already present.
        if !profiles.contains_key("default") {
            profiles.insert(
                "default".into(),
                ManagedProfile {
                    config: ProfileConfig {
                        driver: BrowserDriver::Managed,
                        ..Default::default()
                    },
                    last_activity: std::time::Instant::now(),
                },
            );
        }

        // Auto-inject "user" profile with ExistingSession driver if not already present.
        if !profiles.contains_key("user") {
            profiles.insert(
                "user".into(),
                ManagedProfile {
                    config: ProfileConfig {
                        browser: BrowserType::Chrome,
                        driver: BrowserDriver::ExistingSession,
                        ..Default::default()
                    },
                    last_activity: std::time::Instant::now(),
                },
            );
        }

        // The Chrome MCP driver consults the profile map when it has to launch
        // Chrome itself (engine preference, proxy, user-data-dir, extra args).
        // Hand it the merged set — including the auto-injected "default"/"user"
        // entries above — so its view matches what the manager routes on.
        let chrome_mcp_driver = Arc::new(ChromeMcpDriver::new(
            config.chrome_mcp.clone(),
            profiles
                .iter()
                .map(|(name, p)| (name.clone(), p.config.clone()))
                .collect(),
        ));

        Self {
            profiles: RwLock::new(profiles),
            ssrf_guard,
            config,
            chrome_mcp_driver,
            playwright_cli_driver,
            idle_reaper_started: AtomicBool::new(false),
            tab_registry: TabRegistry::new(),
        }
    }

    /// Hot-swap the SSRF policy. Backends are constructed per call by
    /// [`Self::get_backend`], so the next browser action — and every direct
    /// `check_*` on this manager — uses the new policy without a restart.
    pub fn apply_policy(&self, policy: SsrfConfig) {
        self.ssrf_guard
            .store(Arc::new(BrowserSsrfGuard::new(policy)));
        tracing::info!("browser SSRF policy hot-applied");
    }

    /// Spawn the idle-profile reaper on a background tokio task, at most once
    /// per `ProfileManager` instance. The reaper sweeps every `interval_secs`
    /// and tears down Chrome MCP sessions whose profile is past its idle
    /// timeout. Idempotent — subsequent calls are no-ops.
    ///
    /// It is also where the previous run's orphaned Chromium processes are
    /// reaped — same argument as the live-config handle below: "the manager
    /// whose reaper runs" is precisely "the manager the daemon serves from".
    ///
    /// Also publishes this manager as the process-global live-config target
    /// (see [`apply_policy_live`]). This is the daemon's one boot hook that
    /// already owns the `Arc` and runs exactly once per served manager — "the
    /// manager whose reaper runs" is precisely "the manager the daemon serves
    /// from", and a `ProfileManager` built ad hoc (tests, CLI) never calls this
    /// and so never claims the handle.
    pub fn spawn_idle_reaper(self: &Arc<Self>, interval_secs: u64) {
        if self.idle_reaper_started.swap(true, Ordering::AcqRel) {
            return;
        }
        // BROWSER-R4-08: claim-or-skip rather than last-write-wins.
        // The previous shape overwrote LIVE_MANAGER unconditionally,
        // so a second ProfileManager invocation (a test that builds
        // its own manager, a re-initialised daemon) would silently
        // steal the handle and the live apply_policy_live arm would
        // hot-apply to whichever manager most recently published.
        // Only install the handle when the slot is empty or the
        // existing weak has been dropped (try_unwrap succeeds).
        let mut slot = LIVE_MANAGER.lock().unwrap_or_else(|e| e.into_inner());
        match slot.as_ref() {
            // The previous manager is still alive: refuse to steal the
            // handle. The caller (typically a test) can run its own
            // reaper locally; it does not need the global one.
            Some(existing) if existing.strong_count() > 0 => {
                tracing::warn!(
                    "ProfileManager::spawn_idle_reaper: live manager already installed; \
                     refusing to claim the global reaper slot"
                );
                return;
            }
            // Slot empty OR previous weak handle is already dead — the
            // latter means the prior daemon (or test) has fully torn down.
            // Surface the steal so a future refactor that changes the
            // install order shows up in logs instead of silently shadowing
            // the live-config target.
            Some(_) | None => {
                tracing::info!(
                    "ProfileManager::spawn_idle_reaper: claiming the global reaper slot"
                );
            }
        }
        // Boot hook, and the only one that runs exactly once per SERVED
        // manager (a `ProfileManager` built by a test or a CLI never claims the
        // slot above). Anything Aleph launched before a crash is still running:
        // Chrome does not exit when its parent does, and under `attach` the CLI
        // was never its parent anyway.
        //
        // Detached because boot must not wait for it — nothing downstream
        // reads the count.
        tokio::spawn(async {
            match Self::sweep_orphaned_chromium().await {
                Ok(0) => {}
                Ok(n) => {
                    tracing::info!("reaped {n} orphaned chromium process(es) from a previous run");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "the orphaned-chromium sweep did not complete")
                }
            }
        });
        *slot = Some(Arc::downgrade(self));
        let weak = Arc::downgrade(self);
        let interval = std::time::Duration::from_secs(interval_secs.max(5));
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let Some(mgr) = weak.upgrade() else {
                    tracing::debug!("ProfileManager idle reaper exiting (manager dropped)");
                    break;
                };
                let reaped = mgr.reap_idle().await;
                if reaped > 0 {
                    tracing::info!("Browser idle reaper swept {reaped} profile(s)");
                }
                let tabs = mgr.reap_idle_tabs().await;
                if tabs > 0 {
                    tracing::info!("Browser idle reaper closed {tabs} idle/over-cap tab(s)");
                }
            }
        });
    }

    /// The boot sweep's outward-reaching leaf: read the real sidecar registry
    /// and kill whatever a previous process left running.
    ///
    /// Sealed under `cfg(test)` for the same reason
    /// `PlaywrightCliDriver::provision_binary` is, and the seal is not
    /// theoretical here: [`Self::spawn_idle_reaper`] **has a unit-test caller**
    /// (`gateway::handlers::browser_config`), and this task is detached, so it
    /// outlives the test body — including that test's `AlephHomeEnvGuard`. It
    /// would therefore resolve the *developer's real* `$ALEPH_HOME` after the
    /// guard restored it, and kill the Chromium of an Aleph they have running.
    ///
    /// What the seal costs is only the wire, not the decision: the decision is
    /// covered against injected effects in `chromium_launch::reap_orphans`, and
    /// the wire is pinned by `the_boot_hook_still_calls_the_orphan_sweep`.
    ///
    /// Off the async worker: the sweep does a `read_dir`, a `sysinfo` refresh
    /// per record and possibly a kill, and `with_process_specifics` is
    /// documented as syscall-heavy.
    #[cfg(not(test))]
    async fn sweep_orphaned_chromium() -> Result<usize, tokio::task::JoinError> {
        tokio::task::spawn_blocking(super::chromium_launch::reap_orphans_now).await
    }

    /// The sealed twin. See the production one above for why it is sealed.
    #[cfg(test)]
    #[allow(clippy::unused_async)]
    async fn sweep_orphaned_chromium() -> Result<usize, tokio::task::JoinError> {
        Ok(0)
    }

    /// The managed driver's configuration, for the one consumer that runs a
    /// `playwright-cli` session of its own rather than through a profile:
    /// `pdf_generate`'s browser engine.
    ///
    /// Exposed rather than left to `PlaywrightCliConfig::default()` because a
    /// second construction site inherits none of the first's settings — the PDF
    /// engine was resolving its binary as though the operator had pinned
    /// nothing, so an install whose `binary_path` points off `PATH` had working
    /// browser tools and a PDF engine that either fell back to the native
    /// renderer or reached for the network installer.
    #[must_use]
    pub const fn playwright_cli_config(&self) -> &PlaywrightCliConfig {
        &self.config.playwright_cli
    }

    /// The `[browser.runtime]` section — where this Aleph's Chromium comes from.
    ///
    /// The twin of [`Self::playwright_cli_config`], and it exists for the same
    /// reason: `pdf_generate` builds a `PlaywrightCliDriver` of its own, and
    /// that driver now launches a browser, so a construction site that
    /// inherited the CLI settings but not the browser ones would honour an
    /// operator's pin in one half of Aleph and ignore it in the other.
    ///
    /// ⚠️ Task 6 was told to add `ProfileManager` accessors. This is one of
    /// them, added early because `pdf_generate` needed it; do not add a second.
    pub const fn runtime_config(&self) -> &super::profile::BrowserRuntimeConfig {
        &self.config.runtime
    }

    /// Route a profile to its appropriate `BrowserBackend` instance.
    ///
    /// - `BrowserDriver::Managed`         → `PlaywrightCliBackend`
    /// - `BrowserDriver::ExistingSession` → `ChromeMcpBackend`
    pub fn get_backend(&self, profile_name: &str) -> Result<Arc<dyn BrowserBackend>, BrowserError> {
        let cfg = self
            .get_config(profile_name)
            .ok_or_else(|| BrowserError::ProfileNotFound(profile_name.into()))?;
        match cfg.driver {
            BrowserDriver::Managed => {
                let headless = cfg.headless.unwrap_or(self.config.playwright_cli.headless);
                Ok(Arc::new(PlaywrightCliBackend::new(
                    self.playwright_cli_driver.clone(),
                    profile_name.to_string(),
                    self.ssrf_guard.load_full(),
                    SessionLaunch::from_profile(&cfg, headless),
                )))
            }
            BrowserDriver::ExistingSession => Ok(Arc::new(ChromeMcpBackend::new(
                self.chrome_mcp_driver.clone(),
                profile_name.to_string(),
                self.ssrf_guard.load_full(),
            ))),
        }
    }

    /// Sweep idle profiles past their `idle_timeout_secs`, both drivers.
    /// Returns the number of profiles reaped (best-effort; safe to call any time).
    ///
    /// - `ExistingSession` → tear down the Chrome MCP session. Liveness comes
    ///   from the driver's session map (the only place a session exists).
    /// - `Managed` → `playwright-cli close` (which now only disconnects the CLI
    ///   session) **and then** killing Aleph's own Chromium. Under the previous
    ///   arrangement `close` destroyed the browser the CLI had launched; under
    ///   `attach --cdp` it leaves it running, so stopping at `close` would have
    ///   reported a reaped profile over a browser that never went away.
    ///
    /// The close runs under [`LaunchPolicy::Refuse`]: a reaper that opened a
    /// browser in order to close it would be absurd, and the lazy launch makes
    /// that a real possibility rather than a hypothetical one.
    pub async fn reap_idle(&self) -> usize {
        let mut reaped = 0;
        for name in self.idle_existing_session_profiles() {
            self.chrome_mcp_driver.destroy_session(&name).await;
            reaped += 1;
        }
        for name in self.idle_managed_profiles() {
            match self
                .playwright_cli_driver
                .run(
                    &name,
                    LaunchPolicy::Refuse,
                    &["close"],
                    std::time::Duration::from_secs(self.config.playwright_cli.action_timeout_secs),
                )
                .await
            {
                // Already gone is the same outcome as just-closed.
                Ok(_) | Err(BrowserError::NoSession(_)) => {}
                // Best-effort, and deliberately NOT a gate on the kill below.
                // The CLI's session and the browser are two different things
                // now; an error here is the CLI's opinion of its own session
                // and says nothing about the process Aleph owns. Skipping the
                // kill on it would make an unusable CLI — the state most
                // likely to have leaked a browser in the first place — the one
                // state in which Aleph refuses to reclaim it (判据 §8: a
                // fail-closed answer must not be spent as a value).
                Err(e) => {
                    tracing::warn!(profile = %name, error = %e, "reap_idle: could not close the managed cli session; stopping its browser anyway");
                }
            }
            // `close` under `attach --cdp` is a DISCONNECT: the browser, its
            // pages and their state all survive it (measured). So this is the
            // half that actually reclaims anything, and the only half the
            // count may be earned by — reporting a reaped profile over a
            // browser that never went away is the "success reported for a
            // no-op" shape (判据 §11).
            if self.playwright_cli_driver.shutdown_chromium(&name) {
                reaped += 1;
            } else {
                tracing::warn!(profile = %name, "reap_idle: no chromium to stop for an idle managed profile");
            }
            self.tab_registry.clear_profile(&name);
        }
        reaped
    }

    /// `Managed` profiles idle past their timeout that Aleph has a browser
    /// record for — alive **or** dead.
    ///
    /// Exact, not approximate: the browser is Aleph's own child process, so
    /// "does one exist" is `ChromiumChild::alive`. The `close` below still
    /// tolerates `NoSession` because the CLI's session and the browser are now
    /// two different things — the browser can be alive with no session attached.
    ///
    /// The dead half is not a courtesy. A Chromium that exited on its own
    /// leaves three things behind that only this sweep clears while the daemon
    /// runs — the child record in the driver's map, the sidecar file naming its
    /// pid, and the profile's tab entries — so filtering on `alive` alone would
    /// make "the browser died" mean "there is nothing here to reclaim".
    ///
    /// The filter below reads as `chromium_alive(name) || chromium_died(name)`,
    /// and that is NOT because it behaves differently from a bare "does the
    /// driver have a record for this profile at all" check — algebraically it
    /// does not: [`PlaywrightCliDriver::chromium_died`] is defined as
    /// `key_present && !alive`, so the disjunction reduces to exactly
    /// `key_present`, in every state, not merely today's. It is written this
    /// way because `PlaywrightCliDriver` never exposes a bare "has a record"
    /// accessor — `chromium_alive` / `chromium_died` are the only two names it
    /// hands out for this decision, and they are the vocabulary the rest of
    /// this module already reasons in.
    ///
    /// Scoped to browsers **this** process launched, deliberately: a Chromium
    /// left by a crashed daemon has no idle clock here to be past, and is the
    /// boot sweep's to reclaim, not this sweep's.
    fn idle_managed_profiles(&self) -> Vec<String> {
        let profiles = self.profiles.read().unwrap_or_else(|e| e.into_inner());
        let now = std::time::Instant::now();
        profiles
            .iter()
            .filter(|(name, p)| {
                p.config.driver == BrowserDriver::Managed
                    && is_idle(p.last_activity, now, p.config.idle_timeout_secs)
                    && (self.playwright_cli_driver.chromium_alive(name)
                        || self.playwright_cli_driver.chromium_died(name))
            })
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Whether the profile currently has a live browser session, derived from
    /// the real session-tracking surfaces rather than a state flag:
    /// - `ExistingSession` → a Chrome MCP session exists in the driver.
    /// - `Managed` → Aleph's Chromium for the profile is running. Exact since
    ///   the launch-chain flip; it used to be "the tab registry has tabs",
    ///   which its own doc called an approximation. Exact about **this
    ///   process's** browsers: one orphaned by a previous run reads as
    ///   inactive until [`Self::spawn_idle_reaper`]'s boot sweep disposes of
    ///   it, which is the sweep's job rather than this predicate's.
    pub fn session_active(&self, name: &str) -> bool {
        match self.get_driver(name) {
            Some(BrowserDriver::ExistingSession) => self.chrome_mcp_driver.has_session(name),
            Some(BrowserDriver::Managed) => self.playwright_cli_driver.chromium_alive(name),
            None => false,
        }
    }

    /// `ExistingSession` profiles that have a live Chrome MCP session AND have
    /// been idle longer than their configured timeout. Only these are reaped —
    /// a session that no longer exists needs no teardown, and recent activity
    /// protects live sessions.
    fn idle_existing_session_profiles(&self) -> Vec<String> {
        let profiles = self.profiles.read().unwrap_or_else(|e| e.into_inner());
        // One clock read for the whole sweep, so every profile is judged
        // against the same instant.
        let now = std::time::Instant::now();
        profiles
            .iter()
            .filter(|(name, p)| {
                p.config.driver == BrowserDriver::ExistingSession
                    && is_idle(p.last_activity, now, p.config.idle_timeout_secs)
                    && self.chrome_mcp_driver.has_session(name)
            })
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Record activity on a specific tab so its idle timer resets. No-op for
    /// non-`Managed` profiles — the user's `ExistingSession` tabs are never
    /// tracked or reaped (R5: don't disturb the user).
    pub fn touch_tab(&self, profile_name: &str, tab_id: &str) {
        if let Some(BrowserDriver::Managed) = self.get_driver(profile_name) {
            self.tab_registry.touch(profile_name, tab_id);
        }
    }

    /// Sweep idle / over-cap tabs for every Managed profile with tracked tabs.
    ///
    /// Reconciles the registry against each profile's live `list_tabs` output,
    /// then closes the selected victims (idle beyond `tab_idle_timeout_secs`, or
    /// LRU overflow beyond `max_tabs_per_profile`). The active (most-recently-
    /// used) tab is always protected. Best-effort: any backend error skips that
    /// profile. Returns the number of tabs closed.
    pub async fn reap_idle_tabs(&self) -> usize {
        // Candidates: Managed profiles whose browser was actually used.
        let candidates: Vec<String> = {
            let profiles = self.profiles.read().unwrap_or_else(|e| e.into_inner());
            profiles
                .iter()
                .filter(|(name, p)| {
                    p.config.driver == BrowserDriver::Managed && self.tab_registry.has_tabs(name)
                })
                .map(|(name, _)| name.clone())
                .collect()
        };

        let mut closed = 0;
        for profile in candidates {
            let (max_tabs, idle_secs) = match self.get_config(&profile) {
                Some(c) => (c.max_tabs_per_profile, c.tab_idle_timeout_secs),
                None => continue,
            };
            let backend = match self.get_backend(&profile) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(profile = %profile, error = %e, "reap_idle_tabs: failed to get backend");
                    continue;
                }
            };
            let tabs_text = match backend.list_tabs().await {
                Ok(t) => t,
                Err(e) => {
                    // Browser gone — stop re-probing this profile every sweep.
                    tracing::warn!(profile = %profile, error = %e, "reap_idle_tabs: failed to list tabs");
                    self.tab_registry.clear_profile(&profile);
                    continue;
                }
            };
            let live_ids = parse_tab_ids(&tabs_text);
            let victims = self.tab_registry.select_victims(
                &profile,
                &live_ids,
                max_tabs,
                Duration::from_secs(idle_secs),
            );
            for victim in victims {
                if let Err(e) = backend.close_tab(&victim).await {
                    tracing::warn!(profile = %profile, tab = %victim, error = %e, "reap_idle_tabs: failed to close tab");
                } else {
                    self.tab_registry.forget(&profile, &victim);
                    closed += 1;
                }
            }
        }
        closed
    }

    /// Get the driver mode for a named profile.
    pub fn get_driver(&self, name: &str) -> Option<BrowserDriver> {
        let profiles = self.profiles.read().unwrap_or_else(|e| e.into_inner());
        let result = profiles.get(name).map(|p| p.config.driver.clone());
        tracing::debug!(
            profile = name,
            driver = ?result,
            available_profiles = ?profiles.keys().collect::<Vec<_>>(),
            "ProfileManager::get_driver"
        );
        result
    }

    /// List all profiles with derived session liveness (see [`Self::session_active`]).
    pub fn list_profiles(&self) -> Vec<(String, bool)> {
        let profiles = self.profiles.read().unwrap_or_else(|e| e.into_inner());
        profiles
            .keys()
            .map(|name| (name.clone(), self.session_active(name)))
            .collect()
    }

    /// Get the configuration of a named profile.
    pub fn get_config(&self, name: &str) -> Option<ProfileConfig> {
        let profiles = self.profiles.read().unwrap_or_else(|e| e.into_inner());
        profiles.get(name).map(|p| p.config.clone())
    }

    /// The live CDP endpoint of a `Managed` profile's browser, if it has one.
    ///
    /// The accessor spec §3.2 asks for. `ExistingSession` answers `None` by
    /// construction: that browser is the user's own, Aleph never launched it,
    /// and the live view is deliberately Managed-only — a Chrome the user
    /// started is already on their screen.
    ///
    /// ⚠️ `None` is "**Aleph** has no browser for this profile", which is the
    /// same sentence as "no browser is running" only after
    /// [`Self::spawn_idle_reaper`]'s boot sweep: a Chromium orphaned by a
    /// previous process lives in the sidecar registry, not in this driver's
    /// map (判据 §8 — an absent record is not an absent process).
    // The one allow this task adds, and it replaces two: `PlaywrightCliDriver`'s
    // `endpoint` and `shutdown_chromium` each carried one naming Task 6, and
    // both are consumed here now. This one is the head of that chain — nothing
    // in this crate reads a live endpoint until Plan 2's live view does, and
    // `--lib` (which does not compile `#[cfg(test)]`) sees only that. Delete it
    // with the first non-test caller; if Plan 2 does not land, this is a CUT,
    // not a permanent allow.
    #[allow(dead_code)]
    pub(crate) fn live_endpoint(&self, profile: &str) -> Option<super::CdpEndpoint> {
        match self.get_driver(profile) {
            Some(BrowserDriver::Managed) => self.playwright_cli_driver.endpoint(profile),
            Some(BrowserDriver::ExistingSession) | None => None,
        }
    }

    /// Kill every browser this manager launched. Returns how many were stopped.
    ///
    /// spec §3.6「退出时杀」. `std::process::Child` does not kill on drop, and
    /// under `attach --cdp` the CLI was never the browser's parent — so without
    /// this every restart leaves a Chromium running until the next boot sweep.
    ///
    /// **Must finish inside `SHUTDOWN_FAILSAFE` (5 s, `start/helpers.rs`),**
    /// because one of its two call sites is the wedged-shutdown watchdog and
    /// the `std::process::exit(0)` after it waits for nobody. What that buys
    /// this function is a rule, not a budget: SIGKILL plus a bounded reap per
    /// child, and **never a graceful handshake**. No SIGTERM-then-wait, no CDP
    /// `Browser.close`, no round trip to a browser that may be the reason the
    /// shutdown wedged in the first place. `Child::kill()` is immediate and
    /// `Child::wait()` after a successful kill returns as soon as the kernel
    /// reaps — microseconds, not a negotiation (see `ChromiumChild::shutdown`,
    /// which skips the wait entirely when the kill did not succeed, precisely
    /// so a child we could not signal cannot park this loop).
    pub fn shutdown_browsers(&self) -> usize {
        self.playwright_cli_driver.shutdown_all_chromium()
    }

    /// Validate a URL against the SSRF policy.
    pub async fn check_url(&self, url: &str) -> Result<(), PolicyViolation> {
        self.ssrf_guard.load().check_url(url).await
    }

    /// Validate an agent-initiated navigation target: SSRF policy plus
    /// secret-exfiltration scanning. Use this for `goto`/`open`.
    pub async fn check_navigation(&self, url: &str) -> Result<(), PolicyViolation> {
        self.ssrf_guard.load().check_navigation(url).await
    }

    /// Scan `text` about to be typed into a page form for an embedded
    /// credential (the form-input leg of the secret-egress boundary; mirrors
    /// [`Self::redact_content`]'s delegation pattern). Returns the matched rule
    /// name when the input must be refused, `None` when clean or the flag is off.
    pub fn check_input_secret(&self, text: &str) -> Option<String> {
        self.ssrf_guard.load().check_input(text)
    }

    /// Redact embedded credentials from page-derived `text` before it is
    /// returned to the LLM (the OUT half of the secret-egress boundary). Used by
    /// the content-read tools (snapshot / console / network / evaluate) via the
    /// shared `redact_and_wrap` egress chokepoint. Zero-copy when redaction is
    /// disabled or the text carries no secret.
    pub fn redact_content<'a>(&self, text: &'a str) -> std::borrow::Cow<'a, str> {
        // The returned `Cow` borrows from `text`, never from the guard, so the
        // loaded policy generation can be dropped at the end of the statement
        // and the zero-copy path survives the swap to `ArcSwap`.
        self.ssrf_guard.load().redact_content(text)
    }

    /// Record activity on a profile to reset its idle timer.
    pub fn record_activity(&self, profile_name: &str) {
        let mut profiles = self.profiles.write().unwrap_or_else(|e| e.into_inner());
        if let Some(profile) = profiles.get_mut(profile_name) {
            profile.last_activity = std::time::Instant::now();
        }
    }

    /// Test-only: whether any tabs are tracked for a profile.
    #[cfg(test)]
    pub(crate) fn has_tracked_tabs(&self, profile: &str) -> bool {
        self.tab_registry.has_tabs(profile)
    }

    /// Test-only: hand the managed driver a browser it did not launch, so the
    /// reaper and the shutdown path can be exercised against a REAL pid without
    /// a real Chromium.
    ///
    /// One line of forwarding onto [`PlaywrightCliDriver::insert_test_child`]
    /// and [`ChromiumChild::from_parts`] — both of which say "do not add a
    /// second one", and this is the one. Not `pub`, and `#[cfg(test)]`: a door
    /// that lets something outside put a browser into the driver is a door that
    /// goes around the launch chain.
    #[cfg(test)]
    pub(crate) fn insert_test_child(&self, profile: &str, child: std::process::Child) {
        let endpoint = super::CdpEndpoint {
            http_url: "http://127.0.0.1:1".into(),
            ws_url: "ws://127.0.0.1:1/devtools/browser/test".into(),
            pid: child.id(),
        };
        self.playwright_cli_driver.insert_test_child(
            profile,
            super::chromium_launch::ChromiumChild::from_parts(
                child,
                endpoint,
                std::path::PathBuf::from("/tmp/aleph-test-udd"),
                profile,
            ),
        );
    }
}

/// Whether `last_activity` is older than `timeout_secs` as of `now`. Pure
/// helper so the reaper's timeout filter is unit-testable without a live
/// session.
///
/// `now` is taken explicitly rather than read from the clock inside: it makes
/// the helper a total function of its inputs, and lets a test express "long
/// ago" by moving `now` *forward* instead of subtracting from `Instant::now()`.
/// That subtraction panics wherever the monotonic clock's origin is more recent
/// than the offset — routine on a freshly booted CI VM, where Windows counts
/// `Instant` from system boot.
fn is_idle(last_activity: std::time::Instant, now: std::time::Instant, timeout_secs: u64) -> bool {
    now.saturating_duration_since(last_activity).as_secs() > timeout_secs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::paths::AlephHomeEnvGuard;

    #[test]
    fn test_manager_registers_profiles_from_config() {
        let mut config = BrowserSystemConfig::default();
        config
            .profiles
            .insert("default".into(), ProfileConfig::default());
        config.profiles.insert(
            "work".into(),
            ProfileConfig {
                headless: Some(true),
                ..Default::default()
            },
        );

        let manager = ProfileManager::new(config);
        let profiles = manager.list_profiles();
        // 2 explicit + auto-injected "user" = 3
        assert_eq!(profiles.len(), 3);
        assert!(profiles.iter().any(|p| p.0 == "default"));
        assert!(profiles.iter().any(|p| p.0 == "work"));
        assert!(profiles.iter().any(|p| p.0 == "user"));
    }

    #[test]
    fn test_manager_default_profile_if_none_configured() {
        let config = BrowserSystemConfig::default();
        let manager = ProfileManager::new(config);
        let profiles = manager.list_profiles();
        // "default" + auto-injected "user" = 2
        assert_eq!(profiles.len(), 2);
        assert!(profiles.iter().any(|p| p.0 == "default"));
        assert!(profiles.iter().any(|p| p.0 == "user"));
    }

    #[test]
    fn test_get_profile_state_removed_in_favor_of_session_active() {
        let config = BrowserSystemConfig::default();
        let manager = ProfileManager::new(config);
        // No browser has been used → both profiles report inactive.
        assert!(!manager.session_active("default"));
        assert!(!manager.session_active("user"));
        assert!(!manager.session_active("nonexistent"));

        // Not an approximation any more: Aleph owns the browser process, so
        // `session_active` asks it. A tracked tab says a tab was USED, which is
        // a different fact and no longer stands in for a live browser.
        manager.touch_tab("default", "1");
        assert!(
            !manager.session_active("default"),
            "a tracked tab must not imply a browser that was never launched"
        );
    }

    #[test]
    fn test_is_idle_timeout_filter() {
        // Age is expressed by advancing `now`, never by subtracting from
        // `Instant::now()` — see `is_idle`'s note on the clock origin.
        let touched = std::time::Instant::now();
        assert!(!is_idle(touched, touched, 1800));
        assert!(is_idle(
            touched,
            touched + std::time::Duration::from_secs(1801),
            1800
        ));
        // Boundary: elapsed must strictly exceed the timeout.
        assert!(!is_idle(
            touched,
            touched + std::time::Duration::from_secs(1800),
            1800
        ));
        assert!(!is_idle(touched, touched, 0));
    }

    #[test]
    fn test_auto_injects_user_profile() {
        let config = BrowserSystemConfig::default();
        let manager = ProfileManager::new(config);
        let profiles = manager.list_profiles();
        assert!(profiles.iter().any(|p| p.0 == "default"));
        assert!(profiles.iter().any(|p| p.0 == "user"));
    }

    #[test]
    fn test_user_profile_is_existing_session() {
        let config = BrowserSystemConfig::default();
        let manager = ProfileManager::new(config);
        let user_config = manager.get_config("user").unwrap();
        assert_eq!(user_config.driver, BrowserDriver::ExistingSession);
        assert_eq!(user_config.browser, BrowserType::Chrome);
    }

    #[test]
    fn test_explicit_user_profile_not_overridden() {
        // An explicitly-configured "user" profile must survive the
        // auto-injection pass verbatim. This used `color` as its distinguishing
        // marker until that field was cut in 3757bb4f8; `idle_timeout_secs`
        // carries the same proof (default is 1800, so 999 can only come from
        // the explicit config).
        let mut config = BrowserSystemConfig::default();
        config.profiles.insert(
            "user".into(),
            ProfileConfig {
                browser: BrowserType::Chrome,
                driver: BrowserDriver::ExistingSession,
                idle_timeout_secs: 999,
                ..Default::default()
            },
        );
        let manager = ProfileManager::new(config);
        let user_config = manager.get_config("user").unwrap();
        assert_eq!(user_config.idle_timeout_secs, 999);
        assert_eq!(user_config.driver, BrowserDriver::ExistingSession);
    }

    #[test]
    fn test_get_driver() {
        let config = BrowserSystemConfig::default();
        let manager = ProfileManager::new(config);
        assert_eq!(manager.get_driver("default"), Some(BrowserDriver::Managed));
        assert_eq!(
            manager.get_driver("user"),
            Some(BrowserDriver::ExistingSession)
        );
        assert_eq!(manager.get_driver("nonexistent"), None);
    }

    #[test]
    fn test_get_backend_routes_managed_to_playwright_cli() {
        let mut config = BrowserSystemConfig::default();
        config.profiles.insert(
            "default".into(),
            ProfileConfig {
                driver: BrowserDriver::Managed,
                ..Default::default()
            },
        );
        let manager = ProfileManager::new(config);
        let backend = manager.get_backend("default");
        assert!(backend.is_ok());
    }

    #[test]
    fn test_get_backend_routes_user_to_chrome_mcp() {
        let config = BrowserSystemConfig::default();
        let manager = ProfileManager::new(config);
        let backend = manager.get_backend("user");
        assert!(backend.is_ok());
    }

    #[test]
    fn test_touch_tab_tracks_managed_only() {
        let config = BrowserSystemConfig::default();
        let manager = ProfileManager::new(config);

        // "default" is Managed → tracked.
        manager.touch_tab("default", "1");
        assert!(manager.has_tracked_tabs("default"));

        // "user" is ExistingSession (user's real Chrome) → never tracked.
        manager.touch_tab("user", "1");
        assert!(!manager.has_tracked_tabs("user"));
    }

    #[tokio::test]
    async fn test_reap_idle_tabs_no_browser_is_noop() {
        let config = BrowserSystemConfig::default();
        let manager = ProfileManager::new(config);
        // Track a tab but no browser is running → list_tabs fails, profile is
        // cleared, nothing is closed.
        manager.touch_tab("default", "1");
        assert_eq!(manager.reap_idle_tabs().await, 0);
        assert!(!manager.has_tracked_tabs("default"));
    }

    #[tokio::test]
    async fn test_reap_idle_fresh_manager_is_noop() {
        let config = BrowserSystemConfig::default();
        let manager = ProfileManager::new(config);
        // No live sessions exist → nothing to tear down.
        assert_eq!(manager.reap_idle().await, 0);
    }

    #[tokio::test]
    async fn policy_update_applies_without_a_restart() {
        // Boot with SSRF on: loopback is refused.
        let manager = ProfileManager::new(BrowserSystemConfig::default());
        assert!(manager
            .check_url("http://127.0.0.1:9000/admin")
            .await
            .is_err());

        // The operator turns private-network blocking off (what `browser.update`
        // writes to disk). Before this hot-apply existed the manager kept its
        // boot-time guard and the RPC reported success over a no-op.
        manager.apply_policy(SsrfConfig {
            block_private: false,
            blocked_domains: vec![],
            allowed_domains: vec![],
            block_secrets_in_url: false,
            block_secrets_in_input: false,
            redact_secrets_in_content: false,
        });
        assert!(
            manager
                .check_url("http://127.0.0.1:9000/admin")
                .await
                .is_ok(),
            "the running manager must serve the new policy"
        );
    }

    #[tokio::test]
    async fn live_apply_reaches_a_published_manager_and_downgrades_otherwise() {
        let open = SsrfConfig {
            block_private: false,
            blocked_domains: vec![],
            allowed_domains: vec![],
            block_secrets_in_url: false,
            block_secrets_in_input: false,
            redact_secrets_in_content: false,
        };
        // No handle at all → the caller must be told the change did NOT land
        // (honest downgrade, mirroring config::live_apply).
        assert!(!apply_policy_to(None, open.clone()));

        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        let handle = Arc::downgrade(&manager);
        assert!(apply_policy_to(Some(&handle), open.clone()));
        assert!(manager.check_url("http://127.0.0.1/x").await.is_ok());

        // A handle to a manager that has since been dropped is not a live
        // target either — it must downgrade, not resurrect anything.
        drop(manager);
        assert!(!apply_policy_to(Some(&handle), open));
    }

    /// The boot hook must actually call the orphan sweep.
    ///
    /// A SOURCE pin, and deliberately so: `sweep_orphaned_chromium`'s
    /// production half is `cfg(not(test))` — it reads the real `$ALEPH_HOME`
    /// and kills pids, so no unit test may run it — which leaves the wire
    /// itself unobservable at runtime. Same shape and the same reason as
    /// `both_daemon_exit_paths_reap_background_jobs_and_browsers`. Deleting the
    /// call then fails a test by name, instead of silently letting every
    /// crashed daemon's Chromium survive forever.
    #[test]
    fn the_boot_hook_still_calls_the_orphan_sweep() {
        let src = include_str!("manager.rs").replace('\r', "");
        let production = crate::utils::source_scan::production_prefix(&src);
        assert!(
            production.len() < src.len(),
            "the #[cfg(test)] bound matched nothing — this test would then be \
             reading its own source"
        );
        assert!(
            production.contains("Self::sweep_orphaned_chromium()"),
            "spawn_idle_reaper must still call the boot sweep"
        );
        assert!(
            production.contains("chromium_launch::reap_orphans_now"),
            "the sweep must reach chromium_launch::reap_orphans_now — it is the \
             only thing that ever finds a browser a crashed daemon left running"
        );
    }

    /// A `playwright-cli` stand-in that exits 0 for every verb and, when it is
    /// asked to `close`, records whether `pid` was still alive at that moment.
    ///
    /// It is written *after* the stand-in browser is spawned, which is the only
    /// reason it can name that pid — and naming it is the point: the marker is
    /// how a unit test observes the ORDER of the reaper's two halves without
    /// being able to step inside `reap_idle`.
    #[cfg(unix)]
    fn fake_cli_recording_close(
        dir: &std::path::Path,
        pid: u32,
    ) -> (std::path::PathBuf, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let marker = dir.join("close-saw-a-live-browser");
        let cli = dir.join("fake-playwright-cli");
        std::fs::write(
            &cli,
            format!(
                "#!/bin/sh\n\
                 case \" $* \" in\n\
                 *\" close \"*) kill -0 {pid} 2>/dev/null && : > {marker:?} ;;\n\
                 esac\n\
                 exit 0\n",
                marker = marker.to_string_lossy(),
            ),
        )
        .expect("write the fake playwright-cli");
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755))
            .expect("chmod the fake playwright-cli");
        (cli, marker)
    }

    /// Move a profile's idle clock back so the reaper's timeout filter admits
    /// it, without sleeping through a real timeout.
    ///
    /// `is_idle` reads the monotonic clock and `tokio::time::pause` does not
    /// move that, so a test either waits or backdates. `checked_sub` because
    /// the clock's origin can be more recent than the offset (see `is_idle`);
    /// the caller asserts the profile really did become a candidate, so an
    /// underflow fails loudly instead of turning the test green for the wrong
    /// reason.
    fn backdate(manager: &ProfileManager, profile: &str, ago: Duration) {
        let mut profiles = manager.profiles.write().unwrap_or_else(|e| e.into_inner());
        let entry = profiles.get_mut(profile).expect("profile exists");
        if let Some(t) = std::time::Instant::now().checked_sub(ago) {
            entry.last_activity = t;
        }
    }

    /// A `BrowserSystemConfig` with one `Managed` profile that is idle the
    /// instant it stops being touched.
    fn managed_config(cli: Option<&std::path::Path>) -> BrowserSystemConfig {
        let mut config = BrowserSystemConfig::default();
        config.profiles.insert(
            "default".into(),
            ProfileConfig {
                driver: BrowserDriver::Managed,
                idle_timeout_secs: 0,
                ..Default::default()
            },
        );
        config.playwright_cli.binary_path = cli.map(|p| p.to_string_lossy().into_owned());
        config
    }

    /// The view accessor spec §3.2 asks for, and the one property that makes it
    /// honest: an `ExistingSession` profile has no Aleph-owned browser, so it
    /// must answer `None` rather than somebody else's endpoint. The live view is
    /// Managed-only on purpose — a user's own Chrome is already visible to them.
    ///
    /// The `get_driver` assertion is not decoration. Without it the
    /// `ExistingSession` arm is an empty guard: point it at the driver too and
    /// the test stays green, because no browser exists either way. Pinning
    /// *which* driver the profile has is what makes the arm falsifiable
    /// (判据 §3 — a guard that has never been falsified is not a guard).
    #[tokio::test]
    async fn live_endpoint_is_none_without_a_browser_and_never_answers_for_existing_session() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());
        let manager = ProfileManager::new(BrowserSystemConfig::default());
        // Both auto-injected profiles exist (`ProfileManager::new`): `default`
        // is Managed, `user` is ExistingSession.
        assert_eq!(manager.get_driver("default"), Some(BrowserDriver::Managed));
        assert_eq!(
            manager.get_driver("user"),
            Some(BrowserDriver::ExistingSession),
            "precondition: `user` is the ExistingSession arm this asserts on"
        );
        assert!(manager.live_endpoint("default").is_none());
        assert!(manager.live_endpoint("user").is_none());
        assert!(manager.live_endpoint("no-such-profile").is_none());
    }

    /// The falsifying half of the test above.
    ///
    /// Asserting `None` for `user` on a manager that never launched anything
    /// is an EMPTY guard: point the `ExistingSession` arm at the driver and it
    /// stays green, because the driver has nothing to answer with either
    /// (判据 §3 — a guard that cannot go red is not a guard). So give the
    /// driver something to answer with, under that very profile's key, and the
    /// arm has to be the thing that refuses.
    ///
    /// Not a contrived state: the driver's map is keyed by profile name, so a
    /// profile that is `Managed` today and `ExistingSession` in tomorrow's
    /// config is exactly this shape — and the live view showing Aleph's own
    /// stale browser as "the user's Chrome" is the failure it would produce.
    #[cfg(unix)]
    #[tokio::test]
    async fn live_endpoint_refuses_an_existing_session_profile_that_has_a_child_in_the_map() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());
        let manager = ProfileManager::new(BrowserSystemConfig::default());
        assert_eq!(
            manager.get_driver("user"),
            Some(BrowserDriver::ExistingSession),
            "precondition: `user` is the arm under test"
        );

        let child = std::process::Command::new("sleep")
            .arg("120")
            .spawn()
            .expect("spawn the stand-in browser");
        manager.insert_test_child("user", child);

        assert!(
            manager.live_endpoint("user").is_none(),
            "the live view is Managed-only; an ExistingSession profile must not \
             be handed an endpoint even when the driver has one under its key"
        );
        assert_eq!(
            manager.shutdown_browsers(),
            1,
            "precondition: the driver really was holding a child to answer with"
        );
    }

    /// `session_active` used to answer from the tab registry, which its own doc
    /// called an approximation. Now that Aleph owns the process there is an
    /// exact answer, and the approximation must be GONE rather than kept beside
    /// it — two answers to "does this profile have a browser" is how they drift.
    /// Concretely: tracking a tab must no longer make a browserless profile
    /// report itself active.
    #[tokio::test]
    async fn a_tracked_tab_no_longer_fakes_a_live_managed_session() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());
        let manager = ProfileManager::new(BrowserSystemConfig::default());
        manager.touch_tab("default", "tab-1");
        assert!(
            manager.has_tracked_tabs("default"),
            "precondition: the registry did record the tab"
        );
        assert!(
            !manager.session_active("default"),
            "no chromium was ever launched, so the profile is not active"
        );
    }

    /// The reaper's Managed arm has two halves now, and the second one is the
    /// point: under `attach`, `playwright-cli close` only DISCONNECTS (measured
    /// — nine Chrome processes before and after). A reaper that stopped at
    /// `close` would report a reaped profile and leave the browser running
    /// forever. With no browser to begin with, the sweep must be a no-op and
    /// must not invent one.
    #[tokio::test]
    async fn the_reaper_does_not_launch_a_browser_in_order_to_reap_one() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());
        let manager = ProfileManager::new(managed_config(None));
        // Past its timeout, so the sweep's answer is about the browser rather
        // than about the clock.
        backdate(&manager, "default", Duration::from_secs(2));
        assert_eq!(manager.reap_idle().await, 0);
        assert!(manager.live_endpoint("default").is_none());
    }

    /// spec §3.6 「退出时杀」. `std::process::Child` does NOT kill on drop, and
    /// under `attach --cdp` the CLI was never the browser's parent — so without
    /// an explicit stop every restart leaves a browser behind until the next
    /// boot sweep finds it.
    ///
    /// The fake browser is a real `sleep` subprocess, because the thing being
    /// tested is that a live pid stops being live. A mock child would assert
    /// that a method was called (判据 §4: assert the effect arrived, not that
    /// the call happened).
    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_browsers_kills_what_it_launched_and_says_how_many() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());
        let manager = ProfileManager::new(BrowserSystemConfig::default());

        // Nothing launched → nothing to stop, and it must not pretend otherwise.
        assert_eq!(manager.shutdown_browsers(), 0);

        // A stand-in browser: long-lived, harmless, and observable by pid.
        let child = std::process::Command::new("sleep")
            .arg("120")
            .spawn()
            .expect("spawn the stand-in browser");
        let pid = child.id();
        manager.insert_test_child("default", child);
        assert!(
            crate::utils::process_alive::is_process_alive(pid as i32),
            "precondition: the stand-in is running"
        );

        assert_eq!(manager.shutdown_browsers(), 1);
        assert!(
            !crate::utils::process_alive::is_process_alive(pid as i32),
            "the stand-in browser is still running after shutdown_browsers"
        );
        // Idempotent: a second stop finds nothing and says so.
        assert_eq!(manager.shutdown_browsers(), 0);
    }

    /// The central behavioural claim of the launch-chain flip, asserted in the
    /// only direction a unit test can speak to it: **Aleph's own reclamation
    /// does not come from `close`.**
    ///
    /// The CLI here is a script that succeeds at everything, so the reaper's
    /// first half runs to completion — and it records whether the browser was
    /// still alive when it ran. Both facts together are the order: `close`
    /// happened over a live browser (so the reaper did not kill first, which
    /// would drop the CLI session's own teardown), and the browser is gone
    /// afterwards (so something other than `close` reclaimed it).
    ///
    /// That the *external* `playwright-cli close` leaves the browser running
    /// was measured in the spike (nine Chrome processes before and after) and
    /// is not a property a unit test can hold; what it can hold is that Aleph
    /// no longer depends on it doing anything.
    #[cfg(unix)]
    #[tokio::test]
    async fn reap_idle_closes_the_cli_session_then_kills_the_browser_close_left_running() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());
        let tmp = tempfile::tempdir().expect("tempdir");

        let child = std::process::Command::new("sleep")
            .arg("120")
            .spawn()
            .expect("spawn the stand-in browser");
        let pid = child.id();
        let (cli, close_saw_live_browser) = fake_cli_recording_close(tmp.path(), pid);

        let manager = ProfileManager::new(managed_config(Some(&cli)));
        manager.insert_test_child("default", child);
        backdate(&manager, "default", Duration::from_secs(2));
        assert_eq!(
            manager.idle_managed_profiles(),
            vec!["default".to_string()],
            "precondition: an idle profile WITH a browser is a reap candidate"
        );

        assert_eq!(
            manager.reap_idle().await,
            1,
            "the sweep reclaimed a browser and must say so"
        );
        assert!(
            close_saw_live_browser.exists(),
            "the cli `close` either did not run or ran after the kill"
        );
        assert!(
            !crate::utils::process_alive::is_process_alive(pid as i32),
            "`close` disconnects; only the kill reclaims — the browser is still running"
        );
        assert!(
            manager.live_endpoint("default").is_none(),
            "the reaped profile must not still advertise an endpoint"
        );
    }

    /// A Managed browser that exited on its own is still the reaper's to clean
    /// up — the flip to `chromium_alive` must not turn "died" into "nothing to
    /// do here".
    ///
    /// Three different things outlive that browser and only this sweep clears
    /// them while the daemon runs: the child record in the driver's map, the
    /// sidecar file that names its pid on disk, and the profile's tab entries.
    /// `chromium_died` exists as a concept distinct from "no browser" (Task 5)
    /// for OTHER call sites (`run`'s pre-verb check reads it alone) — but the
    /// candidate filter here admits both `chromium_alive` and `chromium_died`
    /// profiles for a simpler reason than recognising two facts: `chromium_died`
    /// is `key_present && !alive`, so the disjunction is exactly "Aleph has a
    /// record for this profile at all", which is what "something to reclaim"
    /// actually requires.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_browser_that_died_on_its_own_is_still_reclaimed() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());
        let manager = ProfileManager::new(managed_config(None));

        // A browser that is already gone: spawned, then reaped, so `try_wait`
        // has a definite answer rather than a racy one.
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawn the stand-in browser");
        child
            .wait()
            .expect("the stand-in browser exits immediately");
        manager.insert_test_child("default", child);
        manager.touch_tab("default", "tab-1");
        backdate(&manager, "default", Duration::from_secs(2));

        assert!(
            !manager.session_active("default"),
            "precondition: the browser really is gone"
        );
        assert!(
            manager.has_tracked_tabs("default"),
            "precondition: the registry has something to clear"
        );

        assert_eq!(
            manager.reap_idle().await,
            1,
            "a dead browser's record, sidecar and tabs are still state to reclaim"
        );
        assert!(manager.live_endpoint("default").is_none());
        assert!(
            !manager.has_tracked_tabs("default"),
            "the tab entries survived the sweep that was supposed to clear them"
        );
    }

    /// A failed `close` is not a reason to keep a browser.
    ///
    /// The CLI is unavailable here (the sealed test twin refuses to install
    /// one), so the first half of the reaper's Managed arm cannot even run. The
    /// browser is still Aleph's child, and the CLI's opinion of its own session
    /// says nothing about it — treating that `Err` as a reason to skip the kill
    /// would make a broken CLI, the state most likely to have leaked a browser,
    /// the one state where Aleph refuses to reclaim it (判据 §8).
    #[cfg(unix)]
    #[tokio::test]
    async fn a_close_that_could_not_run_does_not_spare_the_browser() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());

        let manager = ProfileManager::new(managed_config(None));
        let child = std::process::Command::new("sleep")
            .arg("120")
            .spawn()
            .expect("spawn the stand-in browser");
        let pid = child.id();
        manager.insert_test_child("default", child);
        backdate(&manager, "default", Duration::from_secs(2));

        assert_eq!(manager.reap_idle().await, 1);
        assert!(
            !crate::utils::process_alive::is_process_alive(pid as i32),
            "a browser survived a sweep because the cli could not be run"
        );
    }

    #[test]
    fn test_get_backend_nonexistent_profile() {
        let config = BrowserSystemConfig::default();
        let manager = ProfileManager::new(config);
        let backend = manager.get_backend("nonexistent");
        assert!(matches!(backend, Err(BrowserError::ProfileNotFound(_))));
    }
}
