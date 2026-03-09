// Browser profile lifecycle manager.
// Manages profile instances: registration, state tracking, idle reclamation.

use std::collections::HashMap;
use std::sync::RwLock;

use super::network_policy::{PolicyViolation, SsrfPolicy};
use super::profile::{BrowserSystemConfig, ProfileConfig, ProfileState};

/// Manages the lifecycle of browser profiles.
pub struct ProfileManager {
    profiles: RwLock<HashMap<String, ManagedProfile>>,
    ssrf_policy: SsrfPolicy,
    #[allow(dead_code)]
    config: BrowserSystemConfig,
}

struct ManagedProfile {
    config: ProfileConfig,
    state: ProfileState,
    last_activity: std::time::Instant,
}

impl ProfileManager {
    pub fn new(config: BrowserSystemConfig) -> Self {
        let ssrf_policy = SsrfPolicy::new(config.policy.clone());

        let mut profiles = HashMap::new();

        if config.profiles.is_empty() {
            // Create default profile if none configured
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

        Self {
            profiles: RwLock::new(profiles),
            ssrf_policy,
            config,
        }
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
        assert_eq!(profiles.len(), 2);
        assert!(profiles.iter().any(|p| p.0 == "default"));
        assert!(profiles.iter().any(|p| p.0 == "work"));
    }

    #[test]
    fn test_manager_default_profile_if_none_configured() {
        let config = BrowserSystemConfig::default();
        let manager = ProfileManager::new(config);
        let profiles = manager.list_profiles();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].0, "default");
    }

    #[test]
    fn test_get_profile_state() {
        let config = BrowserSystemConfig::default();
        let manager = ProfileManager::new(config);
        let state = manager.get_state("default");
        assert_eq!(state, Some(ProfileState::Idle));
    }
}
