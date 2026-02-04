//! Profile configuration for ~/.aleph/profiles.toml
//!
//! This module provides TOML-based configuration for auth profiles with:
//! - Environment variable resolution for API keys
//! - Per-provider profile grouping
//! - Tier-based organization
//!
//! # Example profiles.toml
//!
//! ```toml
//! [profiles.anthropic_main]
//! provider = "anthropic"
//! api_key = "env:ANTHROPIC_API_KEY"
//! tier = "primary"
//!
//! [profiles.anthropic_backup]
//! provider = "anthropic"
//! api_key = "sk-ant-backup-key"
//! tier = "backup"
//! org_id = "org_123"
//!
//! [profiles.openai_prod]
//! provider = "openai"
//! api_key = "env:OPENAI_API_KEY"
//! base_url = "https://api.openai.com/v1"
//! tier = "primary"
//! ```

use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, warn};

/// Error type for profile configuration
#[derive(Debug, Error)]
pub enum ProfileConfigError {
    /// IO error reading/writing file
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// TOML parsing error
    #[error("TOML parse error: {0}")]
    Parse(#[from] toml::de::Error),

    /// TOML serialization error
    #[error("TOML serialization error: {0}")]
    Serialize(#[from] toml::ser::Error),

    /// Environment variable not found
    #[error("Environment variable not found: {0}")]
    EnvVarNotFound(String),

    /// Profile not found
    #[error("Profile not found: {0}")]
    ProfileNotFound(String),

    /// Invalid configuration
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),
}

/// Result type for profile configuration operations
pub type ProfileConfigResult<T> = Result<T, ProfileConfigError>;

/// Tier classification for profiles
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ProfileTier {
    /// Primary profile (used first)
    #[default]
    Primary,
    /// Backup profile (used when primary fails)
    Backup,
    /// Fallback profile (last resort)
    Fallback,
}

impl ProfileTier {
    /// Get priority score (lower = higher priority)
    pub fn priority(&self) -> u8 {
        match self {
            Self::Primary => 0,
            Self::Backup => 1,
            Self::Fallback => 2,
        }
    }
}

/// Single profile configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileConfig {
    /// Provider ID (e.g., "anthropic", "openai")
    pub provider: String,

    /// API key - can be literal or "env:VAR_NAME" for environment variable
    pub api_key: String,

    /// Optional base URL override
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,

    /// Profile tier for ordering
    #[serde(default)]
    pub tier: ProfileTier,

    /// Optional organization ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,

    /// Optional model override
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Whether this profile is disabled
    #[serde(default)]
    pub disabled: bool,
}

impl ProfileConfig {
    /// Resolve the API key, handling env: prefix
    ///
    /// If the api_key starts with "env:", the rest is treated as an
    /// environment variable name and its value is returned.
    ///
    /// # Returns
    /// - `Ok(String)` - The resolved API key
    /// - `Err(ProfileConfigError::EnvVarNotFound)` - If env var not found
    pub fn resolve_api_key(&self) -> ProfileConfigResult<String> {
        if let Some(var_name) = self.api_key.strip_prefix("env:") {
            env::var(var_name).map_err(|_| {
                ProfileConfigError::EnvVarNotFound(var_name.to_string())
            })
        } else {
            Ok(self.api_key.clone())
        }
    }

    /// Check if the API key uses an environment variable
    pub fn uses_env_var(&self) -> bool {
        self.api_key.starts_with("env:")
    }

    /// Get the environment variable name if using env: prefix
    pub fn env_var_name(&self) -> Option<&str> {
        self.api_key.strip_prefix("env:")
    }
}

/// Root configuration containing all profiles
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProfilesConfig {
    /// Map of profile_id -> ProfileConfig
    #[serde(default)]
    pub profiles: HashMap<String, ProfileConfig>,
}

impl ProfilesConfig {
    /// Create an empty configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Load configuration from default path (~/.aleph/profiles.toml)
    pub fn load_default() -> ProfileConfigResult<Self> {
        let path = Self::default_path();
        if path.exists() {
            Self::load(&path)
        } else {
            debug!("No profiles.toml found at {:?}, using empty config", path);
            Ok(Self::new())
        }
    }

    /// Get the default configuration file path
    pub fn default_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".aleph")
            .join("profiles.toml")
    }

    /// Load configuration from a specific path
    pub fn load(path: &Path) -> ProfileConfigResult<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: ProfilesConfig = toml::from_str(&content)?;
        debug!(
            path = %path.display(),
            profile_count = config.profiles.len(),
            "Loaded profiles config"
        );
        Ok(config)
    }

    /// Save configuration to a specific path
    pub fn save(&self, path: &Path) -> ProfileConfigResult<()> {
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        debug!(path = %path.display(), "Saved profiles config");
        Ok(())
    }

    /// Save to default path
    pub fn save_default(&self) -> ProfileConfigResult<()> {
        self.save(&Self::default_path())
    }

    /// Get profiles for a specific provider
    ///
    /// Returns profiles sorted by tier priority (primary first, then backup, then fallback)
    pub fn profiles_for_provider(&self, provider: &str) -> Vec<(&String, &ProfileConfig)> {
        let normalized = provider.trim().to_lowercase();

        let mut profiles: Vec<_> = self
            .profiles
            .iter()
            .filter(|(_, config)| {
                !config.disabled && config.provider.trim().to_lowercase() == normalized
            })
            .collect();

        // Sort by tier priority
        profiles.sort_by_key(|(_, config)| config.tier.priority());

        profiles
    }

    /// Get a specific profile by ID
    pub fn get_profile(&self, profile_id: &str) -> Option<&ProfileConfig> {
        self.profiles.get(profile_id)
    }

    /// Add or update a profile
    pub fn upsert_profile(&mut self, profile_id: String, config: ProfileConfig) {
        self.profiles.insert(profile_id, config);
    }

    /// Remove a profile
    pub fn remove_profile(&mut self, profile_id: &str) -> Option<ProfileConfig> {
        self.profiles.remove(profile_id)
    }

    /// List all unique providers
    pub fn list_providers(&self) -> Vec<String> {
        let mut providers: Vec<String> = self
            .profiles
            .values()
            .filter(|c| !c.disabled)
            .map(|c| c.provider.trim().to_lowercase())
            .collect();

        providers.sort();
        providers.dedup();
        providers
    }

    /// Validate all profiles and return any issues
    pub fn validate(&self) -> Vec<(String, String)> {
        let mut issues = Vec::new();

        for (id, config) in &self.profiles {
            // Check if API key can be resolved
            if config.uses_env_var() {
                if let Err(e) = config.resolve_api_key() {
                    warn!(profile_id = %id, error = %e, "Profile API key cannot be resolved");
                    issues.push((id.clone(), e.to_string()));
                }
            }

            // Check if API key is empty
            if config.api_key.is_empty() {
                issues.push((id.clone(), "API key is empty".to_string()));
            }

            // Check if provider is empty
            if config.provider.trim().is_empty() {
                issues.push((id.clone(), "Provider is empty".to_string()));
            }
        }

        issues
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profile_config_literal_key() {
        let config = ProfileConfig {
            provider: "anthropic".to_string(),
            api_key: "sk-ant-test-key".to_string(),
            base_url: None,
            tier: ProfileTier::Primary,
            org_id: None,
            model: None,
            disabled: false,
        };

        assert!(!config.uses_env_var());
        assert_eq!(config.resolve_api_key().unwrap(), "sk-ant-test-key");
    }

    #[test]
    fn test_profile_config_env_key() {
        // Set test env var
        env::set_var("TEST_API_KEY_12345", "secret-key-from-env");

        let config = ProfileConfig {
            provider: "openai".to_string(),
            api_key: "env:TEST_API_KEY_12345".to_string(),
            base_url: None,
            tier: ProfileTier::Backup,
            org_id: None,
            model: None,
            disabled: false,
        };

        assert!(config.uses_env_var());
        assert_eq!(config.env_var_name(), Some("TEST_API_KEY_12345"));
        assert_eq!(config.resolve_api_key().unwrap(), "secret-key-from-env");

        // Cleanup
        env::remove_var("TEST_API_KEY_12345");
    }

    #[test]
    fn test_profile_config_env_key_not_found() {
        let config = ProfileConfig {
            provider: "anthropic".to_string(),
            api_key: "env:NONEXISTENT_VAR_XYZABC".to_string(),
            base_url: None,
            tier: ProfileTier::Primary,
            org_id: None,
            model: None,
            disabled: false,
        };

        let err = config.resolve_api_key().unwrap_err();
        assert!(matches!(err, ProfileConfigError::EnvVarNotFound(_)));
    }

    #[test]
    fn test_tier_priority() {
        assert!(ProfileTier::Primary.priority() < ProfileTier::Backup.priority());
        assert!(ProfileTier::Backup.priority() < ProfileTier::Fallback.priority());
    }

    #[test]
    fn test_profiles_config_parse() {
        let toml_content = r#"
            [profiles.anthropic_main]
            provider = "anthropic"
            api_key = "sk-ant-main"
            tier = "primary"

            [profiles.anthropic_backup]
            provider = "anthropic"
            api_key = "sk-ant-backup"
            tier = "backup"
            org_id = "org_123"

            [profiles.openai_prod]
            provider = "openai"
            api_key = "env:OPENAI_API_KEY"
            base_url = "https://api.openai.com/v1"
            tier = "primary"
        "#;

        let config: ProfilesConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.profiles.len(), 3);

        let main = config.get_profile("anthropic_main").unwrap();
        assert_eq!(main.provider, "anthropic");
        assert_eq!(main.tier, ProfileTier::Primary);

        let backup = config.get_profile("anthropic_backup").unwrap();
        assert_eq!(backup.org_id, Some("org_123".to_string()));
    }

    #[test]
    fn test_profiles_for_provider() {
        let toml_content = r#"
            [profiles.anthropic_primary]
            provider = "anthropic"
            api_key = "key1"
            tier = "primary"

            [profiles.anthropic_backup]
            provider = "anthropic"
            api_key = "key2"
            tier = "backup"

            [profiles.anthropic_fallback]
            provider = "anthropic"
            api_key = "key3"
            tier = "fallback"

            [profiles.openai_main]
            provider = "openai"
            api_key = "key4"
            tier = "primary"

            [profiles.anthropic_disabled]
            provider = "anthropic"
            api_key = "key5"
            tier = "primary"
            disabled = true
        "#;

        let config: ProfilesConfig = toml::from_str(toml_content).unwrap();

        let anthropic_profiles = config.profiles_for_provider("anthropic");
        assert_eq!(anthropic_profiles.len(), 3); // disabled not included

        // Check ordering by tier
        assert_eq!(anthropic_profiles[0].1.tier, ProfileTier::Primary);
        assert_eq!(anthropic_profiles[1].1.tier, ProfileTier::Backup);
        assert_eq!(anthropic_profiles[2].1.tier, ProfileTier::Fallback);

        // Check case insensitivity
        let anthropic_upper = config.profiles_for_provider("ANTHROPIC");
        assert_eq!(anthropic_upper.len(), 3);
    }

    #[test]
    fn test_list_providers() {
        let toml_content = r#"
            [profiles.a1]
            provider = "anthropic"
            api_key = "k1"

            [profiles.a2]
            provider = "Anthropic"
            api_key = "k2"

            [profiles.o1]
            provider = "openai"
            api_key = "k3"

            [profiles.g1]
            provider = "gemini"
            api_key = "k4"
            disabled = true
        "#;

        let config: ProfilesConfig = toml::from_str(toml_content).unwrap();
        let providers = config.list_providers();

        // Should have anthropic and openai (gemini is disabled)
        assert_eq!(providers.len(), 2);
        assert!(providers.contains(&"anthropic".to_string()));
        assert!(providers.contains(&"openai".to_string()));
    }

    #[test]
    fn test_serialize_roundtrip() {
        let mut config = ProfilesConfig::new();
        config.upsert_profile(
            "test_profile".to_string(),
            ProfileConfig {
                provider: "anthropic".to_string(),
                api_key: "sk-test".to_string(),
                base_url: Some("https://custom.api".to_string()),
                tier: ProfileTier::Backup,
                org_id: Some("org_456".to_string()),
                model: Some("claude-3-opus".to_string()),
                disabled: false,
            },
        );

        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: ProfilesConfig = toml::from_str(&toml_str).unwrap();

        assert_eq!(parsed.profiles.len(), 1);
        let profile = parsed.get_profile("test_profile").unwrap();
        assert_eq!(profile.base_url, Some("https://custom.api".to_string()));
        assert_eq!(profile.tier, ProfileTier::Backup);
    }

    #[test]
    fn test_validate_profiles() {
        env::set_var("VALID_KEY_123", "valid-key");

        let toml_content = r#"
            [profiles.valid]
            provider = "anthropic"
            api_key = "env:VALID_KEY_123"

            [profiles.missing_env]
            provider = "openai"
            api_key = "env:MISSING_ENV_VAR_XYZ"

            [profiles.empty_key]
            provider = "gemini"
            api_key = ""

            [profiles.empty_provider]
            provider = ""
            api_key = "some-key"
        "#;

        let config: ProfilesConfig = toml::from_str(toml_content).unwrap();
        let issues = config.validate();

        assert_eq!(issues.len(), 3);
        assert!(issues.iter().any(|(id, _)| id == "missing_env"));
        assert!(issues.iter().any(|(id, _)| id == "empty_key"));
        assert!(issues.iter().any(|(id, _)| id == "empty_provider"));

        env::remove_var("VALID_KEY_123");
    }
}
