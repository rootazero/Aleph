// Browser profile lifecycle manager.
// Manages profile instances: registration, state tracking, idle reclamation.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use super::chrome_mcp::ChromeMcpDriver;
use super::network_policy::{PolicyViolation, SsrfPolicy};
use super::playwright_mcp::PlaywrightMcpDriver;
use super::profile::{
    BrowserDriver, BrowserSystemConfig, BrowserType, ProfileConfig, ProfileState,
};

/// Manages the lifecycle of browser profiles.
pub struct ProfileManager {
    profiles: RwLock<HashMap<String, ManagedProfile>>,
    ssrf_policy: SsrfPolicy,
    #[allow(dead_code)]
    config: BrowserSystemConfig,
    chrome_mcp_driver: Arc<ChromeMcpDriver>,
    playwright_mcp_driver: Arc<PlaywrightMcpDriver>,
}

struct ManagedProfile {
    config: ProfileConfig,
    state: ProfileState,
    last_activity: std::time::Instant,
}

impl ProfileManager {
    pub fn new(config: BrowserSystemConfig) -> Self {
        let ssrf_policy = SsrfPolicy::new(config.policy.clone());
        let chrome_mcp_driver = Arc::new(ChromeMcpDriver::new(config.chrome_mcp.clone()));
        let playwright_mcp_driver =
            Arc::new(PlaywrightMcpDriver::new(config.playwright_mcp.clone()));

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
            ssrf_policy,
            config,
            chrome_mcp_driver,
            playwright_mcp_driver,
        }
    }

    /// Get the shared Chrome MCP driver instance.
    pub fn get_chrome_mcp_driver(&self) -> Arc<ChromeMcpDriver> {
        self.chrome_mcp_driver.clone()
    }

    /// Get the shared Playwright MCP driver instance.
    pub fn get_playwright_mcp_driver(&self) -> Arc<PlaywrightMcpDriver> {
        self.playwright_mcp_driver.clone()
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
    pub fn check_url(&self, url: &str) -> Result<(), PolicyViolation> {
        self.ssrf_policy.check_url(url)
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
}
