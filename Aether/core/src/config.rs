/// Configuration structure for Aether
///
/// Phase 1: Stub implementation with basic fields.
/// Phase 4: Added memory configuration support.
/// Phase 5: Added AI provider configuration support.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
}

/// General configuration settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    /// Default provider to use when no routing rule matches
    #[serde(default)]
    pub default_provider: Option<String>,
}

/// AI Provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
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
    0.7  // Minimum similarity score for real embedding models
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

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            default_provider: None,
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
        }
    }
}

impl Config {
    /// Create a new config with default values
    pub fn new() -> Self {
        Self::default()
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
        assert!(mem_config.excluded_apps.contains(&"com.apple.keychainaccess".to_string()));
        assert!(mem_config.excluded_apps.contains(&"com.agilebits.onepassword7".to_string()));
    }
}
