//! MCP server management methods for AetherCore
//!
//! This module contains MCP-related methods: list_mcp_servers, add_mcp_server, etc.

use super::{AetherCore, AetherFfiError};
use tracing::info;

impl AetherCore {
    /// Get MCP configuration for Settings UI
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
    pub fn update_mcp_config(&self, new_config: crate::mcp::McpSettingsConfig) -> Result<(), AetherFfiError> {
        let mut config = self.lock_config();

        config.mcp.enabled = new_config.enabled;
        config.tools.fs_enabled = new_config.fs_enabled;
        config.tools.git_enabled = new_config.git_enabled;
        config.tools.shell_enabled = new_config.shell_enabled;
        config.tools.system_info_enabled = new_config.system_info_enabled;
        config.tools.allowed_roots = new_config.allowed_roots;
        config.tools.allowed_repos = new_config.allowed_repos;
        config.tools.allowed_commands = new_config.allowed_commands;
        config.tools.shell_timeout_seconds = new_config.shell_timeout_seconds;

        config.save().map_err(|e| AetherFfiError::Config(e.to_string()))?;
        info!("MCP configuration updated");
        Ok(())
    }

    /// List all external MCP servers
    pub fn list_mcp_servers(&self) -> Vec<crate::mcp::McpServerConfig> {
        let config = self.lock_config();
        let mut servers = Vec::new();

        for ext in &config.mcp.external_servers {
            servers.push(crate::mcp::McpServerConfig {
                id: ext.name.clone(),
                name: ext.name.clone(),
                server_type: crate::mcp::McpServerType::External,
                enabled: true,
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
    pub fn add_mcp_server(&self, config: crate::mcp::McpServerConfig) -> Result<(), AetherFfiError> {
        if config.server_type == crate::mcp::McpServerType::Builtin {
            return Err(AetherFfiError::Config("Cannot add builtin servers".to_string()));
        }

        let command = config
            .command
            .as_ref()
            .ok_or_else(|| AetherFfiError::Config("External server requires a command".to_string()))?;

        if config.id.is_empty() {
            return Err(AetherFfiError::Config("Server ID cannot be empty".to_string()));
        }

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

        let mut cfg = self.lock_config();

        if cfg.mcp.external_servers.iter().any(|s| s.name == config.id) {
            return Err(AetherFfiError::Config(format!(
                "Server '{}' already exists",
                config.id
            )));
        }

        cfg.mcp.external_servers.push(external_config);
        cfg.save().map_err(|e| AetherFfiError::Config(e.to_string()))?;

        info!(server_id = %config.id, "MCP server added");
        Ok(())
    }

    /// Update an external MCP server configuration
    pub fn update_mcp_server(&self, config: crate::mcp::McpServerConfig) -> Result<(), AetherFfiError> {
        if config.server_type == crate::mcp::McpServerType::Builtin {
            return Err(AetherFfiError::Config(
                "Builtin servers cannot be updated via this method".to_string(),
            ));
        }

        let command = config
            .command
            .as_ref()
            .ok_or_else(|| AetherFfiError::Config("External server requires a command".to_string()))?;

        let mut cfg = self.lock_config();

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
                return Err(AetherFfiError::Config(format!(
                    "External server '{}' not found",
                    config.id
                )));
            }
        }

        cfg.save().map_err(|e| AetherFfiError::Config(e.to_string()))?;
        info!(server_id = %config.id, "MCP server updated");
        Ok(())
    }

    /// Delete an external MCP server
    pub fn delete_mcp_server(&self, id: String) -> Result<(), AetherFfiError> {
        let mut cfg = self.lock_config();

        let initial_len = cfg.mcp.external_servers.len();
        cfg.mcp.external_servers.retain(|s| s.name != id);

        if cfg.mcp.external_servers.len() == initial_len {
            return Err(AetherFfiError::Config(format!(
                "External server '{}' not found",
                id
            )));
        }

        cfg.save().map_err(|e| AetherFfiError::Config(e.to_string()))?;
        info!(server_id = %id, "MCP server deleted");
        Ok(())
    }

    /// Get MCP server logs
    pub fn get_mcp_server_logs(&self, _id: String, _max_lines: u32) -> Vec<String> {
        // TODO: Implement log collection from server process
        Vec::new()
    }

    /// Export MCP configuration as claude_desktop_config.json format
    pub fn export_mcp_config_json(&self) -> String {
        let config = self.lock_config();
        let mut servers = serde_json::Map::new();

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
    pub fn import_mcp_config_json(&self, json: String) -> Result<(), AetherFfiError> {
        let parsed: serde_json::Value = serde_json::from_str(&json)
            .map_err(|e| AetherFfiError::Config(format!("Invalid JSON: {}", e)))?;

        let servers = parsed
            .get("mcpServers")
            .ok_or_else(|| AetherFfiError::Config("Missing 'mcpServers' field".to_string()))?
            .as_object()
            .ok_or_else(|| AetherFfiError::Config("'mcpServers' must be an object".to_string()))?;

        let mut cfg = self.lock_config();

        for (name, server_config) in servers {
            let command = server_config
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    AetherFfiError::Config(format!("Server '{}' missing 'command'", name))
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

            if let Some(existing) = cfg.mcp.external_servers.iter_mut().find(|s| s.name == *name) {
                existing.command = command.to_string();
                existing.args = args;
                existing.env = env;
                existing.cwd = cwd;
            } else {
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

        cfg.save().map_err(|e| AetherFfiError::Config(e.to_string()))?;
        info!("MCP configuration imported");
        Ok(())
    }
}
