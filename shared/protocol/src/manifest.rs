//! Client Capability Manifest
//!
//! Types for declaring client capabilities during connection handshake.

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
    /// Create a new manifest with basic info
    pub fn new(client_type: impl Into<String>, client_version: impl Into<String>) -> Self {
        Self {
            client_type: client_type.into(),
            client_version: client_version.into(),
            ..Default::default()
        }
    }

    /// Check if Client supports a specific tool.
    ///
    /// Returns true if:
    /// - Tool is in `specific_tools`, OR
    /// - Tool's category is in `tool_categories`
    ///   AND tool is NOT in `excluded_tools`
    pub fn supports_tool(&self, tool_name: &str) -> bool {
        // Check exclusion first
        if self
            .capabilities
            .excluded_tools
            .contains(&tool_name.to_string())
        {
            return false;
        }

        // Check specific tools
        if self
            .capabilities
            .specific_tools
            .contains(&tool_name.to_string())
        {
            return true;
        }

        // Check category (tool_name format: "category:action" or just "category")
        let category = tool_name.split(':').next().unwrap_or(tool_name);
        self.capabilities
            .tool_categories
            .contains(&category.to_string())
    }

    /// Check if a specific scope is granted for a category.
    ///
    /// # Arguments
    ///
    /// * `category` - Tool category (e.g., "file_system", "shell")
    /// * `scope` - Scope to check (e.g., "read:~/Documents", "write:/tmp")
    ///
    /// # Returns
    ///
    /// - `true` if scope is explicitly granted
    /// - `true` if no scopes are defined for the category (permissive default)
    /// - `false` if scopes are defined but the requested scope is not included
    pub fn has_scope(&self, category: &str, scope: &str) -> bool {
        match &self.capabilities.granted_scopes {
            None => true, // No scopes defined = all allowed
            Some(scopes) => {
                match scopes.get(category) {
                    None => true, // Category not in scopes = all allowed for this category
                    Some(granted) => {
                        // Check exact match
                        if granted.contains(&scope.to_string()) {
                            return true;
                        }
                        // Check wildcard (e.g., "*" grants all)
                        if granted.contains(&"*".to_string()) {
                            return true;
                        }
                        // Check prefix match (e.g., "read:~/" matches "read:~/Documents")
                        for g in granted {
                            if scope.starts_with(g) {
                                return true;
                            }
                        }
                        false
                    }
                }
            }
        }
    }

    /// Get all granted scopes for a category.
    pub fn get_scopes(&self, category: &str) -> Option<&Vec<String>> {
        self.capabilities
            .granted_scopes
            .as_ref()
            .and_then(|s| s.get(category))
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

    #[test]
    fn test_has_scope_no_scopes_defined() {
        let manifest = ClientManifest::default();
        // No scopes defined = all allowed
        assert!(manifest.has_scope("file_system", "read:~/Documents"));
        assert!(manifest.has_scope("shell", "exec:/bin/ls"));
    }

    #[test]
    fn test_has_scope_category_not_in_scopes() {
        let mut scopes = HashMap::new();
        scopes.insert("file_system".to_string(), vec!["read:~/".to_string()]);

        let manifest = ClientManifest {
            capabilities: ClientCapabilities {
                granted_scopes: Some(scopes),
                ..Default::default()
            },
            ..Default::default()
        };

        // file_system has scopes, shell doesn't
        assert!(manifest.has_scope("shell", "exec:/bin/ls")); // Not in scopes = allowed
        assert!(manifest.has_scope("file_system", "read:~/Documents")); // Prefix match
    }

    #[test]
    fn test_has_scope_exact_match() {
        let mut scopes = HashMap::new();
        scopes.insert(
            "file_system".to_string(),
            vec!["read:~/Documents".to_string(), "write:/tmp".to_string()],
        );

        let manifest = ClientManifest {
            capabilities: ClientCapabilities {
                granted_scopes: Some(scopes),
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(manifest.has_scope("file_system", "read:~/Documents"));
        assert!(manifest.has_scope("file_system", "write:/tmp"));
        assert!(!manifest.has_scope("file_system", "read:~/Desktop")); // Not granted
        assert!(!manifest.has_scope("file_system", "write:~/Documents")); // Wrong action
    }

    #[test]
    fn test_has_scope_wildcard() {
        let mut scopes = HashMap::new();
        scopes.insert("shell".to_string(), vec!["*".to_string()]);

        let manifest = ClientManifest {
            capabilities: ClientCapabilities {
                granted_scopes: Some(scopes),
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(manifest.has_scope("shell", "exec:/bin/ls"));
        assert!(manifest.has_scope("shell", "exec:/usr/bin/rm"));
        assert!(manifest.has_scope("shell", "anything"));
    }

    #[test]
    fn test_has_scope_prefix_match() {
        let mut scopes = HashMap::new();
        scopes.insert(
            "file_system".to_string(),
            vec!["read:~/".to_string()], // Prefix grants all under ~/
        );

        let manifest = ClientManifest {
            capabilities: ClientCapabilities {
                granted_scopes: Some(scopes),
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(manifest.has_scope("file_system", "read:~/Documents"));
        assert!(manifest.has_scope("file_system", "read:~/Desktop/file.txt"));
        assert!(!manifest.has_scope("file_system", "read:/etc/passwd")); // Outside ~/
        assert!(!manifest.has_scope("file_system", "write:~/Documents")); // Wrong action
    }
}
