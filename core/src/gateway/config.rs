//! Gateway Configuration
//!
//! Parses and manages the Gateway configuration from TOML files.
//! Supports multi-agent setup, channel bindings, and extended features.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::info;

use super::agent_instance::AgentInstanceConfig;
use crate::config::PrivacyConfig;

/// Root Gateway configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GatewayConfig {
    /// Gateway server settings
    pub gateway: GatewayServerConfig,

    /// Agent configurations (keyed by agent_id)
    #[serde(default)]
    pub agents: HashMap<String, AgentConfig>,

    /// Channel bindings (pattern -> agent_id)
    #[serde(default)]
    pub bindings: HashMap<String, String>,

    /// Channel connector configurations (parsed by app config, ignored here)
    #[serde(default)]
    pub channels: serde_json::Value,

    /// Sandbox configuration
    #[serde(default)]
    pub sandbox: SandboxConfig,

    /// Tool configurations
    #[serde(default)]
    pub tools: ToolsConfig,

    /// Privacy and PII filtering configuration
    #[serde(default)]
    pub privacy: PrivacyConfig,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        let mut agents = HashMap::new();
        agents.insert("main".to_string(), AgentConfig::default());

        Self {
            gateway: GatewayServerConfig::default(),
            agents,
            bindings: HashMap::new(),
            channels: serde_json::Value::Object(serde_json::Map::new()),
            sandbox: SandboxConfig::default(),
            tools: ToolsConfig::default(),
            privacy: PrivacyConfig::default(),
        }
    }
}

/// Gateway server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GatewayServerConfig {
    /// Bind address
    pub host: String,
    /// Port number
    pub port: u16,
    /// Maximum concurrent connections
    pub max_connections: usize,
    /// Legacy field — kept for TOML backward compat
    #[serde(default)]
    pub require_auth: bool,
    /// Protocol version
    pub protocol_version: u32,
    /// Authentication configuration
    #[serde(default)]
    pub auth: AuthConfig,
}

impl Default for GatewayServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 18790,
            max_connections: 100,
            require_auth: false,
            protocol_version: 1,
            auth: AuthConfig::default(),
        }
    }
}

/// Authentication mode
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    /// Require shared token for access (default)
    #[default]
    Token,
    /// No authentication required
    None,
}

impl AuthMode {
    /// Whether this mode requires authentication
    pub fn is_auth_required(&self) -> bool {
        matches!(self, AuthMode::Token)
    }
}

/// Authentication configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// Authentication mode
    pub mode: AuthMode,
    /// HTTP session cookie expiry (hours)
    pub session_expiry_hours: u64,
    /// Device token expiry (hours)
    pub token_expiry_hours: u64,
    /// Allowed WebSocket origins (additional to same-origin)
    #[serde(default)]
    pub allowed_origins: Vec<String>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            mode: AuthMode::Token,
            session_expiry_hours: 72,
            token_expiry_hours: 24,
            allowed_origins: vec![],
        }
    }
}

impl AuthConfig {
    /// Whether authentication is required
    pub fn is_auth_required(&self) -> bool {
        matches!(self.mode, AuthMode::Token)
    }
}

/// Agent configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// Workspace directory (supports ~ expansion)
    pub workspace: String,
    /// Primary model
    pub model: String,
    /// Fallback models
    #[serde(default)]
    pub fallback_models: Vec<String>,
    /// Maximum loop iterations
    pub max_loops: u32,
    /// Maximum total token usage per request (loop guard)
    #[serde(default)]
    pub max_tokens: Option<usize>,
    /// Custom system prompt
    pub system_prompt: Option<String>,
    /// Tool whitelist (empty = all allowed)
    #[serde(default)]
    pub tool_whitelist: Vec<String>,
    /// Tool blacklist
    #[serde(default)]
    pub tool_blacklist: Vec<String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            workspace: "~/.aleph/workspaces/main".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            fallback_models: vec![],
            max_loops: 50,
            max_tokens: None,
            system_prompt: None,
            tool_whitelist: vec![],
            tool_blacklist: vec![],
        }
    }
}

impl AgentConfig {
    /// Convert to AgentInstanceConfig
    pub fn to_instance_config(&self, agent_id: &str) -> AgentInstanceConfig {
        AgentInstanceConfig {
            agent_id: agent_id.to_string(),
            display_name: None,
            workspace: expand_path(&self.workspace),
            model: self.model.clone(),
            fallback_models: self.fallback_models.clone(),
            max_loops: self.max_loops,
            max_tokens: self.max_tokens,
            system_prompt: self.system_prompt.clone(),
            tool_whitelist: self.tool_whitelist.clone(),
            tool_blacklist: self.tool_blacklist.clone(),
            agent_dir: dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
                .join(format!(".aleph/agents/{}", agent_id)),
        }
    }
}

// Channel connector configurations have been unified into the app Config system
// (Config.channels: HashMap<String, Value>). GatewayConfig.channels is kept as
// a raw Value to avoid parse errors — the actual parsing happens in
// Config::resolved_channels().


/// Sandbox configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxConfig {
    /// Enable Docker sandbox
    pub enabled: bool,
    /// Docker image for sandbox
    pub docker_image: String,
    /// Memory limit in MB
    pub memory_limit_mb: u64,
    /// CPU quota percentage
    pub cpu_quota_percent: u32,
    /// Network mode
    pub network_mode: NetworkMode,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            docker_image: "aleph-sandbox:latest".to_string(),
            memory_limit_mb: 512,
            cpu_quota_percent: 50,
            network_mode: NetworkMode::Restricted,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum NetworkMode {
    None,
    #[default]
    Restricted,
    Full,
}

/// Tools configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolsConfig {
    /// Chrome CDP configuration
    pub chrome: Option<ChromeConfig>,
    /// Cron scheduler configuration
    pub cron: Option<CronConfig>,
    /// Webhook listener configuration
    pub webhook: Option<WebhookConfig>,
}

/// Chrome CDP configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChromeConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub executable_path: Option<String>,
    #[serde(default = "default_false")]
    pub headless: bool,
}

/// Cron scheduler configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CronConfig {
    pub enabled: bool,
    pub max_jobs: usize,
}

impl Default for CronConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_jobs: 100,
        }
    }
}

/// Webhook listener configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebhookConfig {
    pub enabled: bool,
    pub port: u16,
    pub max_endpoints: usize,
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 18791,
            max_endpoints: 50,
        }
    }
}

// Helper functions for serde defaults
fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

impl GatewayConfig {
    /// Load configuration from a TOML file
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let content = std::fs::read_to_string(path).map_err(|e| {
            ConfigError::LoadFailed(format!("Failed to read {}: {}", path.display(), e))
        })?;

        Self::from_toml(&content)
    }

    /// Parse configuration from TOML string
    pub fn from_toml(content: &str) -> Result<Self, ConfigError> {
        let config: GatewayConfig =
            toml::from_str(content).map_err(|e| ConfigError::ParseFailed(e.to_string()))?;

        // Validate configuration
        config.validate()?;

        Ok(config)
    }

    /// Load from default location (~/.aleph/config.toml)
    pub fn load_default() -> Result<Self, ConfigError> {
        let config_path = dirs::home_dir()
            .ok_or_else(|| ConfigError::LoadFailed("No home directory".to_string()))?
            .join(".aleph/config.toml");

        if config_path.exists() {
            Self::load(&config_path)
        } else {
            info!("No config file found, using defaults");
            Ok(Self::default())
        }
    }

    /// Validate the configuration
    fn validate(&self) -> Result<(), ConfigError> {
        // Validate port numbers
        if self.gateway.port == 0 {
            return Err(ConfigError::Invalid("Gateway port cannot be 0".to_string()));
        }

        // Validate at least one agent exists
        if self.agents.is_empty() {
            return Err(ConfigError::Invalid(
                "At least one agent must be configured".to_string(),
            ));
        }

        // Validate bindings reference existing agents
        for (pattern, agent_id) in &self.bindings {
            if !self.agents.contains_key(agent_id) {
                return Err(ConfigError::Invalid(format!(
                    "Binding '{}' references unknown agent '{}'",
                    pattern, agent_id
                )));
            }
        }

        Ok(())
    }

    /// Get agent configs as instance configs
    pub fn get_agent_instance_configs(&self) -> Vec<AgentInstanceConfig> {
        self.agents
            .iter()
            .map(|(id, cfg)| cfg.to_instance_config(id))
            .collect()
    }

    /// Get the default agent ID (first one, or "main" if exists)
    pub fn default_agent_id(&self) -> Option<&str> {
        if self.agents.contains_key("main") {
            Some("main")
        } else {
            self.agents.keys().next().map(|s| s.as_str())
        }
    }
}

/// Configuration errors
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Failed to load config: {0}")]
    LoadFailed(String),

    #[error("Failed to parse config: {0}")]
    ParseFailed(String),

    #[error("Invalid config: {0}")]
    Invalid(String),
}

/// Expand ~ in paths
fn expand_path(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

/// Expand ${ENV_VAR} in strings
#[cfg(test)]
fn expand_env_var(s: &str) -> String {
    let mut result = s.to_string();
    let mut search_from = 0;

    // Find ${...} patterns, advancing past substituted values to prevent infinite loops
    while let Some(rel_start) = result[search_from..].find("${") {
        let start = search_from + rel_start;
        if let Some(end) = result[start..].find('}') {
            let var_name = &result[start + 2..start + end];
            let value = std::env::var(var_name).unwrap_or_default();
            let value_len = value.len();
            result = format!("{}{}{}", &result[..start], value, &result[start + end + 1..]);
            search_from = start + value_len;
        } else {
            break;
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = GatewayConfig::default();
        assert_eq!(config.gateway.port, 18790);
        assert!(config.agents.contains_key("main"));
    }

    #[test]
    fn test_parse_minimal_config() {
        let toml = r#"
[gateway]
port = 9000

[agents.main]
model = "claude-opus-4-5"
"#;
        let config = GatewayConfig::from_toml(toml).unwrap();
        assert_eq!(config.gateway.port, 9000);
        assert_eq!(config.agents["main"].model, "claude-opus-4-5");
    }

    #[test]
    fn test_parse_full_config() {
        let toml = r#"
[gateway]
host = "0.0.0.0"
port = 18790
max_connections = 200

[agents.main]
workspace = "~/aleph-main"
model = "claude-sonnet-4-5"
max_loops = 30

[agents.work]
workspace = "~/aleph-work"
model = "claude-opus-4-5"

[bindings]
"gui:window1" = "main"
"cli:*" = "work"

[channels.telegram]
enabled = true
token = "${TELEGRAM_BOT_TOKEN}"

[sandbox]
enabled = true
docker_image = "aleph-sandbox:latest"
memory_limit_mb = 1024

[tools.chrome]
enabled = true
headless = true
"#;
        let config = GatewayConfig::from_toml(toml).unwrap();

        assert_eq!(config.agents.len(), 2);
        assert!(config.agents.contains_key("work"));
        assert_eq!(config.bindings["cli:*"], "work");
        assert!(config.channels.get("telegram").is_some());
        assert!(config.sandbox.enabled);
    }

    #[test]
    fn test_invalid_binding() {
        let toml = r#"
[agents.main]
model = "test"

[bindings]
"test" = "nonexistent"
"#;
        let result = GatewayConfig::from_toml(toml);
        assert!(result.is_err());
    }

    #[test]
    fn test_expand_path() {
        let expanded = expand_path("~/test/path");
        assert!(!expanded.to_string_lossy().starts_with("~"));
    }

    #[test]
    fn test_expand_env_var() {
        std::env::set_var("TEST_VAR", "hello");
        let result = expand_env_var("prefix_${TEST_VAR}_suffix");
        assert_eq!(result, "prefix_hello_suffix");
    }

    #[test]
    fn test_parse_auth_config() {
        let toml = r#"
[gateway]
port = 18790

[gateway.auth]
mode = "token"
session_expiry_hours = 48
token_expiry_hours = 12

[agents.main]
model = "test"
"#;
        let config = GatewayConfig::from_toml(toml).unwrap();
        assert!(matches!(config.gateway.auth.mode, AuthMode::Token));
        assert_eq!(config.gateway.auth.session_expiry_hours, 48);
        assert_eq!(config.gateway.auth.token_expiry_hours, 12);
    }

    #[test]
    fn test_auth_mode_default_is_token() {
        let config = GatewayConfig::default();
        assert!(matches!(config.gateway.auth.mode, AuthMode::Token));
    }

    #[test]
    fn test_auth_mode_none() {
        let toml = r#"
[gateway.auth]
mode = "none"

[agents.main]
model = "test"
"#;
        let config = GatewayConfig::from_toml(toml).unwrap();
        assert!(matches!(config.gateway.auth.mode, AuthMode::None));
        assert!(!config.gateway.auth.is_auth_required());
    }

    #[test]
    fn test_legacy_require_auth_compat() {
        let toml = r#"
[gateway]
port = 18790
require_auth = true

[agents.main]
model = "test"
"#;
        let config = GatewayConfig::from_toml(toml).unwrap();
        assert!(matches!(config.gateway.auth.mode, AuthMode::Token));
    }

}
