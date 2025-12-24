use crate::error::{AetherError, Result};
/// Configuration structure for Aether
///
/// Phase 1: Stub implementation with basic fields.
/// Phase 4: Added memory configuration support.
/// Phase 5: Added AI provider configuration support.
/// Phase 8: Added config file loading from ~/.config/aether/config.toml
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Default hotkey (hardcoded to "Command+Grave" in Phase 1)
    pub default_hotkey: String,
    /// General settings
    #[serde(default)]
    pub general: GeneralConfig,
    /// Memory module configuration
    #[serde(default)]
    pub memory: MemoryConfig,
    /// AI provider configurations (Phase 5)
    /// Note: Not exposed through UniFFI dictionary, managed via separate methods
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub providers: HashMap<String, ProviderConfig>,
    /// Routing rules for smart AI provider selection (Phase 5)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<RoutingRuleConfig>,
}

/// General configuration settings
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GeneralConfig {
    /// Default provider to use when no routing rule matches
    #[serde(default)]
    pub default_provider: Option<String>,
}

/// Routing rule configuration for TOML parsing
///
/// Each rule specifies:
/// - A regex pattern to match against user input
/// - The provider to use when matched
/// - An optional system prompt override
///
/// # Example TOML
///
/// ```toml
/// [[rules]]
/// regex = "^/code"
/// provider = "claude"
/// system_prompt = "You are a senior software engineer."
///
/// [[rules]]
/// regex = ".*"
/// provider = "openai"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingRuleConfig {
    /// Regex pattern to match against user input
    pub regex: String,
    /// Provider name to use when this rule matches
    pub provider: String,
    /// Optional system prompt to guide AI behavior
    #[serde(default)]
    pub system_prompt: Option<String>,
}

/// AI Provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Provider type: "openai", "claude", "ollama", or custom name
    /// If not specified, inferred from provider name in config
    #[serde(default)]
    pub provider_type: Option<String>,
    /// API key for cloud providers (required for OpenAI, Claude)
    #[serde(default)]
    pub api_key: Option<String>,
    /// Model name (e.g., "gpt-4o", "claude-3-5-sonnet-20241022", "llama3.2")
    pub model: String,
    /// Base URL for API endpoint (optional, defaults to official API)
    #[serde(default)]
    pub base_url: Option<String>,
    /// Provider brand color for UI (hex string, e.g., "#10a37f")
    #[serde(default = "default_provider_color")]
    pub color: String,
    /// Request timeout in seconds
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    /// Maximum tokens in response (optional)
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Temperature for response randomness (0.0-2.0, optional)
    #[serde(default)]
    pub temperature: Option<f32>,
}

fn default_provider_color() -> String {
    "#808080".to_string() // Gray as default
}

fn default_timeout_seconds() -> u64 {
    30 // 30 seconds default timeout
}

impl ProviderConfig {
    /// Infer provider type from config
    ///
    /// If `provider_type` is explicitly set, use it.
    /// Otherwise, infer from provider name:
    /// - "openai" -> "openai"
    /// - "claude" -> "claude"
    /// - "ollama" -> "ollama"
    /// - anything with base_url -> "openai" (OpenAI-compatible)
    /// - default -> "openai"
    pub fn infer_provider_type(&self, provider_name: &str) -> String {
        if let Some(ref provider_type) = self.provider_type {
            return provider_type.clone();
        }

        // Infer from provider name
        let name_lower = provider_name.to_lowercase();
        if name_lower.contains("claude") {
            "claude".to_string()
        } else if name_lower.contains("ollama") {
            "ollama".to_string()
        } else {
            // Default to OpenAI-compatible (covers OpenAI, DeepSeek, Moonshot, etc.)
            "openai".to_string()
        }
    }
}

/// Memory module configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Enable/disable memory module
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Embedding model name
    #[serde(default = "default_embedding_model")]
    pub embedding_model: String,
    /// Maximum number of past interactions to retrieve
    #[serde(default = "default_max_context_items")]
    pub max_context_items: u32,
    /// Auto-delete memories older than N days (0 = never delete)
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
    /// Vector database backend: "sqlite-vec" or "lancedb"
    #[serde(default = "default_vector_db")]
    pub vector_db: String,
    /// Minimum similarity score to include memory (0.0-1.0)
    #[serde(default = "default_similarity_threshold")]
    pub similarity_threshold: f32,
    /// List of app bundle IDs to exclude from memory storage
    #[serde(default)]
    pub excluded_apps: Vec<String>,
}

// Default value functions for MemoryConfig
fn default_enabled() -> bool {
    true
}

fn default_embedding_model() -> String {
    "all-MiniLM-L6-v2".to_string()
}

fn default_max_context_items() -> u32 {
    5
}

fn default_retention_days() -> u32 {
    90
}

fn default_vector_db() -> String {
    "sqlite-vec".to_string()
}

fn default_similarity_threshold() -> f32 {
    0.7 // Minimum similarity score for real embedding models
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            embedding_model: default_embedding_model(),
            max_context_items: default_max_context_items(),
            retention_days: default_retention_days(),
            vector_db: default_vector_db(),
            similarity_threshold: default_similarity_threshold(),
            excluded_apps: vec![
                "com.apple.keychainaccess".to_string(),
                "com.agilebits.onepassword7".to_string(),
                "com.lastpass.LastPass".to_string(),
                "com.bitwarden.desktop".to_string(),
            ],
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_hotkey: "Command+Grave".to_string(),
            general: GeneralConfig::default(),
            memory: MemoryConfig::default(),
            providers: HashMap::new(),
            rules: Vec::new(),
        }
    }
}

impl Config {
    /// Create a new config with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the default config path: ~/.config/aether/config.toml
    pub fn default_path() -> PathBuf {
        if let Some(home) = dirs::home_dir() {
            home.join(".config").join("aether").join("config.toml")
        } else {
            // Fallback to current directory if home dir not found
            PathBuf::from("config.toml")
        }
    }

    /// Load configuration from a TOML file
    ///
    /// # Arguments
    /// * `path` - Path to the config file
    ///
    /// # Returns
    /// * `Ok(Config)` - Successfully loaded config
    /// * `Err(AetherError::ConfigNotFound)` - File doesn't exist
    /// * `Err(AetherError::InvalidConfig)` - File exists but parsing failed
    ///
    /// # Example
    /// ```no_run
    /// use aethecore::config::Config;
    ///
    /// let config = Config::load_from_file("config.toml").unwrap();
    /// ```
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();

        // Check if file exists
        if !path.exists() {
            return Err(AetherError::InvalidConfig(format!(
                "Config file not found: {}",
                path.display()
            )));
        }

        // Read file contents
        let contents = fs::read_to_string(path).map_err(|e| {
            AetherError::InvalidConfig(format!(
                "Failed to read config file {}: {}",
                path.display(),
                e
            ))
        })?;

        // Parse TOML
        let config: Config = toml::from_str(&contents).map_err(|e| {
            AetherError::InvalidConfig(format!(
                "Failed to parse config file {}: {}",
                path.display(),
                e
            ))
        })?;

        // Validate config
        config.validate()?;

        Ok(config)
    }

    /// Load configuration from default path (~/.config/aether/config.toml)
    /// Falls back to default config if file doesn't exist
    ///
    /// # Returns
    /// * `Ok(Config)` - Successfully loaded config or default config
    /// * `Err(AetherError::InvalidConfig)` - File exists but parsing failed
    ///
    /// # Example
    /// ```no_run
    /// use aethecore::config::Config;
    ///
    /// let config = Config::load().unwrap();
    /// ```
    pub fn load() -> Result<Self> {
        let path = Self::default_path();

        if path.exists() {
            log::info!("Loading config from {}", path.display());
            Self::load_from_file(&path)
        } else {
            log::info!(
                "Config file not found at {}, using default config",
                path.display()
            );
            Ok(Self::default())
        }
    }

    /// Validate configuration
    ///
    /// Checks:
    /// - Provider references in rules exist in providers map
    /// - Default provider exists (if specified)
    /// - API keys are present for cloud providers
    /// - Regex patterns are valid
    pub fn validate(&self) -> Result<()> {
        // Validate default provider exists
        if let Some(ref default_provider) = self.general.default_provider {
            if !self.providers.contains_key(default_provider) {
                return Err(AetherError::InvalidConfig(format!(
                    "Default provider '{}' not found in providers",
                    default_provider
                )));
            }
        }

        // Validate provider configurations
        for (name, provider) in &self.providers {
            let provider_type = provider.infer_provider_type(name);

            // Check API key for cloud providers
            if (provider_type == "openai" || provider_type == "claude")
                && provider.api_key.is_none()
            {
                return Err(AetherError::InvalidConfig(format!(
                    "Provider '{}' requires an API key",
                    name
                )));
            }

            // Validate timeout
            if provider.timeout_seconds == 0 {
                return Err(AetherError::InvalidConfig(format!(
                    "Provider '{}' timeout must be greater than 0",
                    name
                )));
            }

            // Validate temperature if specified
            if let Some(temp) = provider.temperature {
                if !(0.0..=2.0).contains(&temp) {
                    return Err(AetherError::InvalidConfig(format!(
                        "Provider '{}' temperature must be between 0.0 and 2.0, got {}",
                        name, temp
                    )));
                }
            }
        }

        // Validate routing rules
        for (idx, rule) in self.rules.iter().enumerate() {
            // Check provider exists
            if !self.providers.contains_key(&rule.provider) {
                return Err(AetherError::InvalidConfig(format!(
                    "Rule #{} references unknown provider '{}'",
                    idx + 1,
                    rule.provider
                )));
            }

            // Validate regex pattern
            if let Err(e) = regex::Regex::new(&rule.regex) {
                return Err(AetherError::InvalidConfig(format!(
                    "Rule #{} has invalid regex '{}': {}",
                    idx + 1,
                    rule.regex,
                    e
                )));
            }
        }

        // Validate memory config
        if self.memory.max_context_items == 0 {
            return Err(AetherError::InvalidConfig(
                "memory.max_context_items must be greater than 0".to_string(),
            ));
        }

        if !(0.0..=1.0).contains(&self.memory.similarity_threshold) {
            return Err(AetherError::InvalidConfig(format!(
                "memory.similarity_threshold must be between 0.0 and 1.0, got {}",
                self.memory.similarity_threshold
            )));
        }

        Ok(())
    }

    /// Save configuration to a TOML file
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let path = path.as_ref();

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                AetherError::InvalidConfig(format!(
                    "Failed to create config directory {}: {}",
                    parent.display(),
                    e
                ))
            })?;
        }

        // Serialize to TOML
        let contents = toml::to_string_pretty(self).map_err(|e| {
            AetherError::InvalidConfig(format!("Failed to serialize config: {}", e))
        })?;

        // Write to file
        fs::write(path, contents).map_err(|e| {
            AetherError::InvalidConfig(format!(
                "Failed to write config file {}: {}",
                path.display(),
                e
            ))
        })?;

        log::info!("Config saved to {}", path.display());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.default_hotkey, "Command+Grave");
        assert!(config.memory.enabled);
    }

    #[test]
    fn test_new_config() {
        let config = Config::new();
        assert_eq!(config.default_hotkey, "Command+Grave");
    }

    #[test]
    fn test_config_serialization() {
        let config = Config::default();
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("Command+Grave"));
        assert!(json.contains("memory"));
    }

    #[test]
    fn test_config_deserialization() {
        let json = r#"{"default_hotkey":"Command+Grave"}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.default_hotkey, "Command+Grave");
        // memory field should use default
        assert_eq!(config.memory.embedding_model, "all-MiniLM-L6-v2");
    }

    #[test]
    fn test_memory_config_defaults() {
        let mem_config = MemoryConfig::default();
        assert!(mem_config.enabled);
        assert_eq!(mem_config.embedding_model, "all-MiniLM-L6-v2");
        assert_eq!(mem_config.max_context_items, 5);
        assert_eq!(mem_config.retention_days, 90);
        assert_eq!(mem_config.vector_db, "sqlite-vec");
        assert_eq!(mem_config.similarity_threshold, 0.7);
        assert!(!mem_config.excluded_apps.is_empty());
    }

    #[test]
    fn test_memory_config_serialization() {
        let mem_config = MemoryConfig::default();
        let json = serde_json::to_string(&mem_config).unwrap();
        assert!(json.contains("all-MiniLM-L6-v2"));
        assert!(json.contains("sqlite-vec"));
    }

    #[test]
    fn test_memory_config_deserialization() {
        let json = r#"{
            "enabled": false,
            "embedding_model": "custom-model",
            "max_context_items": 10,
            "retention_days": 30,
            "vector_db": "lancedb",
            "similarity_threshold": 0.8,
            "excluded_apps": ["com.example.app"]
        }"#;
        let config: MemoryConfig = serde_json::from_str(json).unwrap();
        assert!(!config.enabled);
        assert_eq!(config.embedding_model, "custom-model");
        assert_eq!(config.max_context_items, 10);
        assert_eq!(config.retention_days, 30);
        assert_eq!(config.vector_db, "lancedb");
        assert_eq!(config.similarity_threshold, 0.8);
        assert_eq!(config.excluded_apps, vec!["com.example.app"]);
    }

    #[test]
    fn test_default_excluded_apps() {
        let mem_config = MemoryConfig::default();
        assert!(mem_config
            .excluded_apps
            .contains(&"com.apple.keychainaccess".to_string()));
        assert!(mem_config
            .excluded_apps
            .contains(&"com.agilebits.onepassword7".to_string()));
    }

    #[test]
    fn test_config_validation_valid() {
        let mut config = Config::default();

        // Add a provider
        let provider = ProviderConfig {
            provider_type: Some("openai".to_string()),
            api_key: Some("sk-test".to_string()),
            model: "gpt-4o".to_string(),
            base_url: None,
            color: "#10a37f".to_string(),
            timeout_seconds: 30,
            max_tokens: Some(4096),
            temperature: Some(0.7),
        };
        config.providers.insert("openai".to_string(), provider);
        config.general.default_provider = Some("openai".to_string());

        // Should pass validation
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_missing_default_provider() {
        let mut config = Config::default();
        config.general.default_provider = Some("nonexistent".to_string());

        // Should fail validation
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_missing_api_key() {
        let mut config = Config::default();

        // Add OpenAI provider without API key
        let provider = ProviderConfig {
            provider_type: Some("openai".to_string()),
            api_key: None,
            model: "gpt-4o".to_string(),
            base_url: None,
            color: "#10a37f".to_string(),
            timeout_seconds: 30,
            max_tokens: Some(4096),
            temperature: Some(0.7),
        };
        config.providers.insert("openai".to_string(), provider);

        // Should fail validation
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_temperature() {
        let mut config = Config::default();

        // Add provider with invalid temperature
        let provider = ProviderConfig {
            provider_type: Some("openai".to_string()),
            api_key: Some("sk-test".to_string()),
            model: "gpt-4o".to_string(),
            base_url: None,
            color: "#10a37f".to_string(),
            timeout_seconds: 30,
            max_tokens: Some(4096),
            temperature: Some(3.0), // Invalid: > 2.0
        };
        config.providers.insert("openai".to_string(), provider);

        // Should fail validation
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_regex() {
        let mut config = Config::default();

        // Add valid provider
        let provider = ProviderConfig {
            provider_type: Some("openai".to_string()),
            api_key: Some("sk-test".to_string()),
            model: "gpt-4o".to_string(),
            base_url: None,
            color: "#10a37f".to_string(),
            timeout_seconds: 30,
            max_tokens: Some(4096),
            temperature: Some(0.7),
        };
        config.providers.insert("openai".to_string(), provider);

        // Add rule with invalid regex
        config.rules.push(RoutingRuleConfig {
            regex: "[invalid(".to_string(),
            provider: "openai".to_string(),
            system_prompt: None,
        });

        // Should fail validation
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_rule_unknown_provider() {
        let mut config = Config::default();

        // Add rule referencing unknown provider
        config.rules.push(RoutingRuleConfig {
            regex: ".*".to_string(),
            provider: "nonexistent".to_string(),
            system_prompt: None,
        });

        // Should fail validation
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_load_from_toml() {
        let toml_str = r##"
default_hotkey = "Command+Grave"

[general]
default_provider = "openai"

[providers.openai]
api_key = "sk-test"
model = "gpt-4o"
color = "#10a37f"
timeout_seconds = 30
max_tokens = 4096
temperature = 0.7

[[rules]]
regex = "^/code"
provider = "openai"
system_prompt = "You are a coding assistant."

[memory]
enabled = true
max_context_items = 5
"##;

        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.default_hotkey, "Command+Grave");
        assert_eq!(config.general.default_provider, Some("openai".to_string()));
        assert!(config.providers.contains_key("openai"));
        assert_eq!(config.rules.len(), 1);
        assert!(config.memory.enabled);

        // Validation should pass
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_save_and_load() {
        use tempfile::NamedTempFile;

        let mut config = Config::default();

        // Add a provider
        let provider = ProviderConfig {
            provider_type: Some("openai".to_string()),
            api_key: Some("sk-test".to_string()),
            model: "gpt-4o".to_string(),
            base_url: None,
            color: "#10a37f".to_string(),
            timeout_seconds: 30,
            max_tokens: Some(4096),
            temperature: Some(0.7),
        };
        config.providers.insert("openai".to_string(), provider);
        config.general.default_provider = Some("openai".to_string());

        // Save to temp file
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path();
        config.save_to_file(path).unwrap();

        // Load back
        let loaded = Config::load_from_file(path).unwrap();
        assert_eq!(loaded.default_hotkey, config.default_hotkey);
        assert_eq!(
            loaded.general.default_provider,
            config.general.default_provider
        );
        assert!(loaded.providers.contains_key("openai"));
    }

    #[test]
    fn test_config_ollama_no_api_key() {
        let mut config = Config::default();

        // Ollama provider doesn't need API key
        let provider = ProviderConfig {
            provider_type: Some("ollama".to_string()),
            api_key: None,
            model: "llama3.2".to_string(),
            base_url: None,
            color: "#0000ff".to_string(),
            timeout_seconds: 60,
            max_tokens: None,
            temperature: None,
        };
        config.providers.insert("ollama".to_string(), provider);

        // Should pass validation (no API key needed for Ollama)
        assert!(config.validate().is_ok());
    }
}
