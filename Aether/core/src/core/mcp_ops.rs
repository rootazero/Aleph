//! MCP (Model Context Protocol) operations for AetherCore
//!
//! This module contains all MCP capability methods:
//! - MCP configuration management
//! - MCP server listing and management
//! - MCP tool discovery
//! - Config import/export

use super::AetherCore;
use crate::error::{AetherError, Result};
use tracing::info;

impl AetherCore {
    // ========================================================================
    // MCP CAPABILITY METHODS (implement-mcp-capability Phase 3)
    // ========================================================================

    /// Get MCP configuration for Settings UI
    ///
    /// Returns the current MCP configuration including enabled services
    /// and their security settings (allowed paths, commands, etc.)
    pub fn get_mcp_config(&self) -> crate::mcp::McpSettingsConfig {
        let config = self.lock_config();
        crate::mcp::McpSettingsConfig {
            enabled: config.mcp.enabled,
            fs_enabled: config.tools.fs_enabled,
            git_enabled: config.tools.git_enabled,
            shell_enabled: config.tools.shell_enabled,
            system_info_enabled: config.tools.system_info_enabled,
            allowed_roots: config.tools.allowed_roots.clone(),
            allowed_repos: config.tools.allowed_repos.clone(),
            allowed_commands: config.tools.allowed_commands.clone(),
            shell_timeout_seconds: config.tools.shell_timeout_seconds,
        }
    }

    /// Update MCP configuration
    ///
    /// Updates the MCP configuration and persists to disk.
    /// Note: Service changes will take effect on next app restart.
    ///
    /// # Arguments
    /// * `new_config` - New MCP configuration from UI
    ///
    /// # Returns
    /// * `Result<()>` - Success or error
    pub fn update_mcp_config(&self, new_config: crate::mcp::McpSettingsConfig) -> Result<()> {
        let mut config = self.lock_config();

        // Update config
        config.mcp.enabled = new_config.enabled;
        config.tools.fs_enabled = new_config.fs_enabled;
        config.tools.git_enabled = new_config.git_enabled;
        config.tools.shell_enabled = new_config.shell_enabled;
        config.tools.system_info_enabled = new_config.system_info_enabled;
        config.tools.allowed_roots = new_config.allowed_roots;
        config.tools.allowed_repos = new_config.allowed_repos;
        config.tools.allowed_commands = new_config.allowed_commands;
        config.tools.shell_timeout_seconds = new_config.shell_timeout_seconds;

        // Persist to disk
        config.save()?;

        // Notify event handler
        self.event_handler.on_config_changed();

        info!("MCP configuration updated");
        Ok(())
    }

    /// List registered MCP services
    ///
    /// Returns information about external MCP servers only.
    /// Native tools (fs, git, shell, etc.) are now handled via the
    /// `AgentTool` infrastructure and can be queried via `native_tool_count()`.
    ///
    /// # Returns
    /// * `Vec<McpServiceInfo>` - List of service information
    pub fn list_mcp_services(&self) -> Vec<crate::mcp::McpServiceInfo> {
        let Some(client) = &self.mcp_client else {
            return Vec::new();
        };

        // Only list external MCP servers
        // Native tools are now in NativeToolRegistry
        let services = self.runtime.block_on(async {
            let service_names = client.service_names().await;
            let mut result = Vec::new();

            for name in service_names {
                result.push(crate::mcp::McpServiceInfo {
                    name: name.clone(),
                    description: Self::get_service_description(&name),
                    is_builtin: false,
                    is_running: true, // External servers that are listed are running
                    tool_count: 0,    // Will be populated from external server
                });
            }

            result
        });

        services
    }

    /// Get service description by name
    fn get_service_description(name: &str) -> String {
        match name {
            "fs" => "Filesystem operations (read, write, list files)".to_string(),
            "git" => "Git repository operations (status, log, diff)".to_string(),
            "shell" => "Shell command execution (whitelisted commands)".to_string(),
            "system_info" => "System information (hostname, platform, memory)".to_string(),
            _ => format!("External MCP service: {}", name),
        }
    }

    /// List available MCP tools
    ///
    /// Returns information about all available MCP tools from registered services.
    ///
    /// # Returns
    /// * `Vec<McpToolInfo>` - List of tool information
    pub fn list_mcp_tools(&self) -> Vec<crate::mcp::McpToolInfo> {
        let Some(client) = &self.mcp_client else {
            return Vec::new();
        };

        // Use block_on to get tools from async context
        let tools = self.runtime.block_on(async { client.list_tools().await });

        tools
            .into_iter()
            .map(|tool| {
                // Extract service name from tool name (format: "service:tool_name")
                let service_name = tool
                    .name
                    .split(':')
                    .next()
                    .unwrap_or("unknown")
                    .to_string();

                crate::mcp::McpToolInfo {
                    name: tool.name,
                    description: tool.description,
                    requires_confirmation: tool.requires_confirmation,
                    service_name,
                }
            })
            .collect()
    }

    // ========================================================================
    // MCP Server Management Methods (redesign-mcp-settings-ui)
    // ========================================================================

    /// List all external MCP servers
    ///
    /// Returns a list of configured external MCP servers for the Settings UI.
    /// Note: Native tools (fs, git, shell, etc.) are now handled via the
    /// `AgentTool` infrastructure and are NOT included in this list.
    pub fn list_mcp_servers(&self) -> Vec<crate::mcp::McpServerConfig> {
        let config = self.lock_config();
        let mut servers = Vec::new();

        // Only add external servers
        // Native tools (fs, git, shell, system_info) are now in NativeToolRegistry
        for ext in &config.mcp.external_servers {
            servers.push(crate::mcp::McpServerConfig {
                id: ext.name.clone(),
                name: ext.name.clone(),
                server_type: crate::mcp::McpServerType::External,
                enabled: true, // External servers are enabled if configured
                command: Some(ext.command.clone()),
                args: ext.args.clone(),
                env: ext
                    .env
                    .iter()
                    .map(|(k, v)| crate::mcp::McpEnvVar {
                        key: k.clone(),
                        value: v.clone(),
                    })
                    .collect(),
                working_directory: ext.cwd.clone(),
                trigger_command: Some(format!("/mcp/{}", ext.name)),
                permissions: crate::mcp::McpServerPermissions {
                    requires_confirmation: true,
                    allowed_paths: Vec::new(),
                    allowed_commands: Vec::new(),
                },
                icon: "puzzlepiece.extension".to_string(),
                color: "#FF9500".to_string(),
            });
        }

        servers
    }

    /// Get a specific MCP server by ID
    pub fn get_mcp_server(&self, id: String) -> Option<crate::mcp::McpServerConfig> {
        self.list_mcp_servers().into_iter().find(|s| s.id == id)
    }

    /// Get MCP server status
    pub fn get_mcp_server_status(&self, id: String) -> crate::mcp::McpServerStatusInfo {
        // For now, return a basic status based on whether the server is enabled
        let server = self.get_mcp_server(id.clone());

        match server {
            Some(s) => {
                if s.enabled {
                    crate::mcp::McpServerStatusInfo {
                        status: crate::mcp::McpServerStatus::Running,
                        message: Some("Server is active".to_string()),
                        last_error: None,
                    }
                } else {
                    crate::mcp::McpServerStatusInfo {
                        status: crate::mcp::McpServerStatus::Stopped,
                        message: Some("Server is disabled".to_string()),
                        last_error: None,
                    }
                }
            }
            None => crate::mcp::McpServerStatusInfo {
                status: crate::mcp::McpServerStatus::Error,
                message: None,
                last_error: Some(format!("Server '{}' not found", id)),
            },
        }
    }

    /// Add an external MCP server
    pub fn add_mcp_server(&self, config: crate::mcp::McpServerConfig) -> Result<()> {
        // Only allow adding external servers
        if config.server_type == crate::mcp::McpServerType::Builtin {
            return Err(AetherError::config("Cannot add builtin servers"));
        }

        // Validate required fields
        let command = config
            .command
            .as_ref()
            .ok_or_else(|| AetherError::config("External server requires a command"))?;

        if config.id.is_empty() {
            return Err(AetherError::config("Server ID cannot be empty"));
        }

        // Create external server config
        let external_config = crate::config::McpExternalServerConfig {
            name: config.id.clone(),
            command: command.clone(),
            args: config.args.clone(),
            env: config
                .env
                .into_iter()
                .map(|e| (e.key, e.value))
                .collect(),
            cwd: config.working_directory,
            requires_runtime: None,
            timeout_seconds: 30,
        };

        // Add to config
        {
            let mut cfg = self.lock_config();

            // Check for duplicate
            if cfg.mcp.external_servers.iter().any(|s| s.name == config.id) {
                return Err(AetherError::config(format!(
                    "Server '{}' already exists",
                    config.id
                )));
            }

            cfg.mcp.external_servers.push(external_config);
            cfg.save()?;
        }

        // Notify event handler
        self.event_handler.on_config_changed();
        Ok(())
    }

    /// Update an external MCP server configuration
    ///
    /// Note: Native tools (fs, git, shell, etc.) are now managed via
    /// `update_mcp_config()` and are NOT managed via this method.
    pub fn update_mcp_server(&self, config: crate::mcp::McpServerConfig) -> Result<()> {
        // Only external servers can be updated via this method
        if config.server_type == crate::mcp::McpServerType::Builtin {
            return Err(AetherError::config(
                "Builtin servers are no longer supported. Native tools are managed via AgentTool infrastructure.",
            ));
        }

        let command = config
            .command
            .as_ref()
            .ok_or_else(|| AetherError::config("External server requires a command"))?;

        let mut cfg = self.lock_config();

        // Find and update the server
        let server = cfg
            .mcp
            .external_servers
            .iter_mut()
            .find(|s| s.name == config.id);

        match server {
            Some(s) => {
                s.command = command.clone();
                s.args = config.args;
                s.env = config.env.into_iter().map(|e| (e.key, e.value)).collect();
                s.cwd = config.working_directory;
            }
            None => {
                return Err(AetherError::config(format!(
                    "External server '{}' not found",
                    config.id
                )));
            }
        }

        cfg.save()?;
        drop(cfg);
        self.event_handler.on_config_changed();
        Ok(())
    }

    /// Delete an external MCP server
    ///
    /// Note: Only external servers can be deleted. Native tools are managed
    /// via the AgentTool infrastructure.
    pub fn delete_mcp_server(&self, id: String) -> Result<()> {
        let mut cfg = self.lock_config();

        // Find and remove the external server
        let initial_len = cfg.mcp.external_servers.len();
        cfg.mcp.external_servers.retain(|s| s.name != id);

        if cfg.mcp.external_servers.len() == initial_len {
            return Err(AetherError::config(format!(
                "External server '{}' not found",
                id
            )));
        }

        cfg.save()?;
        drop(cfg);
        self.event_handler.on_config_changed();
        Ok(())
    }

    /// Get MCP server logs
    pub fn get_mcp_server_logs(&self, _id: String, _max_lines: u32) -> Vec<String> {
        // TODO: Implement log collection from server process
        // For now, return empty logs
        Vec::new()
    }

    /// Export MCP configuration as claude_desktop_config.json format
    pub fn export_mcp_config_json(&self) -> String {
        let config = self.lock_config();
        let mut servers = serde_json::Map::new();

        // Export external servers in claude_desktop_config.json format
        for ext in &config.mcp.external_servers {
            let mut server_obj = serde_json::Map::new();
            server_obj.insert("command".to_string(), serde_json::json!(ext.command));
            server_obj.insert("args".to_string(), serde_json::json!(ext.args));

            if !ext.env.is_empty() {
                server_obj.insert("env".to_string(), serde_json::json!(ext.env));
            }

            if let Some(cwd) = &ext.cwd {
                server_obj.insert("cwd".to_string(), serde_json::json!(cwd));
            }

            servers.insert(ext.name.clone(), serde_json::Value::Object(server_obj));
        }

        let export = serde_json::json!({ "mcpServers": servers });
        serde_json::to_string_pretty(&export).unwrap_or_else(|_| "{}".to_string())
    }

    /// Import MCP configuration from claude_desktop_config.json format
    pub fn import_mcp_config_json(&self, json: String) -> Result<()> {
        let parsed: serde_json::Value = serde_json::from_str(&json)
            .map_err(|e| AetherError::config(format!("Invalid JSON: {}", e)))?;

        let servers = parsed
            .get("mcpServers")
            .ok_or_else(|| AetherError::config("Missing 'mcpServers' field"))?
            .as_object()
            .ok_or_else(|| AetherError::config("'mcpServers' must be an object"))?;

        let mut cfg = self.lock_config();

        for (name, server_config) in servers {
            let command = server_config
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    AetherError::config(format!("Server '{}' missing 'command'", name))
                })?;

            let args: Vec<String> = server_config
                .get("args")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            let env: std::collections::HashMap<String, String> = server_config
                .get("env")
                .and_then(|v| v.as_object())
                .map(|obj| {
                    obj.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();

            let cwd = server_config
                .get("cwd")
                .and_then(|v| v.as_str())
                .map(String::from);

            // Check if server already exists
            if let Some(existing) = cfg.mcp.external_servers.iter_mut().find(|s| s.name == *name) {
                // Update existing server
                existing.command = command.to_string();
                existing.args = args;
                existing.env = env;
                existing.cwd = cwd;
            } else {
                // Add new server
                cfg.mcp
                    .external_servers
                    .push(crate::config::McpExternalServerConfig {
                        name: name.clone(),
                        command: command.to_string(),
                        args,
                        env,
                        cwd,
                        requires_runtime: None,
                        timeout_seconds: 30,
                    });
            }
        }

        cfg.save()?;
        drop(cfg);
        self.event_handler.on_config_changed();
        Ok(())
    }
}
