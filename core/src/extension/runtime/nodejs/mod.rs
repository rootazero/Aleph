//! Node.js Plugin Runtime
//!
//! Manages Node.js subprocess for executing TypeScript/JavaScript plugins
//! via JSON-RPC 2.0 over stdio.
//!
//! This module provides a synchronous blocking I/O approach for simpler
//! use cases, complementing the async `PluginRuntime` in the parent module.

pub mod ipc;
pub mod process;

pub use ipc::*;
pub use process::{HostScript, NodeProcess, DEFAULT_IPC_TIMEOUT};

use std::collections::HashMap;
use tracing::{error, info};

use crate::extension::error::ExtensionError;
use crate::extension::manifest::PluginManifest;
use crate::extension::registry::{HookRegistration, PluginHookEvent, ToolRegistration};

/// Node.js runtime manager
pub struct NodeJsRuntime {
    /// Running plugin processes
    processes: HashMap<String, NodeProcess>,
    /// Path to Node.js binary
    node_path: String,
    /// Path to plugin host script (empty when using embedded)
    host_script_path: String,
    /// Whether to use embedded plugin-host.js
    use_embedded_host: bool,
}

impl NodeJsRuntime {
    /// Create a new Node.js runtime with external host script
    pub fn new(node_path: impl Into<String>, host_script_path: impl Into<String>) -> Self {
        Self {
            processes: HashMap::new(),
            node_path: node_path.into(),
            host_script_path: host_script_path.into(),
            use_embedded_host: false,
        }
    }

    /// Create runtime with embedded plugin-host.js
    ///
    /// This is the recommended way to create the runtime as it uses the
    /// bundled plugin-host.js script without requiring an external file.
    pub fn with_embedded_host(node_path: impl Into<String>) -> Self {
        Self {
            processes: HashMap::new(),
            node_path: node_path.into(),
            host_script_path: String::new(), // Not used when embedded
            use_embedded_host: true,
        }
    }

    /// Load a Node.js plugin
    pub fn load_plugin(
        &mut self,
        manifest: &PluginManifest,
    ) -> Result<PluginRegistrationParams, ExtensionError> {
        let entry_path = manifest.entry_path();

        if !entry_path.exists() {
            return Err(ExtensionError::Runtime(format!(
                "Plugin entry not found: {:?}",
                entry_path
            )));
        }

        info!(
            "Loading Node.js plugin: {} from {:?}",
            manifest.id, entry_path
        );

        // Choose host script source
        let host_script = if self.use_embedded_host {
            HostScript::Embedded(include_str!("plugin-host.js"))
        } else {
            HostScript::Path(self.host_script_path.clone())
        };

        let mut process = NodeProcess::start(
            &self.node_path,
            host_script,
            entry_path.to_str().unwrap_or(""),
            &manifest.id,
        )?;

        // Call load method to get registrations
        let response = process.call(
            "load",
            serde_json::json!({
                "pluginId": manifest.id,
                "pluginPath": entry_path,
            }),
        )?;

        if !response.is_success() {
            let err = response.error.map(|e| e.message).unwrap_or_default();
            return Err(ExtensionError::Runtime(format!(
                "Plugin load failed: {}",
                err
            )));
        }

        let registrations: PluginRegistrationParams = response
            .result
            .map(|r| serde_json::from_value(r))
            .transpose()
            .map_err(|e| ExtensionError::Runtime(format!("Invalid registration: {}", e)))?
            .unwrap_or_else(|| PluginRegistrationParams {
                plugin_id: manifest.id.clone(),
                tools: vec![],
                hooks: vec![],
                channels: vec![],
                providers: vec![],
                gateway_methods: vec![],
            });

        self.processes.insert(manifest.id.clone(), process);

        Ok(registrations)
    }

    /// Unload a plugin
    pub fn unload_plugin(&mut self, plugin_id: &str) -> Result<(), ExtensionError> {
        if let Some(mut process) = self.processes.remove(plugin_id) {
            process.shutdown()?;
        }
        Ok(())
    }

    /// Call a tool handler
    pub fn call_tool(
        &mut self,
        plugin_id: &str,
        handler: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, ExtensionError> {
        let process = self
            .processes
            .get_mut(plugin_id)
            .ok_or_else(|| ExtensionError::PluginNotFound(plugin_id.to_string()))?;

        let response = process.call(
            "plugin.call",
            serde_json::json!({
                "pluginId": plugin_id,
                "handler": handler,
                "args": args,
            }),
        )?;

        if let Some(error) = response.error {
            return Err(ExtensionError::Runtime(error.message));
        }

        Ok(response.result.unwrap_or(serde_json::Value::Null))
    }

    /// Execute a hook handler
    pub fn execute_hook(
        &mut self,
        plugin_id: &str,
        handler: &str,
        event_data: serde_json::Value,
    ) -> Result<serde_json::Value, ExtensionError> {
        let process = self
            .processes
            .get_mut(plugin_id)
            .ok_or_else(|| ExtensionError::PluginNotFound(plugin_id.to_string()))?;

        let response = process.call(
            "executeHook",
            serde_json::json!({
                "pluginId": plugin_id,
                "handler": handler,
                "event": event_data,
            }),
        )?;

        if let Some(error) = response.error {
            return Err(ExtensionError::Runtime(error.message));
        }

        Ok(response.result.unwrap_or(serde_json::Value::Null))
    }

    /// Check if a plugin is loaded
    pub fn is_loaded(&self, plugin_id: &str) -> bool {
        self.processes.contains_key(plugin_id)
    }

    /// Get list of loaded plugins
    pub fn loaded_plugins(&self) -> Vec<&str> {
        self.processes.keys().map(|s| s.as_str()).collect()
    }

    /// Shutdown all plugins
    pub fn shutdown_all(&mut self) {
        for (id, mut process) in self.processes.drain() {
            if let Err(e) = process.shutdown() {
                error!("Failed to shutdown plugin {}: {}", id, e);
            }
        }
    }
}

impl Drop for NodeJsRuntime {
    fn drop(&mut self) {
        self.shutdown_all();
    }
}

/// Convert IPC tool definition to ToolRegistration
pub fn tool_def_to_registration(def: &ToolDefinition, plugin_id: &str) -> ToolRegistration {
    ToolRegistration {
        name: def.name.clone(),
        description: def.description.clone(),
        parameters: def.parameters.clone(),
        handler: def.handler.clone(),
        plugin_id: plugin_id.to_string(),
    }
}

/// Convert IPC hook definition to HookRegistration
pub fn hook_def_to_registration(def: &HookDefinition, plugin_id: &str) -> Option<HookRegistration> {
    let event = match def.event.as_str() {
        "before_agent_start" => PluginHookEvent::BeforeAgentStart,
        "agent_end" => PluginHookEvent::AgentEnd,
        "before_tool_call" => PluginHookEvent::BeforeToolCall,
        "after_tool_call" => PluginHookEvent::AfterToolCall,
        "message_received" => PluginHookEvent::MessageReceived,
        "message_sending" => PluginHookEvent::MessageSending,
        "session_start" => PluginHookEvent::SessionStart,
        "session_end" => PluginHookEvent::SessionEnd,
        _ => return None,
    };

    Some(HookRegistration {
        event,
        priority: def.priority,
        handler: def.handler.clone(),
        name: None,
        description: None,
        plugin_id: plugin_id.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nodejs_runtime_new() {
        let runtime = NodeJsRuntime::new("/usr/bin/node", "/path/to/host.js");
        assert!(runtime.loaded_plugins().is_empty());
        assert!(!runtime.use_embedded_host);
    }

    #[test]
    fn test_nodejs_runtime_with_embedded_host() {
        let runtime = NodeJsRuntime::with_embedded_host("/usr/bin/node");
        assert!(runtime.loaded_plugins().is_empty());
        assert!(runtime.use_embedded_host);
        assert!(runtime.host_script_path.is_empty());
    }

    #[test]
    fn test_is_loaded_false() {
        let runtime = NodeJsRuntime::new("/usr/bin/node", "/path/to/host.js");
        assert!(!runtime.is_loaded("nonexistent"));
    }

    #[test]
    fn test_is_loaded_false_embedded() {
        let runtime = NodeJsRuntime::with_embedded_host("/usr/bin/node");
        assert!(!runtime.is_loaded("nonexistent"));
    }

    #[test]
    fn test_tool_def_to_registration() {
        let def = ToolDefinition {
            name: "my_tool".to_string(),
            description: "A test tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            handler: "handleMyTool".to_string(),
        };

        let reg = tool_def_to_registration(&def, "test-plugin");
        assert_eq!(reg.name, "my_tool");
        assert_eq!(reg.description, "A test tool");
        assert_eq!(reg.handler, "handleMyTool");
        assert_eq!(reg.plugin_id, "test-plugin");
    }

    #[test]
    fn test_hook_def_to_registration_valid() {
        let def = HookDefinition {
            event: "before_agent_start".to_string(),
            priority: 10,
            handler: "onStart".to_string(),
        };

        let reg = hook_def_to_registration(&def, "test-plugin");
        assert!(reg.is_some());
        let reg = reg.unwrap();
        assert_eq!(reg.event, PluginHookEvent::BeforeAgentStart);
        assert_eq!(reg.priority, 10);
        assert_eq!(reg.handler, "onStart");
        assert_eq!(reg.plugin_id, "test-plugin");
    }

    #[test]
    fn test_hook_def_to_registration_invalid_event() {
        let def = HookDefinition {
            event: "unknown_event".to_string(),
            priority: 0,
            handler: "onUnknown".to_string(),
        };

        let reg = hook_def_to_registration(&def, "test-plugin");
        assert!(reg.is_none());
    }

    #[test]
    fn test_hook_def_to_registration_all_events() {
        let events = vec![
            ("before_agent_start", PluginHookEvent::BeforeAgentStart),
            ("agent_end", PluginHookEvent::AgentEnd),
            ("before_tool_call", PluginHookEvent::BeforeToolCall),
            ("after_tool_call", PluginHookEvent::AfterToolCall),
            ("message_received", PluginHookEvent::MessageReceived),
            ("message_sending", PluginHookEvent::MessageSending),
            ("session_start", PluginHookEvent::SessionStart),
            ("session_end", PluginHookEvent::SessionEnd),
        ];

        for (event_str, expected_event) in events {
            let def = HookDefinition {
                event: event_str.to_string(),
                priority: 0,
                handler: "handler".to_string(),
            };
            let reg = hook_def_to_registration(&def, "plugin");
            assert!(reg.is_some(), "Event '{}' should be valid", event_str);
            assert_eq!(reg.unwrap().event, expected_event);
        }
    }

    #[test]
    fn test_plugin_host_script_exists() {
        let script = include_str!("plugin-host.js");
        assert!(script.contains("jsonrpc"));
        assert!(script.contains("load"));
        assert!(script.contains("plugin.call"));
    }
}
