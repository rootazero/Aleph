//! Configuration API

use crate::connection::AlephConnector;
use crate::protocol::{RpcClient, RpcError};
use serde::{Deserialize, Serialize};

/// Behavior configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BehaviorConfig {
    /// Auto-apply changes without confirmation
    pub auto_apply: bool,
    /// Confirm before applying changes
    pub confirm_before_apply: bool,
    /// Maximum context tokens
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_context_tokens: Option<u32>,
}

/// Search configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    /// Enable search
    pub enabled: bool,
    /// Search provider
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// API key (write-only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

/// Policies configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoliciesConfig {
    /// Allow web browsing
    pub allow_web_browsing: bool,
    /// Allow file access
    pub allow_file_access: bool,
    /// Allow code execution
    pub allow_code_execution: bool,
}

/// Shortcuts configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutsConfig {
    /// Trigger hotkey
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_hotkey: Option<String>,
    /// Vision hotkey
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vision_hotkey: Option<String>,
}

/// Code execution configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeExecConfig {
    /// Enable code execution
    pub enabled: bool,
    /// Use sandbox
    pub sandbox: bool,
    /// Timeout in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u32>,
}

/// File operations configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileOpsConfig {
    /// Enable file operations
    pub enabled: bool,
    /// Allowed paths
    pub allowed_paths: Vec<String>,
    /// Denied paths
    pub denied_paths: Vec<String>,
}

/// Configuration API client
///
/// Provides high-level methods for managing Aleph configuration.
///
/// ## Example
///
/// ```rust,ignore
/// use aleph_ui_logic::api::ConfigApi;
/// use aleph_ui_logic::connection::create_connector;
///
/// let connector = create_connector();
/// let config = ConfigApi::new(connector);
///
/// // Get behavior config
/// let behavior = config.behavior_get().await?;
/// println!("Auto apply: {}", behavior.auto_apply);
///
/// // Update policies
/// let mut policies = config.policies_get().await?;
/// policies.allow_web_browsing = true;
/// config.policies_update(policies).await?;
/// ```
pub struct ConfigApi<C: AlephConnector> {
    client: RpcClient<C>,
}

impl<C: AlephConnector> ConfigApi<C> {
    /// Create a new Config API client
    pub fn new(connector: C) -> Self {
        Self {
            client: RpcClient::new(connector),
        }
    }

    // Behavior configuration

    /// Get behavior configuration
    pub async fn behavior_get(&self) -> Result<BehaviorConfig, RpcError> {
        #[derive(Deserialize)]
        struct Result {
            behavior: BehaviorConfig,
        }

        let result: Result = self.client.call("config.behavior.get", ()).await?;
        Ok(result.behavior)
    }

    /// Update behavior configuration
    pub async fn behavior_update(&self, config: BehaviorConfig) -> Result<bool, RpcError> {
        #[derive(Deserialize)]
        struct Result {
            ok: bool,
        }

        let result: Result = self.client.call("config.behavior.update", config).await?;
        Ok(result.ok)
    }

    // Search configuration

    /// Get search configuration
    pub async fn search_get(&self) -> Result<SearchConfig, RpcError> {
        #[derive(Deserialize)]
        struct Result {
            search: SearchConfig,
        }

        let result: Result = self.client.call("config.search.get", ()).await?;
        Ok(result.search)
    }

    /// Update search configuration
    pub async fn search_update(&self, config: SearchConfig) -> Result<bool, RpcError> {
        #[derive(Deserialize)]
        struct Result {
            ok: bool,
        }

        let result: Result = self.client.call("config.search.update", config).await?;
        Ok(result.ok)
    }

    // Policies configuration

    /// Get policies configuration
    pub async fn policies_get(&self) -> Result<PoliciesConfig, RpcError> {
        #[derive(Deserialize)]
        struct Result {
            policies: PoliciesConfig,
        }

        let result: Result = self.client.call("config.policies.get", ()).await?;
        Ok(result.policies)
    }

    /// Update policies configuration
    pub async fn policies_update(&self, config: PoliciesConfig) -> Result<bool, RpcError> {
        #[derive(Deserialize)]
        struct Result {
            ok: bool,
        }

        let result: Result = self.client.call("config.policies.update", config).await?;
        Ok(result.ok)
    }

    // Shortcuts configuration

    /// Get shortcuts configuration
    pub async fn shortcuts_get(&self) -> Result<ShortcutsConfig, RpcError> {
        #[derive(Deserialize)]
        struct Result {
            shortcuts: ShortcutsConfig,
        }

        let result: Result = self.client.call("config.shortcuts.get", ()).await?;
        Ok(result.shortcuts)
    }

    /// Update shortcuts configuration
    pub async fn shortcuts_update(&self, config: ShortcutsConfig) -> Result<bool, RpcError> {
        #[derive(Deserialize)]
        struct Result {
            ok: bool,
        }

        let result: Result = self.client.call("config.shortcuts.update", config).await?;
        Ok(result.ok)
    }

    // Security configuration

    /// Get code execution configuration
    pub async fn security_code_exec_get(&self) -> Result<CodeExecConfig, RpcError> {
        #[derive(Deserialize)]
        struct Result {
            #[serde(rename = "codeExec")]
            code_exec: CodeExecConfig,
        }

        let result: Result = self.client.call("config.security.getCodeExec", ()).await?;
        Ok(result.code_exec)
    }

    /// Update code execution configuration
    pub async fn security_code_exec_update(&self, config: CodeExecConfig) -> Result<bool, RpcError> {
        #[derive(Deserialize)]
        struct Result {
            ok: bool,
        }

        let result: Result = self
            .client
            .call("config.security.updateCodeExec", config)
            .await?;
        Ok(result.ok)
    }

    /// Get file operations configuration
    pub async fn security_file_ops_get(&self) -> Result<FileOpsConfig, RpcError> {
        #[derive(Deserialize)]
        struct Result {
            #[serde(rename = "fileOps")]
            file_ops: FileOpsConfig,
        }

        let result: Result = self.client.call("config.security.getFileOps", ()).await?;
        Ok(result.file_ops)
    }

    /// Update file operations configuration
    pub async fn security_file_ops_update(&self, config: FileOpsConfig) -> Result<bool, RpcError> {
        #[derive(Deserialize)]
        struct Result {
            ok: bool,
        }

        let result: Result = self
            .client
            .call("config.security.updateFileOps", config)
            .await?;
        Ok(result.ok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_behavior_config_serialization() {
        let config = BehaviorConfig {
            auto_apply: true,
            confirm_before_apply: false,
            max_context_tokens: Some(4096),
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: BehaviorConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.auto_apply, true);
        assert_eq!(deserialized.max_context_tokens, Some(4096));
    }

    #[test]
    fn test_policies_config_serialization() {
        let config = PoliciesConfig {
            allow_web_browsing: true,
            allow_file_access: false,
            allow_code_execution: true,
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: PoliciesConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.allow_web_browsing, true);
        assert_eq!(deserialized.allow_file_access, false);
    }
}
