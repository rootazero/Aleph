//! MCP plugin configuration reader
//!
//! Reads `.mcp.json` from plugin directories and prepares MCP server configs
//! for registration with Aleph's MCP client system (`McpManager`).
//!
//! # .mcp.json Format
//!
//! ## stdio transport (default)
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
//! ## remote transport (HTTP/SSE)
//!
//! ```json
//! {
//!   "mcpServers": {
//!     "remote-server": {
//!       "type": "remote",
//!       "url": "https://mcp.example.com/api",
//!       "headers": { "Authorization": "Bearer ${ALEPH_PLUGIN_ROOT}/token" }
//!     }
//!   }
//! }
//! ```
//!
//! The `type` field defaults to `stdio` when omitted, so existing plugin
//! manifests continue to work unchanged.
//!
//! # Variable Substitution
//!
//! The following variables are expanded in `command`, `args`, `url`, `env`,
//! and `headers` values:
//! - `${CLAUDE_PLUGIN_ROOT}` — absolute path to the plugin directory
//! - `${ALEPH_PLUGIN_ROOT}` — same as above (Aleph alias)
//!
//! (`${ALEPH_PLUGIN_DATA}` lives in the higher-level `McpManagerConfig::env`
//! / `McpManagerConfig::headers` substitution path, not here — those are
//! resolved at spawn time by the manager actor so they see the
//! post-`mcp.list` / `mcp.install` view of the user's data dir.)

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::mcp::{McpManagerConfig, McpTransportType};

/// Raw .mcp.json file structure
#[derive(Debug, Deserialize)]
struct McpJsonFile {
    #[serde(rename = "mcpServers", default)]
    mcp_servers: HashMap<String, McpJsonServerEntry>,
}

/// A single server entry in .mcp.json.
///
/// Either a stdio entry (`command` + `args` + `env`) or a remote entry
/// (`url` + `headers` + optional `transport: "sse"`). The `type` discriminator
/// defaults to `stdio` when absent so existing plugins continue to parse.
#[derive(Debug, Deserialize)]
struct McpJsonServerEntry {
    #[serde(default = "default_transport", rename = "type")]
    transport: String,
    // stdio fields
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    // remote fields
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    headers: HashMap<String, String>,
}

fn default_transport() -> String {
    "stdio".to_string()
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
        ExtensionError::config_parse(&mcp_path, format!("Failed to read .mcp.json: {e}"))
    })?;

    parse_mcp_json_content(&content, plugin_dir, plugin_id)
        .map_err(|e| ExtensionError::config_parse(&mcp_path, format!("Invalid .mcp.json: {e}")))
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
        serde_json::from_str(content).map_err(|e| format!("JSON parse error: {e}"))?;

    let plugin_root = plugin_dir.to_string_lossy();
    let mut result = HashMap::new();

    for (server_name, entry) in file.mcp_servers {
        let server_id = format!("plugin:{plugin_id}/{server_name}");
        let display_name = format!("{server_name} ({plugin_id})");

        let transport = match entry.transport.as_str() {
            "stdio" => McpTransportType::Stdio,
            "http" => McpTransportType::Http,
            "sse" => McpTransportType::Sse,
            other => return Err(format!(
                "unknown MCP transport type '{other}' for server '{server_name}' \
                 (expected one of: stdio, http, sse)"
            )),
        };

        let config = match transport {
            McpTransportType::Stdio => {
                // stdio entries require `command`. Refuse ambiguous configs
                // rather than spawning a phantom process.
                let command = entry.command.ok_or_else(|| {
                    format!(
                        "MCP stdio server '{server_name}' is missing 'command' \
                         (either add it or set `\"type\": \"remote\"` with a `url`)"
                    )
                })?;
                let cmd = substitute_vars(&command, &plugin_root);
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
                McpManagerConfig::stdio(&server_id, &display_name, &cmd)
                    .with_args(args)
                    .with_env(env)
                    .with_auto_start(true)
            }
            McpTransportType::Http | McpTransportType::Sse => {
                // remote entries require `url`. Refuse ambiguous configs.
                let url = entry.url.ok_or_else(|| {
                    format!(
                        "MCP remote server '{server_name}' is missing 'url' \
                         (either add it or set `\"type\": \"stdio\"` with a `command`)"
                    )
                })?;
                let url = substitute_vars(&url, &plugin_root);
                let headers: HashMap<String, String> = entry
                    .headers
                    .iter()
                    .map(|(k, v)| (k.clone(), substitute_vars(v, &plugin_root)))
                    .collect();
                let mut config = if transport == McpTransportType::Sse {
                    McpManagerConfig::sse(&server_id, &display_name, &url)
                } else {
                    McpManagerConfig::http(&server_id, &display_name, &url)
                };
                config.headers = headers;
                config.auto_start = true;
                config
            }
        };

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
            vec!["/plugins/test-plugin/src/server.js", "--port", "3000"]
        );
        assert_eq!(
            config.env.get("PLUGIN_DIR"),
            Some(&"/plugins/test-plugin".to_string())
        );
        assert_eq!(config.env.get("NODE_ENV"), Some(&"production".to_string()));
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

        let result = parse_mcp_json_content(content, Path::new("/plugins/multi"), "multi").unwrap();

        assert_eq!(result.len(), 2);
        assert!(result.contains_key("plugin:multi/alpha"));
        assert!(result.contains_key("plugin:multi/beta"));
    }

    #[test]
    fn test_parse_mcp_json_empty_servers() {
        let content = r#"{ "mcpServers": {} }"#;

        let result = parse_mcp_json_content(content, Path::new("/plugins/empty"), "empty").unwrap();

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
            substitute_vars("${ALEPH_PLUGIN_ROOT}:${CLAUDE_PLUGIN_ROOT}", "/root"),
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

        let result = parse_mcp_json_content(content, Path::new("/p/a"), "my-plugin").unwrap();

        // Server ID should be namespaced with plugin ID
        assert!(result.contains_key("plugin:my-plugin/srv"));
        let config = &result["plugin:my-plugin/srv"];
        assert_eq!(config.id, "plugin:my-plugin/srv");
        assert!(config.name.contains("my-plugin"));
    }

    #[test]
    fn test_parse_mcp_json_remote_http_transport() {
        let content = r#"{
            "mcpServers": {
                "remote-srv": {
                    "type": "remote",
                    "url": "https://mcp.example.com/api",
                    "headers": { "Authorization": "Bearer ${ALEPH_PLUGIN_DATA}/token" }
                }
            }
        }"#;

        let result = parse_mcp_json_content(content, Path::new("/p/x"), "remote-plugin").unwrap();

        let config = result
            .get("plugin:remote-plugin/remote-srv")
            .expect("server must be registered");
        use crate::mcp::McpTransportType;
        assert_eq!(config.transport, McpTransportType::Http);
        assert_eq!(config.url.as_deref(), Some("https://mcp.example.com/api"));
        assert_eq!(config.command, None, "remote transport must not carry a command");
        assert!(config.args.is_empty());
        assert!(config.auto_start, "remote servers auto-start by default");
        assert_eq!(
            config.headers.get("Authorization").map(String::as_str),
            Some("Bearer /p/x/token"),
            "expected plugin_root + /token substitution in the Authorization header"
        );
    }

    #[test]
    fn test_parse_mcp_json_remote_sse_transport() {
        let content = r#"{
            "mcpServers": {
                "events": {
                    "type": "remote",
                    "url": "https://events.example.com/sse",
                    "transport": "sse"
                }
            }
        }"#;

        let result = parse_mcp_json_content(content, Path::new("/p/x"), "ev").unwrap();
        use crate::mcp::McpTransportType;
        let config = result.get("plugin:ev/events").unwrap();
        assert_eq!(config.transport, McpTransportType::Sse);
        assert_eq!(config.url.as_deref(), Some("https://events.example.com/sse"));
    }

    #[test]
    fn test_parse_mcp_json_default_transport_is_stdio() {
        // Bare entry without `type` must still parse as stdio (backward-compat).
        let content = r#"{
            "mcpServers": {
                "legacy": { "command": "node", "args": ["server.js"] }
            }
        }"#;
        let result = parse_mcp_json_content(content, Path::new("/p/x"), "legacy").unwrap();
        use crate::mcp::McpTransportType;
        let config = result.get("plugin:legacy/legacy").unwrap();
        assert_eq!(config.transport, McpTransportType::Stdio);
        assert_eq!(config.command.as_deref(), Some("node"));
    }

    #[test]
    fn test_parse_mcp_json_stdio_without_command_errors() {
        let content = r#"{
            "mcpServers": {
                "broken": { "args": ["x"] }
            }
        }"#;
        let err = parse_mcp_json_content(content, Path::new("/p/x"), "broken").unwrap_err();
        assert!(
            err.contains("missing 'command'"),
            "stdio without command must be a hard error: {err}"
        );
    }

    #[test]
    fn test_parse_mcp_json_remote_without_url_errors() {
        let content = r#"{
            "mcpServers": {
                "broken": { "type": "remote", "headers": {} }
            }
        }"#;
        let err = parse_mcp_json_content(content, Path::new("/p/x"), "broken").unwrap_err();
        assert!(
            err.contains("missing 'url'"),
            "remote without url must be a hard error: {err}"
        );
    }

    #[test]
    fn test_parse_mcp_json_unknown_transport_errors() {
        let content = r#"{
            "mcpServers": {
                "broken": { "type": "telnet" }
            }
        }"#;
        let err = parse_mcp_json_content(content, Path::new("/p/x"), "broken").unwrap_err();
        assert!(
            err.contains("unknown MCP transport type 'telnet'"),
            "unknown transport must surface a clear error: {err}"
        );
    }
}
