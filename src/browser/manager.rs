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
    /// - `Managed` → `playwright-cli close`. This used to be documented as
    ///   impossible ("the Playwright CLI exposes no stop-this-session
    ///   command"), which is false: `close`, `close-all` and `kill-all` are all
    ///   there. `idle_timeout_secs` was therefore accepted and never enforced
    ///   for managed profiles.
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
                // Already gone is the same outcome as just-closed, and the
                // registry must be cleared either way or the profile stays a
                // reap candidate forever.
                Ok(_) | Err(BrowserError::NoSession(_)) => {}
                Err(e) => {
                    tracing::warn!(profile = %name, error = %e, "reap_idle: failed to close managed session");
                    continue;
                }
            }
            self.tab_registry.clear_profile(&name);
            reaped += 1;
        }
        reaped
    }

    /// `Managed` profiles idle past their timeout that plausibly still have a
    /// browser.
    ///
    /// "Plausibly" is [`TabRegistry::has_tabs`] — the same approximation
    /// [`Self::session_active`] uses, and the reason the close tolerates a
    /// `NoSession` answer instead of trusting this predicate.
    fn idle_managed_profiles(&self) -> Vec<String> {
        let profiles = self.profiles.read().unwrap_or_else(|e| e.into_inner());
        let now = std::time::Instant::now();
        profiles
            .iter()
            .filter(|(name, p)| {
                p.config.driver == BrowserDriver::Managed
                    && is_idle(p.last_activity, now, p.config.idle_timeout_secs)
                    && self.tab_registry.has_tabs(name)
            })
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Whether the profile currently has a live browser session, derived from
    /// the real session-tracking surfaces rather than a state flag:
    /// - `ExistingSession` → a Chrome MCP session exists in the driver.
    /// - `Managed` → the tab registry has tracked tabs for the profile.
    ///   Approximation: a managed session exists if tabs were used; the
    ///   registry is reconciled against the live browser on each reaper sweep.
    pub fn session_active(&self, name: &str) -> bool {
        match self.get_driver(name) {
            Some(BrowserDriver::ExistingSession) => self.chrome_mcp_driver.has_session(name),
            Some(BrowserDriver::Managed) => self.tab_registry.has_tabs(name),
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

        // Managed approximation: tracked tabs imply a live session.
        manager.touch_tab("default", "1");
        assert!(manager.session_active("default"));
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

    #[test]
    fn test_get_backend_nonexistent_profile() {
        let config = BrowserSystemConfig::default();
        let manager = ProfileManager::new(config);
        let backend = manager.get_backend("nonexistent");
        assert!(matches!(backend, Err(BrowserError::ProfileNotFound(_))));
    }
}
