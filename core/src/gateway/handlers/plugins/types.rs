//! Plugin handler parameter types

use serde::{Deserialize, Serialize};

use crate::extension::PluginInfo;

// ============================================================================
// Plugin Info JSON
// ============================================================================

/// Plugin info for JSON serialization
#[derive(Debug, Clone, Serialize)]
pub struct PluginInfoJson {
    pub name: String,
    pub version: String,
    pub description: String,
    pub enabled: bool,
    pub path: String,
    pub skills_count: u32,
    pub agents_count: u32,
    pub hooks_count: u32,
    pub mcp_servers_count: u32,
}

impl From<PluginInfo> for PluginInfoJson {
    fn from(info: PluginInfo) -> Self {
        Self {
            name: info.name,
            version: info.version.unwrap_or_default(),
            description: info.description.unwrap_or_default(),
            enabled: info.enabled,
            path: info.path,
            skills_count: info.skills_count as u32,
            agents_count: info.agents_count as u32,
            hooks_count: info.hooks_count as u32,
            mcp_servers_count: info.mcp_servers_count as u32,
        }
    }
}

// ============================================================================
// Install Parameters
// ============================================================================

/// Parameters for plugins.install
#[derive(Debug, Deserialize)]
pub struct InstallParams {
    /// Git URL to install from
    pub url: String,
}

/// Parameters for plugins.installFromZip
#[derive(Debug, Deserialize)]
pub struct InstallFromZipParams {
    /// Base64-encoded zip data
    pub data: String,
}

// ============================================================================
// Uninstall Parameters
// ============================================================================

/// Parameters for plugins.uninstall
#[derive(Debug, Deserialize)]
pub struct UninstallParams {
    pub name: String,
}

// ============================================================================
// Enable/Disable Parameters
// ============================================================================

/// Parameters for plugins.enable and plugins.disable
#[derive(Debug, Deserialize)]
pub struct ToggleParams {
    pub name: String,
}

// ============================================================================
// Call Tool Parameters
// ============================================================================

/// Parameters for plugins.callTool
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolParams {
    /// ID of the plugin containing the tool
    pub plugin_id: String,
    /// Name of the handler function to call
    pub handler: String,
    /// Arguments to pass to the tool
    #[serde(default)]
    pub args: serde_json::Value,
}

// ============================================================================
// Load/Unload Parameters
// ============================================================================

/// Parameters for plugins.load
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadPluginParams {
    /// Path to the plugin directory (containing aleph.plugin.json or package.json with aleph field)
    pub path: String,
}

/// Parameters for plugins.unload
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnloadPluginParams {
    /// ID of the plugin to unload
    pub plugin_id: String,
}

// ============================================================================
// Execute Command Parameters
// ============================================================================

/// Parameters for plugins.executeCommand
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteCommandParams {
    /// ID of the plugin providing the command
    pub plugin_id: String,
    /// Name of the command to execute
    pub command_name: String,
    /// Arguments to pass to the command handler
    #[serde(default)]
    pub args: serde_json::Value,
}
