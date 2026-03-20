//! MCP plugin configuration reader
//!
//! Reads `.mcp.json` from plugin directories and prepares MCP server configs
//! for registration with Aleph's MCP client system (McpManager).
//!
//! # .mcp.json Format
//!
//! ```json
//! {
//!   "mcpServers": {
//!     "server-name": {
//!       "command": "node",
//!       "args": ["${ALEPH_PLUGIN_ROOT}/src/server.js"],
//!       "env": { "KEY": "value" }
//!     }
//!   }
//! }
//! ```
//!
//! # Variable Substitution
//!
//! The following variables are expanded in `command`, `args`, and `env` values:
//! - `${CLAUDE_PLUGIN_ROOT}` — absolute path to the plugin directory
//! - `${ALEPH_PLUGIN_ROOT}` — same as above (Aleph alias)

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::mcp::McpManagerConfig;

/// Raw .mcp.json file structure
#[derive(Debug, Deserialize)]
struct McpJsonFile {
    #[serde(rename = "mcpServers", default)]
    mcp_servers: HashMap<String, McpJsonServerEntry>,
}

/// A single server entry in .mcp.json
#[derive(Debug, Deserialize)]
struct McpJsonServerEntry {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
}

/// Read `.mcp.json` from a plugin directory and return MCP manager configs.
///
/// Each server config has environment variables substituted and is keyed by
/// `plugin_id/server_name` to avoid collisions across plugins.
///
/// # Arguments
///
/// * `plugin_dir` - The plugin root directory containing `.mcp.json`
/// * `plugin_id` - Plugin identifier used to namespace server IDs
///
/// # Returns
///
/// Map of `server_id` → `McpManagerConfig` ready for registration.
/// Returns an empty map if `.mcp.json` does not exist.
pub fn read_mcp_json(
    plugin_dir: &Path,
    plugin_id: &str,
) -> ExtensionResult<HashMap<String, McpManagerConfig>> {
    let mcp_path = plugin_dir.join(".mcp.json");
    if !mcp_path.exists() {
        return Ok(HashMap::new());
    }

    let content = std::fs::read_to_string(&mcp_path).map_err(|e| {
        ExtensionError::config_parse(&mcp_path, format!("Failed to read .mcp.json: {}", e))
    })?;

    parse_mcp_json_content(&content, plugin_dir, plugin_id).map_err(|e| {
        ExtensionError::config_parse(
            &mcp_path,
            format!("Invalid .mcp.json: {}", e),
        )
    })
}

/// Parse .mcp.json content and return MCP manager configs.
///
/// Separated from `read_mcp_json` for testability.
fn parse_mcp_json_content(
    content: &str,
    plugin_dir: &Path,
    plugin_id: &str,
) -> Result<HashMap<String, McpManagerConfig>, String> {
    let file: McpJsonFile =
        serde_json::from_str(content).map_err(|e| format!("JSON parse error: {}", e))?;

    let plugin_root = plugin_dir.to_string_lossy();
    let mut result = HashMap::new();

    for (server_name, entry) in file.mcp_servers {
        let server_id = format!("plugin:{}/{}", plugin_id, server_name);
        let display_name = format!("{} ({})", server_name, plugin_id);

        let command = substitute_vars(&entry.command, &plugin_root);
        let args: Vec<String> = entry
            .args
            .iter()
            .map(|a| substitute_vars(a, &plugin_root))
            .collect();
        let env: HashMap<String, String> = entry
            .env
            .iter()
            .map(|(k, v)| (k.clone(), substitute_vars(v, &plugin_root)))
            .collect();

        let config = McpManagerConfig::stdio(&server_id, &display_name, &command)
            .with_args(args)
            .with_env(env)
            .with_auto_start(true);

        result.insert(server_id, config);
    }

    Ok(result)
}

/// Substitute `${CLAUDE_PLUGIN_ROOT}` and `${ALEPH_PLUGIN_ROOT}` in a string value.
fn substitute_vars(value: &str, plugin_root: &str) -> String {
    value
        .replace("${CLAUDE_PLUGIN_ROOT}", plugin_root)
        .replace("${ALEPH_PLUGIN_ROOT}", plugin_root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mcp_json_basic() {
        let content = r#"{
            "mcpServers": {
                "my-server": {
                    "command": "node",
                    "args": ["${ALEPH_PLUGIN_ROOT}/src/server.js", "--port", "3000"],
                    "env": {
                        "NODE_ENV": "production",
                        "PLUGIN_DIR": "${CLAUDE_PLUGIN_ROOT}"
                    }
                }
            }
        }"#;

        let result =
            parse_mcp_json_content(content, Path::new("/plugins/test-plugin"), "test-plugin")
                .unwrap();

        assert_eq!(result.len(), 1);

        let config = result.get("plugin:test-plugin/my-server").unwrap();
        assert_eq!(config.command, Some("node".to_string()));
        assert_eq!(
            config.args,
            vec![
                "/plugins/test-plugin/src/server.js",
                "--port",
                "3000"
            ]
        );
        assert_eq!(
            config.env.get("PLUGIN_DIR"),
            Some(&"/plugins/test-plugin".to_string())
        );
        assert_eq!(
            config.env.get("NODE_ENV"),
            Some(&"production".to_string())
        );
        assert!(config.auto_start);
    }

    #[test]
    fn test_parse_mcp_json_multiple_servers() {
        let content = r#"{
            "mcpServers": {
                "alpha": {
                    "command": "python",
                    "args": ["-m", "server_a"]
                },
                "beta": {
                    "command": "node",
                    "args": ["server_b.js"]
                }
            }
        }"#;

        let result =
            parse_mcp_json_content(content, Path::new("/plugins/multi"), "multi").unwrap();

        assert_eq!(result.len(), 2);
        assert!(result.contains_key("plugin:multi/alpha"));
        assert!(result.contains_key("plugin:multi/beta"));
    }

    #[test]
    fn test_parse_mcp_json_empty_servers() {
        let content = r#"{ "mcpServers": {} }"#;

        let result =
            parse_mcp_json_content(content, Path::new("/plugins/empty"), "empty").unwrap();

        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_mcp_json_invalid_json() {
        let content = "not json at all";
        let result = parse_mcp_json_content(content, Path::new("/plugins/bad"), "bad");
        assert!(result.is_err());
    }

    #[test]
    fn test_substitute_vars() {
        assert_eq!(
            substitute_vars("${ALEPH_PLUGIN_ROOT}/bin/run", "/home/user/plugins/x"),
            "/home/user/plugins/x/bin/run"
        );
        assert_eq!(
            substitute_vars("${CLAUDE_PLUGIN_ROOT}/index.js", "/tmp/p"),
            "/tmp/p/index.js"
        );
        // Both in same string
        assert_eq!(
            substitute_vars(
                "${ALEPH_PLUGIN_ROOT}:${CLAUDE_PLUGIN_ROOT}",
                "/root"
            ),
            "/root:/root"
        );
        // No vars
        assert_eq!(substitute_vars("plain text", "/root"), "plain text");
    }

    #[test]
    fn test_read_mcp_json_missing_file() {
        let result = read_mcp_json(Path::new("/nonexistent/dir"), "test").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_server_id_namespacing() {
        let content = r#"{
            "mcpServers": {
                "srv": { "command": "echo", "args": [] }
            }
        }"#;

        let result =
            parse_mcp_json_content(content, Path::new("/p/a"), "my-plugin").unwrap();

        // Server ID should be namespaced with plugin ID
        assert!(result.contains_key("plugin:my-plugin/srv"));
        let config = &result["plugin:my-plugin/srv"];
        assert_eq!(config.id, "plugin:my-plugin/srv");
        assert!(config.name.contains("my-plugin"));
    }
}
