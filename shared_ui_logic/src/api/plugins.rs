//! Plugins API
//!
//! High-level API for plugin management operations.

use crate::protocol::rpc::{RpcClient, RpcError};
use serde::{Deserialize, Serialize};

/// Plugin information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    /// Plugin name
    pub name: String,
    /// Plugin version
    pub version: String,
    /// Plugin description
    pub description: String,
    /// Whether the plugin is enabled
    pub enabled: bool,
    /// Plugin installation path
    pub path: String,
    /// Number of skills provided
    pub skills_count: u32,
    /// Number of agents provided
    pub agents_count: u32,
    /// Number of hooks provided
    pub hooks_count: u32,
    /// Number of MCP servers provided
    pub mcp_servers_count: u32,
}

/// Plugin API client
///
/// Provides high-level methods for plugin management.
///
/// # Example
///
/// ```ignore
/// use aleph_ui_logic::api::PluginsApi;
///
/// let api = PluginsApi::new(rpc_client);
/// let plugins = api.list().await?;
/// ```
pub struct PluginsApi<C: crate::connection::AlephConnector> {
    rpc: RpcClient<C>,
}

impl<C: crate::connection::AlephConnector> PluginsApi<C> {
    /// Create a new plugins API client
    pub fn new(rpc: RpcClient<C>) -> Self {
        Self { rpc }
    }

    /// List all installed plugins
    pub async fn list(&self) -> Result<Vec<PluginInfo>, RpcError>
    where
        C: crate::connection::AlephConnector,
    {
        #[derive(Deserialize)]
        struct Response {
            plugins: Vec<PluginInfo>,
        }

        let response: Response = self.rpc.call("plugins.list", &()).await?;
        Ok(response.plugins)
    }

    /// Install a plugin from Git repository
    pub async fn install(&self, url: &str) -> Result<PluginInfo, RpcError>
    where
        C: crate::connection::AlephConnector,
    {
        #[derive(Serialize)]
        struct Params<'a> {
            url: &'a str,
        }

        #[derive(Deserialize)]
        struct Response {
            plugin: PluginInfo,
        }

        let response: Response = self.rpc.call("plugins.install", &Params { url }).await?;
        Ok(response.plugin)
    }

    /// Install a plugin from base64-encoded ZIP data
    pub async fn install_from_zip(&self, data: &str) -> Result<PluginInfo, RpcError>
    where
        C: crate::connection::AlephConnector,
    {
        #[derive(Serialize)]
        struct Params<'a> {
            data: &'a str,
        }

        #[derive(Deserialize)]
        struct Response {
            plugin: PluginInfo,
        }

        let response: Response = self
            .rpc
            .call("plugins.installFromZip", &Params { data })
            .await?;
        Ok(response.plugin)
    }

    /// Uninstall a plugin
    pub async fn uninstall(&self, name: &str) -> Result<(), RpcError>
    where
        C: crate::connection::AlephConnector,
    {
        #[derive(Serialize)]
        struct Params<'a> {
            name: &'a str,
        }

        #[derive(Deserialize)]
        struct Response {
            ok: bool,
        }

        let _response: Response = self.rpc.call("plugins.uninstall", &Params { name }).await?;
        Ok(())
    }

    /// Enable a plugin
    pub async fn enable(&self, name: &str) -> Result<(), RpcError>
    where
        C: crate::connection::AlephConnector,
    {
        #[derive(Serialize)]
        struct Params<'a> {
            name: &'a str,
        }

        #[derive(Deserialize)]
        struct Response {
            ok: bool,
        }

        let _response: Response = self.rpc.call("plugins.enable", &Params { name }).await?;
        Ok(())
    }

    /// Disable a plugin
    pub async fn disable(&self, name: &str) -> Result<(), RpcError>
    where
        C: crate::connection::AlephConnector,
    {
        #[derive(Serialize)]
        struct Params<'a> {
            name: &'a str,
        }

        #[derive(Deserialize)]
        struct Response {
            ok: bool,
        }

        let _response: Response = self.rpc.call("plugins.disable", &Params { name }).await?;
        Ok(())
    }

    /// Call a plugin tool
    pub async fn call_tool(
        &self,
        plugin_id: &str,
        handler: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, RpcError>
    where
        C: crate::connection::AlephConnector,
    {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Params<'a> {
            plugin_id: &'a str,
            handler: &'a str,
            args: serde_json::Value,
        }

        self.rpc
            .call(
                "plugins.callTool",
                &Params {
                    plugin_id,
                    handler,
                    args,
                },
            )
            .await
    }

    /// Execute a plugin command
    pub async fn execute_command(
        &self,
        plugin_id: &str,
        command_name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, RpcError>
    where
        C: crate::connection::AlephConnector,
    {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Params<'a> {
            plugin_id: &'a str,
            command_name: &'a str,
            args: serde_json::Value,
        }

        self.rpc
            .call(
                "plugins.executeCommand",
                &Params {
                    plugin_id,
                    command_name,
                    args,
                },
            )
            .await
    }

    /// Load a plugin from path
    pub async fn load(&self, path: &str) -> Result<PluginInfo, RpcError>
    where
        C: crate::connection::AlephConnector,
    {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Params<'a> {
            path: &'a str,
        }

        #[derive(Deserialize)]
        struct Response {
            plugin: PluginInfo,
        }

        let response: Response = self.rpc.call("plugins.load", &Params { path }).await?;
        Ok(response.plugin)
    }

    /// Unload a plugin
    pub async fn unload(&self, plugin_id: &str) -> Result<(), RpcError>
    where
        C: crate::connection::AlephConnector,
    {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Params<'a> {
            plugin_id: &'a str,
        }

        #[derive(Deserialize)]
        struct Response {
            ok: bool,
        }

        let _response: Response = self.rpc.call("plugins.unload", &Params { plugin_id }).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_info_serialization() {
        let info = PluginInfo {
            name: "test-plugin".to_string(),
            version: "1.0.0".to_string(),
            description: "Test plugin".to_string(),
            enabled: true,
            path: "/path/to/plugin".to_string(),
            skills_count: 5,
            agents_count: 2,
            hooks_count: 3,
            mcp_servers_count: 1,
        };

        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["name"], "test-plugin");
        assert_eq!(json["version"], "1.0.0");
        assert_eq!(json["enabled"], true);
        assert_eq!(json["skills_count"], 5);
    }
}
