// Bridge between Aleph browser tools and the Playwright MCP Server.

use super::profile::PlaywrightMcpConfig;

/// Bridge between Aleph browser tools and the Playwright MCP Server.
/// Translates Aleph tool names/args to Playwright MCP calls.
pub struct PlaywrightBridge;

impl PlaywrightBridge {
    /// Generate an MCP external server config for Playwright.
    pub fn to_mcp_config(
        config: &PlaywrightMcpConfig,
        profile_name: &str,
        cdp_port: u16,
    ) -> crate::config::types::tools::McpExternalServerConfig {
        let mut args = config.args.clone();

        // Add CDP endpoint so Playwright connects to our managed browser
        args.push("--cdp-endpoint".into());
        args.push(format!("http://127.0.0.1:{cdp_port}"));

        crate::config::types::tools::McpExternalServerConfig {
            name: format!("playwright-{profile_name}"),
            command: config.command.clone(),
            args,
            env: std::collections::HashMap::new(),
            cwd: None,
            requires_runtime: Some("node".into()),
            timeout_seconds: 30,
        }
    }

    /// Map Aleph browser tool names to Playwright MCP tool names.
    pub fn map_tool_name(aleph_name: &str) -> Option<&'static str> {
        match aleph_name {
            "browser_click" => Some("playwright_click"),
            "browser_type" => Some("playwright_type"),
            "browser_select" => Some("playwright_select"),
            "browser_screenshot" => Some("playwright_screenshot"),
            "browser_snapshot" => Some("playwright_snapshot"),
            "browser_evaluate" => Some("playwright_evaluate"),
            "browser_navigate" => Some("playwright_navigate"),
            "browser_fill_form" => Some("playwright_fill"),
            "browser_upload" => Some("playwright_upload"),
            "browser_download" => Some("playwright_download"),
            "browser_network" => Some("playwright_network"),
            _ => None,
        }
    }

    /// Map Aleph tool args to Playwright MCP args format.
    /// For now, pass through. Playwright MCP accepts similar arg shapes.
    pub fn map_args(
        _aleph_name: &str,
        args: serde_json::Value,
    ) -> serde_json::Value {
        args
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generates_mcp_server_config() {
        let config = PlaywrightMcpConfig::default();
        let mcp_config = PlaywrightBridge::to_mcp_config(&config, "default", 18800);

        assert_eq!(mcp_config.name, "playwright-default");
        assert_eq!(mcp_config.command, "npx");
        assert!(mcp_config.args.contains(&"@anthropic/mcp-playwright".to_string()));
        assert!(mcp_config.args.contains(&"--cdp-endpoint".to_string()));
        assert!(mcp_config.args.contains(&"http://127.0.0.1:18800".to_string()));
        assert_eq!(mcp_config.requires_runtime, Some("node".into()));
        assert_eq!(mcp_config.timeout_seconds, 30);
    }

    #[test]
    fn test_bridge_tool_name_mapping() {
        assert_eq!(
            PlaywrightBridge::map_tool_name("browser_click"),
            Some("playwright_click")
        );
        assert_eq!(
            PlaywrightBridge::map_tool_name("browser_screenshot"),
            Some("playwright_screenshot")
        );
        assert_eq!(
            PlaywrightBridge::map_tool_name("unknown_tool"),
            None
        );
    }

    #[test]
    fn test_map_args_passthrough() {
        let args = serde_json::json!({"selector": "#btn", "text": "hello"});
        let mapped = PlaywrightBridge::map_args("browser_click", args.clone());
        assert_eq!(mapped, args);
    }

    #[test]
    fn test_custom_mcp_config() {
        let config = PlaywrightMcpConfig {
            enabled: true,
            command: "node".to_string(),
            args: vec!["./custom-server.js".to_string()],
        };
        let mcp_config = PlaywrightBridge::to_mcp_config(&config, "work", 19000);

        assert_eq!(mcp_config.name, "playwright-work");
        assert_eq!(mcp_config.command, "node");
        assert_eq!(mcp_config.args, vec![
            "./custom-server.js",
            "--cdp-endpoint",
            "http://127.0.0.1:19000",
        ]);
    }
}
