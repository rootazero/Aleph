use crate::context::DashboardState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerInfo {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Option<std::collections::HashMap<String, String>>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub requires_runtime: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: Option<std::collections::HashMap<String, String>>,
}

pub struct McpConfigApi;

impl McpConfigApi {
    /// List all MCP servers
    pub async fn list(state: &DashboardState) -> Result<Vec<McpServerInfo>, String> {
        let result = state
            .rpc_call("mcp_config.list", serde_json::Value::Null)
            .await?;

        let servers = result
            .get("servers")
            .cloned()
            .unwrap_or(serde_json::Value::Array(vec![]));
        serde_json::from_value(servers).map_err(|e| format!("Failed to parse MCP servers: {e}"))
    }

    /// Get a specific MCP server by id
    pub async fn get(state: &DashboardState, id: String) -> Result<McpServerInfo, String> {
        let params = serde_json::json!({ "id": id });
        let result = state.rpc_call("mcp_config.get", params).await?;
        let server = result.get("server").cloned().unwrap_or(result);
        serde_json::from_value(server).map_err(|e| format!("Failed to parse MCP server: {e}"))
    }

    /// Create a new MCP server
    pub async fn create(
        state: &DashboardState,
        name: String,
        config: McpServerConfig,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "name": name,
            "config": config,
        });

        state.rpc_call("mcp_config.create", params).await?;
        Ok(())
    }

    /// Update an existing MCP server by id
    pub async fn update(
        state: &DashboardState,
        id: String,
        config: McpServerConfig,
    ) -> Result<(), String> {
        let params = serde_json::json!({ "id": id, "config": config });
        state.rpc_call("mcp_config.update", params).await?;
        Ok(())
    }

    /// Delete an MCP server by id
    pub async fn delete(state: &DashboardState, id: String) -> Result<(), String> {
        let params = serde_json::json!({ "id": id });
        state.rpc_call("mcp_config.delete", params).await?;
        Ok(())
    }
}

/// One env var a preset needs; drives the install dialog's key-entry UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPresetEnvVar {
    pub key: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub secret: bool,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub how_to_get_url: Option<String>,
}

/// A curated MCP preset from the built-in catalog (`mcp.list_presets`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPresetInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub official: bool,
    #[serde(default)]
    pub reachability: String,
    #[serde(default)]
    pub required_env: Vec<McpPresetEnvVar>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Already present in the running MCP set — hidden from the "recommended" list.
    #[serde(default)]
    pub installed: bool,
}

/// Outcome of an `mcp.install_preset` call, mirroring the host's `InstallPlan`.
#[derive(Debug, Clone)]
pub enum PresetInstallOutcome {
    Installed,
    AlreadyInstalled,
    /// Required keys still missing (names).
    NeedsKey(Vec<String>),
    /// No transport runtime available (e.g. "python", "node").
    NoRuntime(String),
}

pub struct McpPresetApi;

impl McpPresetApi {
    /// List the built-in preset catalog (each carries an `installed` flag).
    pub async fn list(state: &DashboardState) -> Result<Vec<McpPresetInfo>, String> {
        let result = state
            .rpc_call("mcp.list_presets", serde_json::Value::Null)
            .await?;
        let presets = result
            .get("presets")
            .cloned()
            .unwrap_or(serde_json::Value::Array(vec![]));
        serde_json::from_value(presets).map_err(|e| format!("Failed to parse MCP presets: {e}"))
    }

    /// Install a preset by id with the provided env. The host plans + (if ready)
    /// hot-starts the server, persisting it so it then appears in `mcp_config.list`.
    pub async fn install(
        state: &DashboardState,
        id: String,
        env: std::collections::HashMap<String, String>,
    ) -> Result<PresetInstallOutcome, String> {
        let params = serde_json::json!({ "id": id, "env": env });
        let result = state.rpc_call("mcp.install_preset", params).await?;
        match result.get("status").and_then(serde_json::Value::as_str) {
            Some("installed") => Ok(PresetInstallOutcome::Installed),
            Some("already_installed") => Ok(PresetInstallOutcome::AlreadyInstalled),
            Some("no_runtime") => Ok(PresetInstallOutcome::NoRuntime(
                result
                    .get("runtime")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )),
            Some("needs_key") => {
                let missing = result
                    .get("missing")
                    .and_then(serde_json::Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|e| {
                                e.get("key")
                                    .and_then(serde_json::Value::as_str)
                                    .map(String::from)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(PresetInstallOutcome::NeedsKey(missing))
            }
            other => Err(format!("Unexpected install status: {other:?}")),
        }
    }
}
