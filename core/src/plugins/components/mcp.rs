//! MCP configuration parser
//!
//! Parses Claude Code compatible .mcp.json files.

use std::path::Path;

use crate::plugins::error::{PluginError, PluginResult};
use crate::plugins::types::PluginMcpConfig;

/// MCP configuration loader
#[derive(Debug, Default)]
pub struct McpLoader;

impl McpLoader {
    /// Create a new MCP loader
    pub fn new() -> Self {
        Self
    }

    /// Load MCP configuration from a .mcp.json file
    pub fn load(&self, path: &Path) -> PluginResult<PluginMcpConfig> {
        let content = std::fs::read_to_string(path)?;
        let config: PluginMcpConfig =
            serde_json::from_str(&content).map_err(|e| PluginError::McpConfigParseError {
                path: path.to_path_buf(),
                source: e,
            })?;

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_load_mcp_config() {
        let temp = TempDir::new().unwrap();
        let mcp_path = temp.path().join(".mcp.json");

        let content = r#"{
            "mcpServers": {
                "filesystem": {
                    "command": "npx",
                    "args": ["-y", "@anthropic/mcp-server-filesystem"],
                    "env": {
                        "HOME": "/home/user"
                    }
                },
                "github": {
                    "command": "uvx",
                    "args": ["mcp-server-github"]
                }
            }
        }"#;

        fs::write(&mcp_path, content).unwrap();

        let loader = McpLoader::new();
        let config = loader.load(&mcp_path).unwrap();

        assert_eq!(config.mcp_servers.len(), 2);
        assert!(config.mcp_servers.contains_key("filesystem"));
        assert!(config.mcp_servers.contains_key("github"));

        let fs_server = &config.mcp_servers["filesystem"];
        assert_eq!(fs_server.command, "npx");
        assert_eq!(
            fs_server.args,
            vec!["-y", "@anthropic/mcp-server-filesystem"]
        );
        assert_eq!(fs_server.env.get("HOME"), Some(&"/home/user".to_string()));
    }

    #[test]
    fn test_load_empty_mcp_config() {
        let temp = TempDir::new().unwrap();
        let mcp_path = temp.path().join(".mcp.json");

        fs::write(&mcp_path, r#"{"mcpServers": {}}"#).unwrap();

        let loader = McpLoader::new();
        let config = loader.load(&mcp_path).unwrap();

        assert!(config.mcp_servers.is_empty());
    }

    #[test]
    fn test_load_minimal_server() {
        let temp = TempDir::new().unwrap();
        let mcp_path = temp.path().join(".mcp.json");

        let content = r#"{
            "mcpServers": {
                "simple": {
                    "command": "my-server"
                }
            }
        }"#;

        fs::write(&mcp_path, content).unwrap();

        let loader = McpLoader::new();
        let config = loader.load(&mcp_path).unwrap();

        let server = &config.mcp_servers["simple"];
        assert_eq!(server.command, "my-server");
        assert!(server.args.is_empty());
        assert!(server.env.is_empty());
    }

    #[test]
    fn test_load_invalid_json() {
        let temp = TempDir::new().unwrap();
        let mcp_path = temp.path().join(".mcp.json");

        fs::write(&mcp_path, "not valid json").unwrap();

        let loader = McpLoader::new();
        let result = loader.load(&mcp_path);

        assert!(matches!(result, Err(PluginError::McpConfigParseError { .. })));
    }
}
