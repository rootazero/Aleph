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

mod manager;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::auth_profiles::AuthProfileFailureReason;
use super::profile_config::{ProfileConfigError, ProfileTier};

// Re-export the manager and all public types
pub use manager::AuthProfileManager;

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
    pub(super) fn from_config(
        id: String,
        config: &super::profile_config::ProfileConfig,
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
