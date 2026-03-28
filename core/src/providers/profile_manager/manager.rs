//! AuthProfileManager implementation

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use chrono::Utc;
use tracing::{debug, info, warn};

use crate::sync_primitives::{Arc, RwLock};

use super::{
    AgentState, EffectiveProfile, ProfileInfo, ProfileManagerError, ProfileManagerResult,
    RuntimeStatus,
};
use crate::providers::auth_profiles::{calculate_cooldown_ms, AuthProfileFailureReason};
use crate::providers::profile_config::ProfilesConfig;

// ============================================================================
// Auth Profile Manager
// ============================================================================

/// Auth profile manager with hybrid storage
pub struct AuthProfileManager {
    /// Profile configurations (from profiles.toml)
    configs: Arc<RwLock<ProfilesConfig>>,

    /// Runtime status (in-memory, not persisted)
    status: Arc<RwLock<HashMap<String, RuntimeStatus>>>,

    /// Path to profiles.toml
    config_path: PathBuf,

    /// Base directory for agent state (~/.aleph/workspaces)
    agents_dir: PathBuf,

    /// Cached agent states
    agent_states: Arc<RwLock<HashMap<String, AgentState>>>,
}

impl AuthProfileManager {
    /// Create a new manager with default paths
    pub fn new() -> ProfileManagerResult<Self> {
        let config_path = ProfilesConfig::default_path();
        let agents_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".aleph")
            .join("workspaces");

        Self::with_paths(config_path, agents_dir)
    }

    /// Create a new manager with custom paths
    pub fn with_paths(config_path: PathBuf, agents_dir: PathBuf) -> ProfileManagerResult<Self> {
        let configs = if config_path.exists() {
            ProfilesConfig::load(&config_path)?
        } else {
            ProfilesConfig::new()
        };

        info!(
            config_path = %config_path.display(),
            profile_count = configs.profiles.len(),
            "AuthProfileManager initialized"
        );

        Ok(Self {
            configs: Arc::new(RwLock::new(configs)),
            status: Arc::new(RwLock::new(HashMap::new())),
            config_path,
            agents_dir,
            agent_states: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Reload configuration from disk
    pub fn reload_config(&self) -> ProfileManagerResult<()> {
        if !self.config_path.exists() {
            warn!(
                path = %self.config_path.display(),
                "Config file does not exist, skipping reload"
            );
            return Ok(());
        }

        let new_configs = ProfilesConfig::load(&self.config_path)?;
        let mut configs = self.configs.write().unwrap_or_else(|e| e.into_inner());
        *configs = new_configs;

        info!(
            profile_count = configs.profiles.len(),
            "Reloaded profile configuration"
        );

        Ok(())
    }

    /// Get an available profile for a provider (considering cooldowns and budget)
    pub fn get_available_profile(
        &self,
        provider: &str,
        agent_id: &str,
    ) -> ProfileManagerResult<EffectiveProfile> {
        let configs = self.configs.read().unwrap_or_else(|e| e.into_inner());
        let status_map = self.status.read().unwrap_or_else(|e| e.into_inner());

        // Get profiles for this provider sorted by tier
        let profiles = configs.profiles_for_provider(provider);

        if profiles.is_empty() {
            return Err(ProfileManagerError::NoProfilesAvailable(provider.to_string()));
        }

        // Load agent state for budget checking
        let agent_state = self.load_agent_state(agent_id)?;

        // Find first available profile
        let mut all_in_cooldown = true;
        let mut best_cooldown_profile: Option<(&String, &crate::providers::profile_config::ProfileConfig, u64)> = None;

        for (profile_id, config) in profiles {
            // Skip if disabled in agent state
            if agent_state.is_profile_disabled(profile_id) {
                debug!(profile_id = %profile_id, "Profile disabled for agent");
                continue;
            }

            // Check budget
            if agent_state.exceeds_budget(profile_id) {
                let usage = agent_state.get_usage(profile_id);
                let budget = agent_state.get_override(profile_id)
                    .and_then(|o| o.max_budget_usd)
                    .unwrap_or(0.0);
                let used = usage.map(|u| u.total_cost_usd).unwrap_or(0.0);
                debug!(
                    profile_id = %profile_id,
                    budget = %budget,
                    used = %used,
                    "Profile budget exceeded"
                );
                continue;
            }

            // Check cooldown
            let status = status_map.get(profile_id);
            let in_cooldown = status.is_some_and(|s| s.is_in_cooldown());

            if !in_cooldown {
                all_in_cooldown = false;
                // Found available profile - try to resolve API key
                match EffectiveProfile::from_config(profile_id.clone(), config) {
                    Ok(effective) => {
                        debug!(
                            profile_id = %profile_id,
                            provider = %provider,
                            tier = ?config.tier,
                            "Selected available profile"
                        );
                        return Ok(effective);
                    }
                    Err(e) => {
                        warn!(
                            profile_id = %profile_id,
                            error = %e,
                            "Failed to resolve profile API key"
                        );
                        continue;
                    }
                }
            } else {
                // Track profile with shortest cooldown remaining
                let remaining = status.and_then(|s| s.cooldown_remaining_ms()).unwrap_or(u64::MAX);
                if best_cooldown_profile.is_none()
                    || remaining < best_cooldown_profile.as_ref().unwrap().2
                {
                    best_cooldown_profile = Some((profile_id, config, remaining));
                }
            }
        }

        // All profiles are in cooldown
        if all_in_cooldown {
            // Return the profile with shortest cooldown if available
            if let Some((profile_id, config, remaining_ms)) = best_cooldown_profile {
                warn!(
                    provider = %provider,
                    profile_id = %profile_id,
                    cooldown_remaining_ms = remaining_ms,
                    "All profiles in cooldown, returning profile with shortest cooldown"
                );
                // Still try to return it - caller can wait or handle as needed
                if let Ok(effective) = EffectiveProfile::from_config(profile_id.clone(), config) {
                    return Ok(effective);
                }
            }
            return Err(ProfileManagerError::AllProfilesInCooldown(provider.to_string()));
        }

        Err(ProfileManagerError::NoProfilesAvailable(provider.to_string()))
    }

    /// Mark a profile as failed (triggers cooldown)
    pub fn mark_failure(
        &self,
        profile_id: &str,
        reason: AuthProfileFailureReason,
    ) -> ProfileManagerResult<()> {
        let mut status_map = self.status.write().unwrap_or_else(|e| e.into_inner());
        let status = status_map
            .entry(profile_id.to_string())
            .or_default();

        status.failure_count += 1;
        status.last_failure_reason = Some(reason);
        status.is_rate_limited = reason == AuthProfileFailureReason::RateLimit;

        // Calculate cooldown using the algorithm from auth_profiles
        let cooldown_ms = calculate_cooldown_ms(status.failure_count);
        let cooldown_duration = std::time::Duration::from_millis(cooldown_ms);
        status.cooldown_until = Some(Instant::now() + cooldown_duration);

        warn!(
            profile_id = %profile_id,
            reason = ?reason,
            failure_count = status.failure_count,
            cooldown_ms = cooldown_ms,
            "Profile marked as failed"
        );

        Ok(())
    }

    /// Mark a profile as successful (resets failure count)
    pub fn mark_success(&self, profile_id: &str) -> ProfileManagerResult<()> {
        let mut status_map = self.status.write().unwrap_or_else(|e| e.into_inner());
        let status = status_map
            .entry(profile_id.to_string())
            .or_default();

        status.failure_count = 0;
        status.is_rate_limited = false;
        status.cooldown_until = None;
        status.last_failure_reason = None;

        debug!(profile_id = %profile_id, "Profile marked as successful");

        Ok(())
    }

    /// Record usage for a profile in an agent's state
    pub fn record_usage(
        &self,
        agent_id: &str,
        profile_id: &str,
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: f64,
    ) -> ProfileManagerResult<()> {
        let mut agent_states = self.agent_states.write().unwrap_or_else(|e| e.into_inner());

        // Load or get cached state
        let state = agent_states
            .entry(agent_id.to_string())
            .or_insert_with(|| {
                let path = self.agent_state_path(agent_id);
                AgentState::load(&path).unwrap_or_default()
            });

        // Update usage
        let usage = state.get_or_create_usage(profile_id);
        usage.input_tokens += input_tokens;
        usage.output_tokens += output_tokens;
        usage.total_cost_usd += cost_usd;
        usage.request_count += 1;
        usage.last_used_at = Some(Utc::now());

        // Save state
        let path = self.agent_state_path(agent_id);
        state.save(&path)?;

        debug!(
            agent_id = %agent_id,
            profile_id = %profile_id,
            input_tokens = input_tokens,
            output_tokens = output_tokens,
            cost_usd = cost_usd,
            "Recorded profile usage"
        );

        Ok(())
    }

    /// List all profiles with their current status
    pub fn list_profiles(&self) -> Vec<ProfileInfo> {
        let configs = self.configs.read().unwrap_or_else(|e| e.into_inner());
        let status_map = self.status.read().unwrap_or_else(|e| e.into_inner());

        configs
            .profiles
            .iter()
            .map(|(id, config)| {
                let status = status_map.get(id);
                let in_cooldown = status.is_some_and(|s| s.is_in_cooldown());
                let cooldown_remaining_ms = status.and_then(|s| s.cooldown_remaining_ms());
                let key_resolvable = config.resolve_api_key().is_ok();

                ProfileInfo {
                    id: id.clone(),
                    provider: config.provider.clone(),
                    tier: config.tier,
                    in_cooldown,
                    cooldown_remaining_ms,
                    disabled: config.disabled,
                    failure_count: status.map(|s| s.failure_count).unwrap_or(0),
                    last_failure_reason: status.and_then(|s| s.last_failure_reason),
                    uses_env_var: config.uses_env_var(),
                    key_resolvable,
                }
            })
            .collect()
    }

    /// Get profiles for a specific provider
    pub fn profiles_for_provider(&self, provider: &str) -> Vec<ProfileInfo> {
        self.list_profiles()
            .into_iter()
            .filter(|p| p.provider.to_lowercase() == provider.to_lowercase())
            .collect()
    }

    /// Get profile count
    pub fn profile_count(&self) -> usize {
        self.configs.read().unwrap_or_else(|e| e.into_inner()).profiles.len()
    }

    /// Get agent state path
    fn agent_state_path(&self, agent_id: &str) -> PathBuf {
        self.agents_dir.join(agent_id).join("state.json")
    }

    /// Load agent state (with caching)
    fn load_agent_state(&self, agent_id: &str) -> ProfileManagerResult<AgentState> {
        // Use write lock with entry API to avoid TOCTOU race between read and write
        let mut agent_states = self.agent_states.write().unwrap_or_else(|e| e.into_inner());
        let state = match agent_states.entry(agent_id.to_string()) {
            std::collections::hash_map::Entry::Occupied(e) => e.get().clone(),
            std::collections::hash_map::Entry::Vacant(e) => {
                let path = self.agent_state_path(agent_id);
                let loaded = AgentState::load(&path)?;
                e.insert(loaded.clone());
                loaded
            }
        };
        Ok(state)
    }

    /// Clear cooldown for a profile
    pub fn clear_cooldown(&self, profile_id: &str) -> ProfileManagerResult<()> {
        let mut status_map = self.status.write().unwrap_or_else(|e| e.into_inner());
        if let Some(status) = status_map.get_mut(profile_id) {
            status.cooldown_until = None;
            status.is_rate_limited = false;
            debug!(profile_id = %profile_id, "Cleared cooldown");
        }
        Ok(())
    }

    /// Set a budget override for a profile in an agent
    pub fn set_budget_override(
        &self,
        agent_id: &str,
        profile_id: &str,
        max_budget_usd: Option<f64>,
    ) -> ProfileManagerResult<()> {
        let mut agent_states = self.agent_states.write().unwrap_or_else(|e| e.into_inner());

        let state = agent_states
            .entry(agent_id.to_string())
            .or_insert_with(|| {
                let path = self.agent_state_path(agent_id);
                AgentState::load(&path).unwrap_or_default()
            });

        let override_ = state.overrides
            .entry(profile_id.to_string())
            .or_default();
        override_.max_budget_usd = max_budget_usd;

        let path = self.agent_state_path(agent_id);
        state.save(&path)?;

        info!(
            agent_id = %agent_id,
            profile_id = %profile_id,
            max_budget_usd = ?max_budget_usd,
            "Set budget override"
        );

        Ok(())
    }

    /// Disable a profile for an agent
    pub fn disable_profile_for_agent(
        &self,
        agent_id: &str,
        profile_id: &str,
        disabled: bool,
    ) -> ProfileManagerResult<()> {
        let mut agent_states = self.agent_states.write().unwrap_or_else(|e| e.into_inner());

        let state = agent_states
            .entry(agent_id.to_string())
            .or_insert_with(|| {
                let path = self.agent_state_path(agent_id);
                AgentState::load(&path).unwrap_or_default()
            });

        let override_ = state.overrides
            .entry(profile_id.to_string())
            .or_default();
        override_.disabled = disabled;

        let path = self.agent_state_path(agent_id);
        state.save(&path)?;

        info!(
            agent_id = %agent_id,
            profile_id = %profile_id,
            disabled = disabled,
            "Profile disable state changed"
        );

        Ok(())
    }

    /// Get reference to configs (for testing)
    pub fn configs(&self) -> &Arc<RwLock<ProfilesConfig>> {
        &self.configs
    }
}
