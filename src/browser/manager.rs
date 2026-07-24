// Browser profile lifecycle manager.
// Manages profile instances: registration, state tracking, idle reclamation.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::sync_primitives::{AtomicBool, Ordering, RwLock};

use super::backend::BrowserBackend;
use super::chrome_mcp::ChromeMcpDriver;
use super::chrome_mcp_backend::ChromeMcpBackend;
use super::error::BrowserError;
use super::network_policy::{BrowserSsrfGuard, PolicyViolation};
use super::playwright_cli::PlaywrightCliDriver;
use super::playwright_cli_backend::PlaywrightCliBackend;
use super::profile::{
    BrowserDriver, BrowserSystemConfig, BrowserType, ProfileConfig, ProfileState,
};
use super::tab_registry::{parse_tab_ids, TabRegistry};

/// Manages the lifecycle of browser profiles.
pub struct ProfileManager {
    profiles: RwLock<HashMap<String, ManagedProfile>>,
    ssrf_guard: Arc<BrowserSsrfGuard>,
    config: BrowserSystemConfig,
    chrome_mcp_driver: Arc<ChromeMcpDriver>,
    playwright_cli_driver: Arc<PlaywrightCliDriver>,
    idle_reaper_started: AtomicBool,
    /// Per-tab lifecycle tracking for Managed profiles (idle reclamation + cap).
    tab_registry: TabRegistry,
}

struct ManagedProfile {
    config: ProfileConfig,
    state: ProfileState,
    last_activity: std::time::Instant,
}

impl ProfileManager {
    #[must_use]
    pub fn new(config: BrowserSystemConfig) -> Self {
        let ssrf_guard = Arc::new(BrowserSsrfGuard::new(config.policy.clone()));
        let chrome_mcp_driver = Arc::new(ChromeMcpDriver::new(config.chrome_mcp.clone()));
        let playwright_cli_driver =
            Arc::new(PlaywrightCliDriver::new(config.playwright_cli.clone()));

        let mut profiles = HashMap::new();

        if config.profiles.is_empty() {
            // Create default profile with Managed driver if none configured.
            profiles.insert(
                "default".into(),
                ManagedProfile {
                    config: ProfileConfig::default(),
                    state: ProfileState::Idle,
                    last_activity: std::time::Instant::now(),
                },
            );
        } else {
            for (name, profile_config) in &config.profiles {
                profiles.insert(
                    name.clone(),
                    ManagedProfile {
                        config: profile_config.clone(),
                        state: ProfileState::Idle,
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
                        color: Some("#00AA00".into()),
                        ..Default::default()
                    },
                    state: ProfileState::Idle,
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
                        color: Some("#00AA00".into()),
                        ..Default::default()
                    },
                    state: ProfileState::Idle,
                    last_activity: std::time::Instant::now(),
                },
            );
        }

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

    /// Spawn the idle-profile reaper on a background tokio task, at most once
    /// per `ProfileManager` instance. The reaper sweeps every `interval_secs`,
    /// tears down Chrome MCP sessions whose profile is past its idle timeout,
    /// and resets state to `Idle`. Idempotent — subsequent calls are no-ops.
    pub fn spawn_idle_reaper(self: &Arc<Self>, interval_secs: u64) {
        if self.idle_reaper_started.swap(true, Ordering::AcqRel) {
            return;
        }
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

    /// Get the shared Chrome MCP driver instance.
    pub fn get_chrome_mcp_driver(&self) -> Arc<ChromeMcpDriver> {
        self.chrome_mcp_driver.clone()
    }

    /// Get the shared Playwright CLI driver instance.
    pub fn get_playwright_cli_driver(&self) -> Arc<PlaywrightCliDriver> {
        self.playwright_cli_driver.clone()
    }

    /// Get the shared SSRF guard (Arc-wrapped for cheap cloning).
    pub fn get_ssrf_guard(&self) -> Arc<BrowserSsrfGuard> {
        self.ssrf_guard.clone()
    }

    /// Route a profile to its appropriate `BrowserBackend` instance.
    ///
    /// - `BrowserDriver::Managed`         → `PlaywrightCliBackend`
    /// - `BrowserDriver::ExistingSession` → `ChromeMcpBackend`
    pub fn get_backend(&self, profile_name: &str) -> Result<Arc<dyn BrowserBackend>, BrowserError> {
        let cfg = self
            .get_config(profile_name)
            .ok_or_else(|| BrowserError::ProfileNotFound(profile_name.into()))?;
        if matches!(self.get_state(profile_name), Some(ProfileState::Stopping)) {
            return Err(BrowserError::ProfileBusy(profile_name.into()));
        }
        match cfg.driver {
            BrowserDriver::Managed => {
                let headless = cfg.headless.unwrap_or(self.config.playwright_cli.headless);
                Ok(Arc::new(PlaywrightCliBackend::new(
                    self.playwright_cli_driver.clone(),
                    profile_name.to_string(),
                    self.ssrf_guard.clone(),
                    headless,
                )))
            }
            BrowserDriver::ExistingSession => Ok(Arc::new(ChromeMcpBackend::new(
                self.chrome_mcp_driver.clone(),
                profile_name.to_string(),
                self.ssrf_guard.clone(),
            ))),
        }
    }

    /// Resolve the effective headless flag for a profile: profile-level override
    /// falls back to the global `playwright_cli.headless` default.
    pub fn resolve_headless(&self, profile_name: &str) -> bool {
        self.get_config(profile_name)
            .and_then(|c| c.headless)
            .unwrap_or(self.config.playwright_cli.headless)
    }

    /// Sweep idle profiles: tear down Chrome MCP sessions for `ExistingSession`
    /// profiles past their `idle_timeout_secs`, then reset state to `Idle`.
    /// Returns the number of profiles reaped (best-effort; safe to call any time).
    pub async fn reap_idle(&self) -> usize {
        let idle = self.idle_profiles();
        let mut reaped = 0;
        for name in idle {
            if let Some(BrowserDriver::ExistingSession) = self.get_driver(&name) {
                self.chrome_mcp_driver.destroy_session(&name).await;
            }
            // Atomically check-and-set state inside a single write lock.
            let mut profiles = self.profiles.write().unwrap_or_else(|e| e.into_inner());
            if let Some(profile) = profiles.get_mut(&name) {
                if profile.state.is_running() {
                    profile.state = ProfileState::Idle;
                    reaped += 1;
                }
            }
        }
        reaped
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

    /// List all profiles with their current state.
    pub fn list_profiles(&self) -> Vec<(String, ProfileState)> {
        let profiles = self.profiles.read().unwrap_or_else(|e| e.into_inner());
        profiles
            .iter()
            .map(|(name, p)| (name.clone(), p.state.clone()))
            .collect()
    }

    /// Get the current state of a named profile.
    pub fn get_state(&self, name: &str) -> Option<ProfileState> {
        let profiles = self.profiles.read().unwrap_or_else(|e| e.into_inner());
        profiles.get(name).map(|p| p.state.clone())
    }

    /// Get the configuration of a named profile.
    pub fn get_config(&self, name: &str) -> Option<ProfileConfig> {
        let profiles = self.profiles.read().unwrap_or_else(|e| e.into_inner());
        profiles.get(name).map(|p| p.config.clone())
    }

    /// Validate a URL against the SSRF policy.
    pub async fn check_url(&self, url: &str) -> Result<(), PolicyViolation> {
        self.ssrf_guard.check_url(url).await
    }

    /// Validate an agent-initiated navigation target: SSRF policy plus
    /// secret-exfiltration scanning. Use this for `goto`/`open`.
    pub async fn check_navigation(&self, url: &str) -> Result<(), PolicyViolation> {
        self.ssrf_guard.check_navigation(url).await
    }

    /// Redact embedded credentials from page-derived `text` before it is
    /// returned to the LLM (the OUT half of the secret-egress boundary). Used by
    /// the content-read tools (snapshot / console / network / evaluate) via the
    /// shared `redact_and_wrap` egress chokepoint. Zero-copy when redaction is
    /// disabled or the text carries no secret.
    pub fn redact_content<'a>(&self, text: &'a str) -> std::borrow::Cow<'a, str> {
        self.ssrf_guard.redact_content(text)
    }

    /// Record activity on a profile to reset its idle timer.
    pub fn record_activity(&self, profile_name: &str) {
        let mut profiles = self.profiles.write().unwrap_or_else(|e| e.into_inner());
        if let Some(profile) = profiles.get_mut(profile_name) {
            profile.last_activity = std::time::Instant::now();
        }
    }

    /// Update the state of a named profile.
    pub fn set_state(&self, profile_name: &str, state: ProfileState) {
        let mut profiles = self.profiles.write().unwrap_or_else(|e| e.into_inner());
        if let Some(profile) = profiles.get_mut(profile_name) {
            profile.state = state;
        }
    }

    /// Test-only: whether any tabs are tracked for a profile.
    #[cfg(test)]
    pub(crate) fn has_tracked_tabs(&self, profile: &str) -> bool {
        self.tab_registry.has_tabs(profile)
    }

    /// Returns profiles that have been idle longer than their configured timeout.
    pub fn idle_profiles(&self) -> Vec<String> {
        let profiles = self.profiles.read().unwrap_or_else(|e| e.into_inner());
        profiles
            .iter()
            .filter(|(_, p)| {
                p.state.is_running()
                    && p.last_activity.elapsed().as_secs() > p.config.idle_timeout_secs
            })
            .map(|(name, _)| name.clone())
            .collect()
    }
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
                cdp_port: 18801,
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
    fn test_get_profile_state() {
        let config = BrowserSystemConfig::default();
        let manager = ProfileManager::new(config);
        let state = manager.get_state("default");
        assert_eq!(state, Some(ProfileState::Idle));
    }

    #[test]
    fn test_profile_state_transitions() {
        let config = BrowserSystemConfig::default();
        let manager = ProfileManager::new(config);

        // Initially Idle
        assert_eq!(manager.get_state("default"), Some(ProfileState::Idle));

        // Can transition to Running
        manager.set_state(
            "default",
            ProfileState::Running {
                pid: 1234,
                port: 18800,
            },
        );
        assert_eq!(
            manager.get_state("default"),
            Some(ProfileState::Running {
                pid: 1234,
                port: 18800,
            })
        );

        // Activity recording doesn't error
        manager.record_activity("default");

        // No idle profiles (just recorded activity)
        assert!(manager.idle_profiles().is_empty());

        // Back to Idle
        manager.set_state("default", ProfileState::Idle);
        assert_eq!(manager.get_state("default"), Some(ProfileState::Idle));
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
        assert_eq!(user_config.color.as_deref(), Some("#00AA00"));
    }

    #[test]
    fn test_explicit_user_profile_not_overridden() {
        let mut config = BrowserSystemConfig::default();
        config.profiles.insert(
            "user".into(),
            ProfileConfig {
                browser: BrowserType::Chrome,
                driver: BrowserDriver::ExistingSession,
                color: Some("#FF0000".into()),
                ..Default::default()
            },
        );
        let manager = ProfileManager::new(config);
        let user_config = manager.get_config("user").unwrap();
        assert_eq!(user_config.color.as_deref(), Some("#FF0000"));
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

    #[test]
    fn test_get_backend_nonexistent_profile() {
        let config = BrowserSystemConfig::default();
        let manager = ProfileManager::new(config);
        let backend = manager.get_backend("nonexistent");
        assert!(matches!(backend, Err(BrowserError::ProfileNotFound(_))));
    }
}
