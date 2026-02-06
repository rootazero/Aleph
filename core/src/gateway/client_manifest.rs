//! Client capability manifest for Server-Client architecture.
//!
//! Sent by Client during `connect` handshake to declare its capabilities,
//! environment, and execution constraints.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Complete capability declaration sent by Client at connect time.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ClientManifest {
    /// Client type identifier (e.g., "macos_native", "tauri", "cli", "web")
    pub client_type: String,

    /// Client version for protocol compatibility
    pub client_version: String,

    /// Capability declarations
    #[serde(default)]
    pub capabilities: ClientCapabilities,

    /// Runtime environment information
    #[serde(default)]
    pub environment: ClientEnvironment,
}

/// Client's tool execution capabilities.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ClientCapabilities {
    /// Supported tool categories (e.g., ["shell", "file_system", "ui"])
    #[serde(default)]
    pub tool_categories: Vec<String>,

    /// Explicitly supported specific tools (e.g., ["applescript:run"])
    #[serde(default)]
    pub specific_tools: Vec<String>,

    /// Explicitly excluded tools (e.g., ["shell:sudo"])
    #[serde(default)]
    pub excluded_tools: Vec<String>,

    /// Execution constraints
    #[serde(default)]
    pub constraints: ExecutionConstraints,

    /// Permission scopes granted by user (e.g., {"file_system": ["read:~/Documents"]})
    #[serde(default)]
    pub granted_scopes: Option<HashMap<String, Vec<String>>>,
}

/// Execution constraints for Client-side tool execution.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionConstraints {
    /// Maximum concurrent tool executions
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_tools: u32,

    /// Tool execution timeout in milliseconds
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_max_concurrent() -> u32 {
    3
}

fn default_timeout_ms() -> u64 {
    30000
}

impl Default for ExecutionConstraints {
    fn default() -> Self {
        Self {
            max_concurrent_tools: default_max_concurrent(),
            timeout_ms: default_timeout_ms(),
        }
    }
}

/// Client runtime environment information.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ClientEnvironment {
    /// Operating system (e.g., "macos", "windows", "linux", "web")
    #[serde(default)]
    pub os: String,

    /// CPU architecture (e.g., "arm64", "x86_64", "wasm")
    #[serde(default)]
    pub arch: String,

    /// Whether Client runs in a sandbox environment
    #[serde(default)]
    pub sandbox: bool,
}

impl ClientManifest {
    /// Check if Client supports a specific tool.
    ///
    /// Returns true if:
    /// - Tool is in `specific_tools`, OR
    /// - Tool's category is in `tool_categories`
    /// AND tool is NOT in `excluded_tools`
    pub fn supports_tool(&self, tool_name: &str) -> bool {
        // Check exclusion first
        if self.capabilities.excluded_tools.contains(&tool_name.to_string()) {
            return false;
        }

        // Check specific tools
        if self.capabilities.specific_tools.contains(&tool_name.to_string()) {
            return true;
        }

        // Check category (tool_name format: "category:action" or just "category")
        let category = tool_name.split(':').next().unwrap_or(tool_name);
        self.capabilities.tool_categories.contains(&category.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supports_tool_by_category() {
        let manifest = ClientManifest {
            capabilities: ClientCapabilities {
                tool_categories: vec!["shell".to_string(), "file_system".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(manifest.supports_tool("shell:exec"));
        assert!(manifest.supports_tool("shell"));
        assert!(manifest.supports_tool("file_system:read"));
        assert!(!manifest.supports_tool("network:fetch"));
    }

    #[test]
    fn test_supports_tool_by_specific() {
        let manifest = ClientManifest {
            capabilities: ClientCapabilities {
                specific_tools: vec!["applescript:run".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(manifest.supports_tool("applescript:run"));
        assert!(!manifest.supports_tool("applescript:compile"));
    }

    #[test]
    fn test_excluded_tools_override() {
        let manifest = ClientManifest {
            capabilities: ClientCapabilities {
                tool_categories: vec!["shell".to_string()],
                excluded_tools: vec!["shell:sudo".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(manifest.supports_tool("shell:exec"));
        assert!(!manifest.supports_tool("shell:sudo"));
    }

    #[test]
    fn test_default_constraints() {
        let constraints = ExecutionConstraints::default();
        assert_eq!(constraints.max_concurrent_tools, 3);
        assert_eq!(constraints.timeout_ms, 30000);
    }

    #[test]
    fn test_serde_roundtrip() {
        let manifest = ClientManifest {
            client_type: "macos_native".to_string(),
            client_version: "1.0.0".to_string(),
            capabilities: ClientCapabilities {
                tool_categories: vec!["shell".to_string()],
                constraints: ExecutionConstraints {
                    max_concurrent_tools: 5,
                    timeout_ms: 60000,
                },
                ..Default::default()
            },
            environment: ClientEnvironment {
                os: "macos".to_string(),
                arch: "arm64".to_string(),
                sandbox: false,
            },
        };

        let json = serde_json::to_string(&manifest).unwrap();
        let parsed: ClientManifest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.client_type, "macos_native");
        assert_eq!(parsed.capabilities.constraints.max_concurrent_tools, 5);
    }
}
