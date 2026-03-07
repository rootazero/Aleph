//! Auth Profile Manager with Hybrid Storage
//!
//! Manages auth profiles with a three-tier storage architecture:
//!
//! 1. **Global Config** (~/.aleph/profiles.toml) - User-maintained, TOML format
//! 2. **Runtime State** (memory) - Cooldowns, not persisted across restarts
//! 3. **Per-Agent State** (~/.aleph/workspaces/{id}/state.json) - Usage tracking, persisted
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    AuthProfileManager                            │
//! ├─────────────────────────────────────────────────────────────────┤
//! │  ┌────────────────┐ ┌────────────────┐ ┌────────────────────┐   │
//! │  │ profiles.toml  │ │ Runtime State  │ │ agents/{id}/state  │   │
//! │  │   (global)     │ │   (memory)     │ │    (per-agent)     │   │
//! │  │                │ │                │ │                    │   │
//! │  │ • provider     │ │ • is_rate_ltd  │ │ • usage: tokens    │   │
//! │  │ • api_key      │ │ • cooldown_at  │ │ • usage: cost_usd  │   │
//! │  │ • base_url     │ │ • fail_count   │ │ • usage: last_used │   │
//! │  │ • tier         │ │ • fail_reason  │ │ • overrides        │   │
//! │  └────────────────┘ └────────────────┘ └────────────────────┘   │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use alephcore::providers::profile_manager::{AuthProfileManager, EffectiveProfile};
//!
//! // Create manager
//! let manager = AuthProfileManager::new()?;
//!
//! // Get best available profile for a provider
//! let profile = manager.get_available_profile("anthropic", "main")?;
//!
//! // After successful API call
//! manager.mark_success(&profile.id)?;
//! manager.record_usage("main", &profile.id, 1000, 500, 0.015)?;
//!
//! // After failed API call
//! manager.mark_failure(&profile.id, AuthProfileFailureReason::RateLimit)?;
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use crate::sync_primitives::{Arc, RwLock};
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, warn};

use super::auth_profiles::{calculate_cooldown_ms, AuthProfileFailureReason};
use super::profile_config::{ProfileConfig, ProfileConfigError, ProfilesConfig, ProfileTier};

// ============================================================================
// Error Types
// ============================================================================

/// Error type for profile manager operations
#[derive(Debug, Error)]
pub enum ProfileManagerError {
    /// Configuration error
    #[error("Configuration error: {0}")]
    Config(#[from] ProfileConfigError),

    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization error
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// No profiles available
    #[error("No profiles available for provider: {0}")]
    NoProfilesAvailable(String),

    /// All profiles in cooldown
    #[error("All profiles for {0} are in cooldown")]
    AllProfilesInCooldown(String),

    /// Profile not found
    #[error("Profile not found: {0}")]
    ProfileNotFound(String),

    /// Budget exceeded
    #[error("Budget exceeded for profile {0}: limit ${1}, used ${2}")]
    BudgetExceeded(String, f64, f64),
}

/// Result type for profile manager operations
pub type ProfileManagerResult<T> = Result<T, ProfileManagerError>;

// ============================================================================
// Runtime State (In-Memory Only)
// ============================================================================

/// Runtime status for a profile (not persisted)
#[derive(Debug, Clone)]
#[derive(Default)]
pub struct RuntimeStatus {
    /// Whether the profile is currently rate limited
    pub is_rate_limited: bool,

    /// When the cooldown expires (using Instant for monotonic timing)
    pub cooldown_until: Option<Instant>,

    /// Consecutive failure count (resets on success)
    pub failure_count: u32,

    /// Last failure reason
    pub last_failure_reason: Option<AuthProfileFailureReason>,
}


impl RuntimeStatus {
    /// Check if currently in cooldown
    pub fn is_in_cooldown(&self) -> bool {
        self.cooldown_until.is_some_and(|until| Instant::now() < until)
    }

    /// Get remaining cooldown duration in milliseconds
    pub fn cooldown_remaining_ms(&self) -> Option<u64> {
        self.cooldown_until.and_then(|until| {
            let now = Instant::now();
            if now < until {
                Some(until.duration_since(now).as_millis() as u64)
            } else {
                None
            }
        })
    }
}

// ============================================================================
// Per-Agent State (Persisted)
// ============================================================================

/// Usage statistics for a profile within an agent context
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProfileUsage {
    /// Total input tokens used
    #[serde(default)]
    pub input_tokens: u64,

    /// Total output tokens used
    #[serde(default)]
    pub output_tokens: u64,

    /// Total cost in USD
    #[serde(default)]
    pub total_cost_usd: f64,

    /// Total request count
    #[serde(default)]
    pub request_count: u64,

    /// Last used timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<DateTime<Utc>>,
}

/// Per-profile overrides for an agent
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProfileOverride {
    /// Maximum budget in USD (None = unlimited)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_budget_usd: Option<f64>,

    /// Whether this profile is disabled for this agent
    #[serde(default)]
    pub disabled: bool,
}

/// Per-agent state (persisted to ~/.aleph/workspaces/{id}/state.json)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentState {
    /// Usage statistics per profile
    #[serde(default)]
    pub usage: HashMap<String, ProfileUsage>,

    /// Per-profile overrides
    #[serde(default)]
    pub overrides: HashMap<String, ProfileOverride>,
}

impl AgentState {
    /// Load state from file
    pub fn load(path: &Path) -> ProfileManagerResult<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;
        let state: AgentState = serde_json::from_str(&content)?;
        Ok(state)
    }

    /// Save state to file
    pub fn save(&self, path: &Path) -> ProfileManagerResult<()> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Get usage for a profile
    pub fn get_usage(&self, profile_id: &str) -> Option<&ProfileUsage> {
        self.usage.get(profile_id)
    }

    /// Get mutable usage for a profile, creating if needed
    pub fn get_or_create_usage(&mut self, profile_id: &str) -> &mut ProfileUsage {
        self.usage
            .entry(profile_id.to_string())
            .or_default()
    }

    /// Get override for a profile
    pub fn get_override(&self, profile_id: &str) -> Option<&ProfileOverride> {
        self.overrides.get(profile_id)
    }

    /// Check if a profile is disabled for this agent
    pub fn is_profile_disabled(&self, profile_id: &str) -> bool {
        self.overrides
            .get(profile_id)
            .is_some_and(|o| o.disabled)
    }

    /// Check if a profile exceeds budget
    pub fn exceeds_budget(&self, profile_id: &str) -> bool {
        let Some(override_) = self.overrides.get(profile_id) else {
            return false;
        };
        let Some(max_budget) = override_.max_budget_usd else {
            return false;
        };
        let Some(usage) = self.usage.get(profile_id) else {
            return false;
        };
        usage.total_cost_usd >= max_budget
    }
}

// ============================================================================
// Effective Profile (Ready-to-Use)
// ============================================================================

/// Effective profile ready for use (with resolved API key)
#[derive(Debug, Clone)]
pub struct EffectiveProfile {
    /// Profile ID
    pub id: String,

    /// Provider ID (e.g., "anthropic")
    pub provider: String,

    /// Resolved API key
    pub api_key: String,

    /// Optional base URL
    pub base_url: Option<String>,

    /// Tier
    pub tier: ProfileTier,

    /// Optional organization ID
    pub org_id: Option<String>,

    /// Optional model override
    pub model: Option<String>,
}

impl EffectiveProfile {
    /// Create from profile config
    fn from_config(
        id: String,
        config: &ProfileConfig,
    ) -> Result<Self, ProfileConfigError> {
        let api_key = config.resolve_api_key()?;
        Ok(Self {
            id,
            provider: config.provider.clone(),
            api_key,
            base_url: config.base_url.clone(),
            tier: config.tier,
            org_id: config.org_id.clone(),
            model: config.model.clone(),
        })
    }
}

// ============================================================================
// Profile Info (For Listing)
// ============================================================================

/// Profile information for listing/UI
#[derive(Debug, Clone, Serialize)]
pub struct ProfileInfo {
    /// Profile ID
    pub id: String,

    /// Provider ID
    pub provider: String,

    /// Tier
    pub tier: ProfileTier,

    /// Whether currently in cooldown
    pub in_cooldown: bool,

    /// Cooldown remaining in milliseconds (if any)
    pub cooldown_remaining_ms: Option<u64>,

    /// Whether disabled globally
    pub disabled: bool,

    /// Current failure count
    pub failure_count: u32,

    /// Last failure reason (if any)
    pub last_failure_reason: Option<AuthProfileFailureReason>,

    /// Whether API key uses environment variable
    pub uses_env_var: bool,

    /// Whether API key is currently resolvable
    pub key_resolvable: bool,
}

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
        let mut best_cooldown_profile: Option<(&String, &ProfileConfig, u64)> = None;

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
        let agent_states = self.agent_states.read().unwrap_or_else(|e| e.into_inner());
        if let Some(state) = agent_states.get(agent_id) {
            return Ok(state.clone());
        }
        drop(agent_states);

        let path = self.agent_state_path(agent_id);
        let state = AgentState::load(&path)?;

        let mut agent_states = self.agent_states.write().unwrap_or_else(|e| e.into_inner());
        agent_states.insert(agent_id.to_string(), state.clone());

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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_config(temp_dir: &TempDir) -> PathBuf {
        let config_path = temp_dir.path().join("profiles.toml");
        let content = r#"
            [profiles.anthropic_primary]
            provider = "anthropic"
            api_key = "sk-ant-primary"
            tier = "primary"

            [profiles.anthropic_backup]
            provider = "anthropic"
            api_key = "sk-ant-backup"
            tier = "backup"

            [profiles.openai_main]
            provider = "openai"
            api_key = "sk-openai-main"
            tier = "primary"
        "#;
        std::fs::write(&config_path, content).unwrap();
        config_path
    }

    #[test]
    fn test_manager_creation() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = create_test_config(&temp_dir);
        let agents_dir = temp_dir.path().join("agents");

        let manager = AuthProfileManager::with_paths(config_path, agents_dir).unwrap();
        assert_eq!(manager.profile_count(), 3);
    }

    #[test]
    fn test_get_available_profile() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = create_test_config(&temp_dir);
        let agents_dir = temp_dir.path().join("agents");

        let manager = AuthProfileManager::with_paths(config_path, agents_dir).unwrap();

        // Should get primary profile first
        let profile = manager.get_available_profile("anthropic", "main").unwrap();
        assert_eq!(profile.provider, "anthropic");
        assert_eq!(profile.tier, ProfileTier::Primary);
        assert_eq!(profile.api_key, "sk-ant-primary");
    }

    #[test]
    fn test_mark_failure_triggers_cooldown() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = create_test_config(&temp_dir);
        let agents_dir = temp_dir.path().join("agents");

        let manager = AuthProfileManager::with_paths(config_path, agents_dir).unwrap();

        // Mark primary as failed
        manager
            .mark_failure("anthropic_primary", AuthProfileFailureReason::RateLimit)
            .unwrap();

        // Check that profile is in cooldown
        let profiles = manager.profiles_for_provider("anthropic");
        let primary = profiles.iter().find(|p| p.id == "anthropic_primary").unwrap();
        assert!(primary.in_cooldown);
        assert!(primary.cooldown_remaining_ms.is_some());
        assert_eq!(primary.failure_count, 1);
    }

    #[test]
    fn test_fallback_to_backup_on_cooldown() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = create_test_config(&temp_dir);
        let agents_dir = temp_dir.path().join("agents");

        let manager = AuthProfileManager::with_paths(config_path, agents_dir).unwrap();

        // Mark primary as failed
        manager
            .mark_failure("anthropic_primary", AuthProfileFailureReason::RateLimit)
            .unwrap();

        // Should get backup profile now
        let profile = manager.get_available_profile("anthropic", "main").unwrap();
        assert_eq!(profile.id, "anthropic_backup");
        assert_eq!(profile.tier, ProfileTier::Backup);
    }

    #[test]
    fn test_mark_success_clears_cooldown() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = create_test_config(&temp_dir);
        let agents_dir = temp_dir.path().join("agents");

        let manager = AuthProfileManager::with_paths(config_path, agents_dir).unwrap();

        // Mark as failed, then success
        manager
            .mark_failure("anthropic_primary", AuthProfileFailureReason::RateLimit)
            .unwrap();
        manager.mark_success("anthropic_primary").unwrap();

        // Should not be in cooldown
        let profiles = manager.profiles_for_provider("anthropic");
        let primary = profiles.iter().find(|p| p.id == "anthropic_primary").unwrap();
        assert!(!primary.in_cooldown);
        assert_eq!(primary.failure_count, 0);
    }

    #[test]
    fn test_record_usage() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = create_test_config(&temp_dir);
        let agents_dir = temp_dir.path().join("agents");

        let manager = AuthProfileManager::with_paths(config_path, agents_dir.clone()).unwrap();

        // Record usage
        manager
            .record_usage("main", "anthropic_primary", 1000, 500, 0.015)
            .unwrap();

        // Check that state was saved
        let state_path = agents_dir.join("main").join("state.json");
        assert!(state_path.exists());

        let state = AgentState::load(&state_path).unwrap();
        let usage = state.get_usage("anthropic_primary").unwrap();
        assert_eq!(usage.input_tokens, 1000);
        assert_eq!(usage.output_tokens, 500);
        assert!((usage.total_cost_usd - 0.015).abs() < 0.0001);
        assert_eq!(usage.request_count, 1);
    }

    #[test]
    fn test_budget_override() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = create_test_config(&temp_dir);
        let agents_dir = temp_dir.path().join("agents");

        let manager = AuthProfileManager::with_paths(config_path, agents_dir).unwrap();

        // Set a $10 budget
        manager
            .set_budget_override("main", "anthropic_primary", Some(10.0))
            .unwrap();

        // Record usage that exceeds budget
        manager
            .record_usage("main", "anthropic_primary", 100000, 50000, 11.0)
            .unwrap();

        // Should skip to backup because primary exceeds budget
        let profile = manager.get_available_profile("anthropic", "main").unwrap();
        assert_eq!(profile.id, "anthropic_backup");
    }

    #[test]
    fn test_disable_profile_for_agent() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = create_test_config(&temp_dir);
        let agents_dir = temp_dir.path().join("agents");

        let manager = AuthProfileManager::with_paths(config_path, agents_dir).unwrap();

        // Disable primary for this agent
        manager
            .disable_profile_for_agent("main", "anthropic_primary", true)
            .unwrap();

        // Should get backup profile
        let profile = manager.get_available_profile("anthropic", "main").unwrap();
        assert_eq!(profile.id, "anthropic_backup");
    }

    #[test]
    fn test_list_profiles() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = create_test_config(&temp_dir);
        let agents_dir = temp_dir.path().join("agents");

        let manager = AuthProfileManager::with_paths(config_path, agents_dir).unwrap();

        let profiles = manager.list_profiles();
        assert_eq!(profiles.len(), 3);

        // All should have resolvable keys (literal keys)
        assert!(profiles.iter().all(|p| p.key_resolvable));
        assert!(profiles.iter().all(|p| !p.uses_env_var));
    }

    #[test]
    fn test_no_profiles_error() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("profiles.toml");
        let agents_dir = temp_dir.path().join("agents");

        // Empty config
        std::fs::write(&config_path, "").unwrap();

        let manager = AuthProfileManager::with_paths(config_path, agents_dir).unwrap();

        let result = manager.get_available_profile("anthropic", "main");
        assert!(matches!(
            result,
            Err(ProfileManagerError::NoProfilesAvailable(_))
        ));
    }

    #[test]
    fn test_clear_cooldown() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = create_test_config(&temp_dir);
        let agents_dir = temp_dir.path().join("agents");

        let manager = AuthProfileManager::with_paths(config_path, agents_dir).unwrap();

        // Mark as failed
        manager
            .mark_failure("anthropic_primary", AuthProfileFailureReason::RateLimit)
            .unwrap();

        // Clear cooldown
        manager.clear_cooldown("anthropic_primary").unwrap();

        // Should not be in cooldown
        let profiles = manager.profiles_for_provider("anthropic");
        let primary = profiles.iter().find(|p| p.id == "anthropic_primary").unwrap();
        assert!(!primary.in_cooldown);
    }

    #[test]
    fn test_reload_config() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = create_test_config(&temp_dir);
        let agents_dir = temp_dir.path().join("agents");

        let manager = AuthProfileManager::with_paths(config_path.clone(), agents_dir).unwrap();
        assert_eq!(manager.profile_count(), 3);

        // Add a new profile to config
        let new_content = r#"
            [profiles.anthropic_primary]
            provider = "anthropic"
            api_key = "sk-ant-primary"
            tier = "primary"

            [profiles.new_profile]
            provider = "gemini"
            api_key = "gemini-key"
            tier = "primary"
        "#;
        std::fs::write(&config_path, new_content).unwrap();

        // Reload
        manager.reload_config().unwrap();
        assert_eq!(manager.profile_count(), 2);
    }
}
