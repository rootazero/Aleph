//! Configuration types for aleph.jsonc

use crate::extension::types::PermissionRule;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Aleph configuration (from aleph.jsonc)
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct AlephConfig {
    /// JSON schema reference
    #[serde(rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,

    /// Plugins to load (npm packages or file:// URLs)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin: Option<Vec<String>>,

    /// Additional instruction files to include
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<Vec<String>>,

    /// Agent configuration overrides
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<HashMap<String, AgentConfigOverride>>,

    /// MCP server configurations
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp: Option<HashMap<String, McpConfig>>,

    /// Permission configurations
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission: Option<HashMap<String, PermissionRule>>,

    /// Default model (provider/model format)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Small model for tasks like title generation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub small_model: Option<String>,

    /// Default agent to use
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_agent: Option<String>,

    /// Disabled providers
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_providers: Option<Vec<String>>,

    /// Enabled providers (exclusive list)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled_providers: Option<Vec<String>>,

    /// Provider configurations
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<HashMap<String, ProviderOverride>>,

    /// Compaction settings
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compaction: Option<CompactionConfig>,

    /// Experimental features
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experimental: Option<ExperimentalConfig>,
}

/// Agent configuration override
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct AgentConfigOverride {
    /// Model to use (provider/model)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Temperature
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    /// Top P
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,

    /// System prompt override
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,

    /// Agent mode
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,

    /// Whether to hide from UI
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,

    /// UI color
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,

    /// Maximum steps
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<u32>,

    /// Description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Whether to disable this agent
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable: Option<bool>,

    /// Tool permissions (legacy)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<HashMap<String, bool>>,

    /// Permission rules
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission: Option<HashMap<String, PermissionRule>>,

    /// Provider-specific options
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<HashMap<String, serde_json::Value>>,
}

/// MCP server configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum McpConfig {
    /// Local MCP server (stdio)
    Local {
        /// Command to run
        command: Vec<String>,
        /// Environment variables
        #[serde(default, skip_serializing_if = "HashMap::is_empty")]
        environment: HashMap<String, String>,
        /// Whether enabled
        #[serde(default = "default_true")]
        enabled: bool,
        /// Timeout in ms
        #[serde(skip_serializing_if = "Option::is_none")]
        timeout: Option<u64>,
    },
    /// Remote MCP server (HTTP/SSE)
    Remote {
        /// Server URL
        url: String,
        /// Whether enabled
        #[serde(default = "default_true")]
        enabled: bool,
        /// Custom headers
        #[serde(default, skip_serializing_if = "HashMap::is_empty")]
        headers: HashMap<String, String>,
        /// OAuth configuration
        #[serde(skip_serializing_if = "Option::is_none")]
        oauth: Option<OAuthConfig>,
        /// Timeout in ms
        #[serde(skip_serializing_if = "Option::is_none")]
        timeout: Option<u64>,
    },
}

fn default_true() -> bool {
    true
}

/// OAuth configuration for MCP
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OAuthConfig {
    /// Client ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// Client secret
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub client_secret: Option<String>,
    /// Scopes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

/// Provider configuration override
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ProviderOverride {
    /// API key
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub api_key: Option<String>,
    /// Base URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Model whitelist
    #[serde(skip_serializing_if = "Option::is_none")]
    pub whitelist: Option<Vec<String>>,
    /// Model blacklist
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blacklist: Option<Vec<String>>,
    /// Additional options
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<HashMap<String, serde_json::Value>>,
}

/// Compaction settings
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct CompactionConfig {
    /// Enable automatic compaction
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto: Option<bool>,
    /// Enable pruning of old outputs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prune: Option<bool>,
}

/// Experimental features
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ExperimentalConfig {
    /// Chat max retries
    #[serde(rename = "chatMaxRetries", skip_serializing_if = "Option::is_none")]
    pub chat_max_retries: Option<u32>,
    /// Enable batch tool
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_tool: Option<bool>,
    /// Enable OpenTelemetry
    #[serde(rename = "openTelemetry", skip_serializing_if = "Option::is_none")]
    pub open_telemetry: Option<bool>,
    /// Primary-only tools
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_tools: Option<Vec<String>>,
    /// Continue loop on deny
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continue_loop_on_deny: Option<bool>,
    /// MCP timeout
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_timeout: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aether_config_deserialize() {
        let json = r#"{
            "$schema": "https://aether.ai/config.json",
            "model": "anthropic/claude-4",
            "plugin": ["@my/plugin"],
            "agent": {
                "build": {
                    "temperature": 0.7
                }
            }
        }"#;

        let config: AlephConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.model, Some("anthropic/claude-4".to_string()));
        assert!(config.plugin.unwrap().contains(&"@my/plugin".to_string()));
    }

    #[test]
    fn test_mcp_config_local() {
        let json = r#"{
            "type": "local",
            "command": ["npx", "-y", "@anthropic/mcp-server-filesystem"]
        }"#;

        let config: McpConfig = serde_json::from_str(json).unwrap();
        match config {
            McpConfig::Local { command, enabled, .. } => {
                assert_eq!(command.len(), 3);
                assert!(enabled);
            }
            _ => panic!("Expected Local config"),
        }
    }

    #[test]
    fn test_mcp_config_remote() {
        let json = r#"{
            "type": "remote",
            "url": "https://mcp.example.com/api"
        }"#;

        let config: McpConfig = serde_json::from_str(json).unwrap();
        match config {
            McpConfig::Remote { url, enabled, .. } => {
                assert_eq!(url, "https://mcp.example.com/api");
                assert!(enabled);
            }
            _ => panic!("Expected Remote config"),
        }
    }
}
