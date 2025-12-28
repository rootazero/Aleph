use crate::error::{AetherError, Result};
/// Configuration structure for Aether
///
/// Phase 1: Stub implementation with basic fields.
/// Phase 4: Added memory configuration support.
/// Phase 5: Added AI provider configuration support.
/// Phase 6: Added Keychain integration and file watching support.
/// Phase 8: Added config file loading from ~/.config/aether/config.toml
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, error, info};

// Submodules
pub mod keychain;
pub mod watcher;
pub use keychain::KeychainManager;
#[allow(unused_imports)]
pub use watcher::ConfigWatcher;

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
    /// Shortcuts configuration (Phase 6)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shortcuts: Option<ShortcutsConfig>,
    /// Behavior configuration (Phase 6)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior: Option<BehaviorConfig>,
}

/// General configuration settings
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GeneralConfig {
    /// Default provider to use when no routing rule matches
    #[serde(default)]
    pub default_provider: Option<String>,
    /// Log retention in days (1-30, default: 7)
    #[serde(default = "default_log_retention_days")]
    pub log_retention_days: u32,
    /// Enable performance logging (default: false)
    #[serde(default)]
    pub enable_performance_logging: bool,
}

fn default_log_retention_days() -> u32 {
    7 // Keep logs for 7 days by default
}

/// Shortcuts configuration (Phase 6 - Task 4.2)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutsConfig {
    /// Global summon hotkey (e.g., "Command+Grave")
    pub summon: String,
    /// Cancel operation hotkey (optional)
    #[serde(default)]
    pub cancel: Option<String>,
}

impl Default for ShortcutsConfig {
    fn default() -> Self {
        Self {
            summon: "Command+Grave".to_string(),
            cancel: Some("Escape".to_string()),
        }
    }
}

/// Behavior configuration (Phase 6 - Task 5.1)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BehaviorConfig {
    /// Input mode: "cut" or "copy"
    #[serde(default = "default_input_mode")]
    pub input_mode: String,
    /// Output mode: "typewriter" or "instant"
    #[serde(default = "default_output_mode")]
    pub output_mode: String,
    /// Typing speed in characters per second (10-200)
    #[serde(default = "default_typing_speed")]
    pub typing_speed: u32,
    /// Enable PII scrubbing (email, phone, SSN, etc.)
    #[serde(default)]
    pub pii_scrubbing_enabled: bool,
}

fn default_input_mode() -> String {
    "cut".to_string()
}

fn default_output_mode() -> String {
    "typewriter".to_string()
}

fn default_typing_speed() -> u32 {
    50 // 50 characters per second
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            input_mode: default_input_mode(),
            output_mode: default_output_mode(),
            typing_speed: default_typing_speed(),
            pii_scrubbing_enabled: false,
        }
    }
}

/// Provider config entry with name (for UniFFI)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfigEntry {
    pub name: String,
    #[serde(flatten)]
    pub config: ProviderConfig,
}

/// Test connection result (for provider connection testing)
#[derive(Debug, Clone)]
pub struct TestConnectionResult {
    pub success: bool,
    pub message: String,
}

/// Full configuration exposed through UniFFI
/// This wraps Config with a flattened provider list
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullConfig {
    pub default_hotkey: String,
    pub general: GeneralConfig,
    pub memory: MemoryConfig,
    pub providers: Vec<ProviderConfigEntry>,
    pub rules: Vec<RoutingRuleConfig>,
    #[serde(default)]
    pub shortcuts: Option<ShortcutsConfig>,
    #[serde(default)]
    pub behavior: Option<BehaviorConfig>,
}

impl From<Config> for FullConfig {
    fn from(config: Config) -> Self {
        let providers = config
            .providers
            .into_iter()
            .map(|(name, config)| ProviderConfigEntry { name, config })
            .collect();

        Self {
            default_hotkey: config.default_hotkey,
            general: config.general,
            memory: config.memory,
            providers,
            rules: config.rules,
            shortcuts: config.shortcuts,
            behavior: config.behavior,
        }
    }
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
    /// Provider type: "openai", "claude", "gemini", "ollama", or custom name
    /// If not specified, inferred from provider name in config
    #[serde(default)]
    pub provider_type: Option<String>,
    /// API key for cloud providers (required for OpenAI, Claude, Gemini)
    #[serde(default)]
    pub api_key: Option<String>,
    /// Model name (e.g., "gpt-4o", "claude-3-5-sonnet-20241022", "gemini-3-flash", "llama3.2")
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
    /// Whether the provider is enabled/active
    #[serde(default = "default_provider_enabled")]
    pub enabled: bool,

    // Common generation parameters
    /// Maximum tokens in response (optional)
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Temperature for response randomness (0.0-2.0 for OpenAI/Gemini, 0.0-1.0 for Claude)
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Top-p nucleus sampling (0.0-1.0, optional)
    #[serde(default)]
    pub top_p: Option<f32>,
    /// Top-k sampling (integer, optional, used by Claude, Gemini, Ollama)
    #[serde(default)]
    pub top_k: Option<u32>,

    // OpenAI-specific parameters
    /// Frequency penalty (-2.0 to 2.0, OpenAI only)
    #[serde(default)]
    pub frequency_penalty: Option<f32>,
    /// Presence penalty (-2.0 to 2.0, OpenAI only)
    #[serde(default)]
    pub presence_penalty: Option<f32>,

    // Claude/Gemini/Ollama-specific parameters
    /// Stop sequences (comma-separated, Claude/Gemini/Ollama)
    #[serde(default)]
    pub stop_sequences: Option<String>,

    // Gemini-specific parameters
    /// Thinking level for Gemini 3 models (LOW or HIGH)
    #[serde(default)]
    pub thinking_level: Option<String>,
    /// Media resolution for Gemini (LOW, MEDIUM, HIGH)
    #[serde(default)]
    pub media_resolution: Option<String>,

    // Ollama-specific parameters
    /// Repeat penalty for Ollama (default 1.1)
    #[serde(default)]
    pub repeat_penalty: Option<f32>,
}

fn default_provider_color() -> String {
    "#808080".to_string() // Gray as default
}

fn default_timeout_seconds() -> u64 {
    30 // 30 seconds default timeout
}

fn default_provider_enabled() -> bool {
    true // Providers are enabled by default when added
}

impl ProviderConfig {
    /// Infer provider type from config
    ///
    /// If `provider_type` is explicitly set, use it.
    /// Otherwise, infer from provider name:
    /// - "openai" -> "openai"
    /// - "claude" -> "claude"
    /// - "gemini" -> "gemini"
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
        } else if name_lower.contains("gemini") || name_lower.contains("google") {
            "gemini".to_string()
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
            default_hotkey: "Grave".to_string(),  // Single ` key (backtick) for quick access
            general: GeneralConfig::default(),
            memory: MemoryConfig::default(),
            providers: HashMap::new(),
            rules: Vec::new(),
            shortcuts: Some(ShortcutsConfig::default()),
            behavior: Some(BehaviorConfig::default()),
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

        debug!(path = %path.display(), "Attempting to load config from file");

        // Check if file exists
        if !path.exists() {
            error!(path = %path.display(), "Config file not found");
            return Err(AetherError::invalid_config(format!(
                "Config file not found: {}",
                path.display()
            )));
        }

        // Read file contents
        let contents = fs::read_to_string(path).map_err(|e| {
            error!(path = %path.display(), error = %e, "Failed to read config file");
            AetherError::invalid_config(format!(
                "Failed to read config file {}: {}",
                path.display(),
                e
            ))
        })?;

        debug!(
            path = %path.display(),
            size_bytes = contents.len(),
            "Config file read successfully, parsing TOML"
        );

        // Parse TOML
        let config: Config = toml::from_str(&contents).map_err(|e| {
            error!(path = %path.display(), error = %e, "Failed to parse config TOML");
            AetherError::invalid_config(format!(
                "Failed to parse config file {}: {}",
                path.display(),
                e
            ))
        })?;

        debug!(
            path = %path.display(),
            providers_count = config.providers.len(),
            rules_count = config.rules.len(),
            "Config parsed successfully, validating"
        );

        // Validate config
        config.validate()?;

        info!(
            path = %path.display(),
            providers_count = config.providers.len(),
            rules_count = config.rules.len(),
            memory_enabled = config.memory.enabled,
            "Config loaded and validated successfully"
        );

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

        debug!(path = %path.display(), "Loading config from default path");

        if path.exists() {
            info!(path = %path.display(), "Found config file, loading");
            Self::load_from_file(&path)
        } else {
            info!(
                path = %path.display(),
                "Config file not found, using default configuration"
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
        debug!(
            providers_count = self.providers.len(),
            rules_count = self.rules.len(),
            "Starting config validation"
        );

        // Validate default provider exists
        if let Some(ref default_provider) = self.general.default_provider {
            if !self.providers.contains_key(default_provider) {
                error!(default_provider = %default_provider, "Default provider not found");
                return Err(AetherError::invalid_config(format!(
                    "Default provider '{}' not found in providers",
                    default_provider
                )));
            }
            debug!(default_provider = %default_provider, "Default provider validated");
        }

        // Validate provider configurations
        for (name, provider) in &self.providers {
            let provider_type = provider.infer_provider_type(name);

            // Check API key for cloud providers (not required for Ollama)
            if (provider_type == "openai" || provider_type == "claude" || provider_type == "gemini")
                && provider.api_key.is_none()
            {
                error!(provider = %name, provider_type = %provider_type, "Provider missing API key");
                return Err(AetherError::invalid_config(format!(
                    "Provider '{}' requires an API key",
                    name
                )));
            }

            // Validate timeout
            if provider.timeout_seconds == 0 {
                error!(provider = %name, "Provider timeout is zero");
                return Err(AetherError::invalid_config(format!(
                    "Provider '{}' timeout must be greater than 0",
                    name
                )));
            }

            // Validate temperature if specified (provider-specific ranges)
            if let Some(temp) = provider.temperature {
                let (min, max, provider_name): (f32, f32, &str) = match provider_type.as_str() {
                    "claude" => (0.0, 1.0, "Claude"),
                    "openai" => (0.0, 2.0, "OpenAI"),
                    "gemini" => (0.0, 2.0, "Gemini"),
                    "ollama" => (0.0, f32::MAX, "Ollama"),
                    _ => (0.0, 2.0, "Custom"),
                };

                if !(min..=max).contains(&temp) {
                    error!(provider = %name, temperature = temp, "Invalid temperature for {}", provider_name);
                    return Err(AetherError::invalid_config(format!(
                        "Provider '{}' ({}) temperature must be between {} and {}, got {}",
                        name, provider_name, min, max, temp
                    )));
                }
            }

            // Validate max_tokens if specified
            if let Some(max_tokens) = provider.max_tokens {
                if max_tokens == 0 {
                    error!(provider = %name, max_tokens = max_tokens, "Invalid max_tokens");
                    return Err(AetherError::invalid_config(format!(
                        "Provider '{}' max_tokens must be greater than 0, got {}",
                        name, max_tokens
                    )));
                }
            }

            // Validate top_p if specified
            if let Some(top_p) = provider.top_p {
                if !(0.0..=1.0).contains(&top_p) {
                    error!(provider = %name, top_p = top_p, "Invalid top_p");
                    return Err(AetherError::invalid_config(format!(
                        "Provider '{}' top_p must be between 0.0 and 1.0, got {}",
                        name, top_p
                    )));
                }
            }

            // Validate top_k if specified
            if let Some(top_k) = provider.top_k {
                if top_k == 0 {
                    error!(provider = %name, top_k = top_k, "Invalid top_k");
                    return Err(AetherError::invalid_config(format!(
                        "Provider '{}' top_k must be greater than 0, got {}",
                        name, top_k
                    )));
                }
            }

            // Validate OpenAI-specific parameters
            if provider_type == "openai" {
                if let Some(freq_pen) = provider.frequency_penalty {
                    if !(-2.0..=2.0).contains(&freq_pen) {
                        error!(provider = %name, frequency_penalty = freq_pen, "Invalid frequency_penalty");
                        return Err(AetherError::invalid_config(format!(
                            "Provider '{}' frequency_penalty must be between -2.0 and 2.0, got {}",
                            name, freq_pen
                        )));
                    }
                }

                if let Some(pres_pen) = provider.presence_penalty {
                    if !(-2.0..=2.0).contains(&pres_pen) {
                        error!(provider = %name, presence_penalty = pres_pen, "Invalid presence_penalty");
                        return Err(AetherError::invalid_config(format!(
                            "Provider '{}' presence_penalty must be between -2.0 and 2.0, got {}",
                            name, pres_pen
                        )));
                    }
                }
            }

            // Validate Gemini-specific parameters
            if provider_type == "gemini" {
                if let Some(ref thinking_level) = provider.thinking_level {
                    if thinking_level != "LOW" && thinking_level != "HIGH" {
                        error!(provider = %name, thinking_level = %thinking_level, "Invalid thinking_level");
                        return Err(AetherError::invalid_config(format!(
                            "Provider '{}' thinking_level must be 'LOW' or 'HIGH', got '{}'",
                            name, thinking_level
                        )));
                    }
                }

                if let Some(ref media_res) = provider.media_resolution {
                    if media_res != "LOW" && media_res != "MEDIUM" && media_res != "HIGH" {
                        error!(provider = %name, media_resolution = %media_res, "Invalid media_resolution");
                        return Err(AetherError::invalid_config(format!(
                            "Provider '{}' media_resolution must be 'LOW', 'MEDIUM', or 'HIGH', got '{}'",
                            name, media_res
                        )));
                    }
                }
            }

            // Validate Ollama-specific parameters
            if provider_type == "ollama" {
                if let Some(repeat_pen) = provider.repeat_penalty {
                    if repeat_pen < 0.0 {
                        error!(provider = %name, repeat_penalty = repeat_pen, "Invalid repeat_penalty");
                        return Err(AetherError::invalid_config(format!(
                            "Provider '{}' repeat_penalty must be >= 0.0, got {}",
                            name, repeat_pen
                        )));
                    }
                }
            }

            debug!(
                provider = %name,
                provider_type = %provider_type,
                timeout_seconds = provider.timeout_seconds,
                "Provider validated"
            );
        }

        // Validate routing rules
        for (idx, rule) in self.rules.iter().enumerate() {
            // Check provider exists
            if !self.providers.contains_key(&rule.provider) {
                error!(
                    rule_index = idx + 1,
                    provider = %rule.provider,
                    "Rule references unknown provider"
                );
                return Err(AetherError::invalid_config(format!(
                    "Rule #{} references unknown provider '{}'",
                    idx + 1,
                    rule.provider
                )));
            }

            // Validate regex pattern
            if let Err(e) = regex::Regex::new(&rule.regex) {
                error!(
                    rule_index = idx + 1,
                    regex = %rule.regex,
                    error = %e,
                    "Invalid regex pattern"
                );
                return Err(AetherError::invalid_config(format!(
                    "Rule #{} has invalid regex '{}': {}",
                    idx + 1,
                    rule.regex,
                    e
                )));
            }

            debug!(
                rule_index = idx + 1,
                provider = %rule.provider,
                regex = %rule.regex,
                "Routing rule validated"
            );
        }

        // Validate memory config
        if self.memory.max_context_items == 0 {
            error!("Memory max_context_items is zero");
            return Err(AetherError::invalid_config(
                "memory.max_context_items must be greater than 0",
            ));
        }

        if !(0.0..=1.0).contains(&self.memory.similarity_threshold) {
            error!(threshold = self.memory.similarity_threshold, "Invalid similarity threshold");
            return Err(AetherError::invalid_config(format!(
                "memory.similarity_threshold must be between 0.0 and 1.0, got {}",
                self.memory.similarity_threshold
            )));
        }

        debug!(
            memory_enabled = self.memory.enabled,
            max_context_items = self.memory.max_context_items,
            similarity_threshold = self.memory.similarity_threshold,
            "Memory config validated"
        );

        info!(
            providers_count = self.providers.len(),
            rules_count = self.rules.len(),
            "Config validation completed successfully"
        );

        Ok(())
    }

    /// Save configuration to a TOML file with atomic write
    ///
    /// This method uses atomic write operation to prevent corruption:
    /// 1. Write to temporary file (.tmp suffix)
    /// 2. fsync() to ensure data is on disk
    /// 3. Atomic rename to target path
    ///
    /// This ensures that the config file is never in a partially written state,
    /// even if the application crashes or loses power during the write.
    ///
    /// # Arguments
    /// * `path` - Target path for config file
    ///
    /// # Errors
    /// * `AetherError::InvalidConfig` - Failed to serialize or write config
    ///
    /// # Example
    /// ```no_run
    /// let config = Config::default();
    /// config.save_to_file("config.toml")?;
    /// ```
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let path = path.as_ref();

        debug!(
            path = %path.display(),
            providers_count = self.providers.len(),
            rules_count = self.rules.len(),
            "Attempting to save config"
        );

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                error!(directory = %parent.display(), error = %e, "Failed to create config directory");
                AetherError::invalid_config(format!(
                    "Failed to create config directory {}: {}",
                    parent.display(),
                    e
                ))
            })?;
            debug!(directory = %parent.display(), "Config directory ensured");
        }

        // Serialize to TOML
        let contents = toml::to_string_pretty(self).map_err(|e| {
            error!(error = %e, "Failed to serialize config to TOML");
            AetherError::invalid_config(format!("Failed to serialize config: {}", e))
        })?;

        debug!(
            size_bytes = contents.len(),
            lines = contents.lines().count(),
            "Config serialized to TOML"
        );

        // Create temporary file in the same directory (atomic rename requirement)
        let temp_path = path.with_extension("tmp");

        // Write to temp file
        fs::write(&temp_path, &contents).map_err(|e| {
            error!(temp_path = %temp_path.display(), error = %e, "Failed to write temp file");
            AetherError::invalid_config(format!(
                "Failed to write temp config file {}: {}",
                temp_path.display(),
                e
            ))
        })?;

        debug!(temp_path = %temp_path.display(), "Wrote config to temp file");

        // fsync the temp file to ensure data is on disk
        #[cfg(unix)]
        {
            let file = std::fs::OpenOptions::new()
                .write(true)
                .open(&temp_path)
                .map_err(|e| {
                    error!(temp_path = %temp_path.display(), error = %e, "Failed to open temp file for fsync");
                    AetherError::invalid_config(format!(
                        "Failed to open temp file for fsync: {}",
                        e
                    ))
                })?;

            // Sync file data and metadata
            file.sync_all().map_err(|e| {
                error!(temp_path = %temp_path.display(), error = %e, "Failed to fsync temp file");
                AetherError::invalid_config(format!("Failed to fsync temp file: {}", e))
            })?;

            debug!(temp_path = %temp_path.display(), "Fsynced temp file to disk");
        }

        // Atomic rename (overwrites target if exists)
        fs::rename(&temp_path, path).map_err(|e| {
            error!(
                temp_path = %temp_path.display(),
                target_path = %path.display(),
                error = %e,
                "Failed to atomically rename temp file"
            );
            // Clean up temp file on error
            let _ = fs::remove_file(&temp_path);
            AetherError::invalid_config(format!(
                "Failed to rename temp config to {}: {}",
                path.display(),
                e
            ))
        })?;

        info!(
            path = %path.display(),
            size_bytes = contents.len(),
            "Config saved successfully with atomic write"
        );

        Ok(())
    }

    /// Save configuration to default path with atomic write
    ///
    /// This is a convenience method that saves to ~/.config/aether/config.toml
    /// using atomic write operation.
    ///
    /// # Example
    /// ```no_run
    /// let mut config = Config::default();
    /// config.default_hotkey = "Command+Shift+A".to_string();
    /// config.save()?;
    /// ```
    pub fn save(&self) -> Result<()> {
        self.save_to_file(Self::default_path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.default_hotkey, "Grave");  // Single ` key
        assert!(config.memory.enabled);
    }

    #[test]
    fn test_new_config() {
        let config = Config::new();
        assert_eq!(config.default_hotkey, "Grave");  // Single ` key
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
        let json = r#"{"default_hotkey":"Grave"}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.default_hotkey, "Grave");
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
default_hotkey = "Grave"

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
        assert_eq!(config.default_hotkey, "Grave");  // Single ` key
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

    // Additional comprehensive tests for Phase 6 - Task 8.1

    #[test]
    fn test_regex_validation_valid_patterns() {
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

        // Test various valid regex patterns
        let valid_patterns = vec![
            ".*",                    // Match all
            "^/code",                // Start with /code
            "\\d+",                  // One or more digits
            "hello|world",           // Alternatives
            "[a-zA-Z]+",             // Character class
            "^test$",                // Exact match
            "(foo|bar)\\s+\\w+",     // Groups and word characters
        ];

        for pattern in valid_patterns {
            config.rules = vec![RoutingRuleConfig {
                regex: pattern.to_string(),
                provider: "openai".to_string(),
                system_prompt: None,
            }];
            assert!(
                config.validate().is_ok(),
                "Pattern '{}' should be valid",
                pattern
            );
        }
    }

    #[test]
    fn test_regex_validation_invalid_patterns() {
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

        // Test various invalid regex patterns
        let invalid_patterns = vec![
            "[invalid(",          // Unclosed bracket
            "(unclosed",          // Unclosed parenthesis
            "**",                 // Invalid quantifier
            "(?P<invalid",        // Unclosed named group
            "[z-a]",              // Invalid range
        ];

        for pattern in invalid_patterns {
            config.rules = vec![RoutingRuleConfig {
                regex: pattern.to_string(),
                provider: "openai".to_string(),
                system_prompt: None,
            }];
            assert!(
                config.validate().is_err(),
                "Pattern '{}' should be invalid",
                pattern
            );
        }
    }

    #[test]
    fn test_shortcuts_config_defaults() {
        let shortcuts = ShortcutsConfig::default();
        assert_eq!(shortcuts.summon, "Command+Grave");
        assert_eq!(shortcuts.cancel, Some("Escape".to_string()));
    }

    #[test]
    fn test_shortcuts_config_serialization() {
        let shortcuts = ShortcutsConfig {
            summon: "Command+Shift+A".to_string(),
            cancel: Some("Escape".to_string()),
        };
        let json = serde_json::to_string(&shortcuts).unwrap();
        assert!(json.contains("Command+Shift+A"));
        assert!(json.contains("Escape"));
    }

    #[test]
    fn test_behavior_config_defaults() {
        let behavior = BehaviorConfig::default();
        assert_eq!(behavior.input_mode, "cut");
        assert_eq!(behavior.output_mode, "typewriter");
        assert_eq!(behavior.typing_speed, 50);
        assert!(!behavior.pii_scrubbing_enabled);
    }

    #[test]
    fn test_behavior_config_serialization() {
        let behavior = BehaviorConfig {
            input_mode: "copy".to_string(),
            output_mode: "instant".to_string(),
            typing_speed: 100,
            pii_scrubbing_enabled: true,
        };
        let json = serde_json::to_string(&behavior).unwrap();
        assert!(json.contains("copy"));
        assert!(json.contains("instant"));
        assert!(json.contains("100"));
        assert!(json.contains("true"));
    }

    #[test]
    fn test_atomic_write_creates_parent_directory() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let nested_path = temp_dir.path().join("nested").join("config.toml");

        let config = Config::default();
        config.save_to_file(&nested_path).unwrap();

        assert!(nested_path.exists());
    }

    #[test]
    fn test_atomic_write_overwrites_existing_file() {
        use tempfile::NamedTempFile;

        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path();

        // Write first config
        let mut config1 = Config::default();
        config1.default_hotkey = "Command+A".to_string();
        config1.save_to_file(path).unwrap();

        // Overwrite with second config
        let mut config2 = Config::default();
        config2.default_hotkey = "Command+B".to_string();
        config2.save_to_file(path).unwrap();

        // Load and verify
        let loaded = Config::load_from_file(path).unwrap();
        assert_eq!(loaded.default_hotkey, "Command+B");
    }

    #[test]
    fn test_config_validation_zero_timeout() {
        let mut config = Config::default();

        // Add provider with zero timeout
        let provider = ProviderConfig {
            provider_type: Some("openai".to_string()),
            api_key: Some("sk-test".to_string()),
            model: "gpt-4o".to_string(),
            base_url: None,
            color: "#10a37f".to_string(),
            timeout_seconds: 0,
            max_tokens: Some(4096),
            temperature: Some(0.7),
        };
        config.providers.insert("openai".to_string(), provider);

        // Should fail validation
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("timeout must be greater than 0"));
    }

    #[test]
    fn test_config_validation_memory_zero_max_context() {
        let mut config = Config::default();
        config.memory.max_context_items = 0;

        // Should fail validation
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("max_context_items must be greater than 0"));
    }

    #[test]
    fn test_config_validation_memory_invalid_similarity() {
        let mut config = Config::default();
        config.memory.similarity_threshold = 1.5; // > 1.0

        // Should fail validation
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("similarity_threshold must be between 0.0 and 1.0"));
    }

    #[test]
    fn test_provider_type_inference() {
        let provider = ProviderConfig {
            provider_type: None,
            api_key: Some("test".to_string()),
            model: "test-model".to_string(),
            base_url: None,
            color: "#000000".to_string(),
            timeout_seconds: 30,
            max_tokens: None,
            temperature: None,
        };

        // Test inference from provider name
        assert_eq!(provider.infer_provider_type("openai"), "openai");
        assert_eq!(provider.infer_provider_type("claude"), "claude");
        assert_eq!(provider.infer_provider_type("ollama"), "ollama");
        assert_eq!(provider.infer_provider_type("deepseek"), "openai"); // OpenAI-compatible
        assert_eq!(provider.infer_provider_type("custom"), "openai"); // Default
    }

    #[test]
    fn test_provider_type_explicit_override() {
        let provider = ProviderConfig {
            provider_type: Some("custom".to_string()),
            api_key: Some("test".to_string()),
            model: "test-model".to_string(),
            base_url: None,
            color: "#000000".to_string(),
            timeout_seconds: 30,
            max_tokens: None,
            temperature: None,
        };

        // Explicit type should override inference
        assert_eq!(provider.infer_provider_type("openai"), "custom");
    }

    #[test]
    fn test_full_config_conversion() {
        let mut config = Config::default();

        // Add providers
        let provider1 = ProviderConfig {
            provider_type: Some("openai".to_string()),
            api_key: Some("sk-test".to_string()),
            model: "gpt-4o".to_string(),
            base_url: None,
            color: "#10a37f".to_string(),
            timeout_seconds: 30,
            max_tokens: Some(4096),
            temperature: Some(0.7),
        };
        config.providers.insert("openai".to_string(), provider1);

        let provider2 = ProviderConfig {
            provider_type: Some("claude".to_string()),
            api_key: Some("sk-ant-test".to_string()),
            model: "claude-3-5-sonnet-20241022".to_string(),
            base_url: None,
            color: "#d97757".to_string(),
            timeout_seconds: 30,
            max_tokens: Some(4096),
            temperature: Some(0.7),
        };
        config.providers.insert("claude".to_string(), provider2);

        // Convert to FullConfig
        let full_config: FullConfig = config.into();

        // Verify conversion
        assert_eq!(full_config.providers.len(), 2);
        assert!(full_config
            .providers
            .iter()
            .any(|p| p.name == "openai"));
        assert!(full_config
            .providers
            .iter()
            .any(|p| p.name == "claude"));
    }

    #[test]
    fn test_config_toml_round_trip() {
        let mut config = Config::default();

        // Add comprehensive configuration
        config.shortcuts = Some(ShortcutsConfig {
            summon: "Command+Shift+A".to_string(),
            cancel: Some("Escape".to_string()),
        });

        config.behavior = Some(BehaviorConfig {
            input_mode: "copy".to_string(),
            output_mode: "instant".to_string(),
            typing_speed: 100,
            pii_scrubbing_enabled: true,
        });

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

        config.rules.push(RoutingRuleConfig {
            regex: "^/code".to_string(),
            provider: "openai".to_string(),
            system_prompt: Some("You are a coding assistant.".to_string()),
        });

        // Serialize to TOML
        let toml_str = toml::to_string_pretty(&config).unwrap();

        // Deserialize back
        let deserialized: Config = toml::from_str(&toml_str).unwrap();

        // Verify all fields
        assert_eq!(deserialized.default_hotkey, config.default_hotkey);
        assert_eq!(
            deserialized.shortcuts.as_ref().unwrap().summon,
            "Command+Shift+A"
        );
        assert_eq!(
            deserialized.behavior.as_ref().unwrap().input_mode,
            "copy"
        );
        assert_eq!(deserialized.providers.len(), 1);
        assert_eq!(deserialized.rules.len(), 1);
        assert!(deserialized.validate().is_ok());
    }
}
