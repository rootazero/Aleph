//! MCP Registrar — collect-then-batch capability registration for MCP plugins
//!
//! MCP registration uses a two-phase pattern:
//! - Phase 1 (async): Probe MCP server for capabilities, collect declarations
//! - Phase 2 (sync): Acquire registry lock, batch-write all declarations
//!
//! This avoids holding RwLockWriteGuard across await points.

use crate::extension::capability::CapabilityDeclaration;
use crate::extension::manifest::PluginPermission;
use crate::extension::registrar::api::CapabilityApi;
use crate::extension::registry::PluginRegistry;
use anyhow::Result;

/// Registrar for MCP-based plugins using collect-then-batch pattern.
pub struct McpRegistrar {
    pub plugin_id: String,
    pub permissions: Vec<PluginPermission>,
}

impl McpRegistrar {
    pub fn new(plugin_id: String, permissions: Vec<PluginPermission>) -> Self {
        Self {
            plugin_id,
            permissions,
        }
    }

    /// Phase 2 (sync): Write collected capabilities into registry.
    /// Lock should be held briefly — caller acquires it just before calling this.
    pub fn batch_register(
        &self,
        caps: Vec<CapabilityDeclaration>,
        registry: &mut PluginRegistry,
    ) -> Result<()> {
        let mut api =
            CapabilityApi::new(registry, self.plugin_id.clone(), self.permissions.clone());
        for cap in caps {
            api.register_capability(cap)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::capability::*;
    use crate::extension::registry::{ServiceRegistration, ToolRegistration};
    use crate::extension::types::{PluginKind, PluginOrigin, PluginRecord};

    fn make_registry_with_plugin(plugin_id: &str) -> PluginRegistry {
        let mut registry = PluginRegistry::new();
        let record = PluginRecord::new(
            plugin_id.to_string(),
            plugin_id.to_string(),
            PluginKind::Mcp,
            PluginOrigin::Global,
        );
        registry.register_plugin(record);
        registry
    }

    #[test]
    fn test_batch_register_tools() {
        let mut registry = make_registry_with_plugin("test-mcp");

        let registrar = McpRegistrar::new("test-mcp".into(), vec![]);
        let caps = vec![CapabilityDeclaration::Tool(ToolRegistration {
            name: "mcp-tool".into(),
            description: "A tool from MCP".into(),
            parameters: serde_json::json!({}),
            handler: "mcp_handler".into(),
            plugin_id: "test-mcp".into(),
        })];

        registrar.batch_register(caps, &mut registry).unwrap();
        assert!(registry.get_tool("mcp-tool").is_some());
    }

    #[test]
    fn test_batch_register_multiple_tools() {
        let mut registry = make_registry_with_plugin("test-mcp");

        let registrar = McpRegistrar::new("test-mcp".into(), vec![]);
        let caps = vec![
            CapabilityDeclaration::Tool(ToolRegistration {
                name: "tool-a".into(),
                description: "Tool A".into(),
                parameters: serde_json::json!({}),
                handler: "handle_a".into(),
                plugin_id: "test-mcp".into(),
            }),
            CapabilityDeclaration::Tool(ToolRegistration {
                name: "tool-b".into(),
                description: "Tool B".into(),
                parameters: serde_json::json!({}),
                handler: "handle_b".into(),
                plugin_id: "test-mcp".into(),
            }),
        ];

        registrar.batch_register(caps, &mut registry).unwrap();
        assert!(registry.get_tool("tool-a").is_some());
        assert!(registry.get_tool("tool-b").is_some());
    }

    #[test]
    fn test_batch_register_permission_check_service() {
        let mut registry = make_registry_with_plugin("test-mcp");

        // No Background permission → Service registration should fail
        let registrar = McpRegistrar::new("test-mcp".into(), vec![]);
        let caps = vec![CapabilityDeclaration::Service(ServiceRegistration {
            id: "test-service".into(),
            name: "Test Service".into(),
            start_handler: "start".into(),
            stop_handler: "stop".into(),
            plugin_id: "test-mcp".into(),
        })];

        assert!(registrar.batch_register(caps, &mut registry).is_err());
    }

    #[test]
    fn test_batch_register_empty_caps() {
        let mut registry = make_registry_with_plugin("test-mcp");
        let registrar = McpRegistrar::new("test-mcp".into(), vec![]);
        // Empty capability list should succeed with no changes
        assert!(registrar.batch_register(vec![], &mut registry).is_ok());
    }
}
