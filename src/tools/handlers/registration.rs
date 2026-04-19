//! Registration helpers for MCP + Extension tool handlers (Task 4, Step 4.4/4.5).
//!
//! These helpers encapsulate the "scan → build handler → register" logic so
//! that the MCP connection lifecycle and the Extension plugin-load lifecycle
//! can both call a single entry point when a server connects or a plugin is
//! loaded, and a matching cleanup path when they tear down.
//!
//! The helpers take an `Arc<ToolRegistry>` explicitly rather than owning it,
//! because threading a registry handle through `McpClient` and
//! `ExtensionManager` would require deep plumbing across 5+ modules. Phase 2
//! Task 10 (AppContext wiring) will invoke these helpers from the existing
//! connection/load sites once the registry is plumbed into `AppContext`.

use std::sync::Arc;

use crate::extension::{ExtensionManager, ToolRegistration};
use crate::mcp::{McpClient, McpTool};
use crate::tools::handlers::extension::ExtensionHandler;
use crate::tools::handlers::mcp::McpHandler;
use crate::tools::handlers::ToolHandler;
use crate::tools::registry::ToolRegistry;
use crate::tools::service::ToolSource;

/// Register every tool from one MCP server into the shared `ToolRegistry`.
///
/// Should be invoked *after* a successful `McpClient::start_external_server`
/// or `start_remote_server` for the matching `server_id`. Safe to call
/// repeatedly — collisions log a warning and are skipped. Returns the list of
/// qualified names that were newly registered so the caller can tear them down
/// in a matching `unregister_mcp_tools` on disconnect.
pub fn register_mcp_tools(
    registry: &ToolRegistry,
    client: Arc<McpClient>,
    server_id: &str,
    tools: &[McpTool],
) -> Vec<String> {
    let mut registered = Vec::with_capacity(tools.len());
    for tool in tools {
        let qualified = format!("{}__{}", server_id, tool.name);
        let handler: Arc<dyn ToolHandler> = Arc::new(McpHandler::new(
            Arc::clone(&client),
            server_id.to_string(),
            tool.name.clone(),
            tool.description.clone(),
            tool.input_schema.clone(),
        ));
        match registry.register(qualified.clone(), handler) {
            Ok(()) => registered.push(qualified),
            Err(e) => tracing::warn!(
                error = ?e,
                qualified = %qualified,
                "MCP tool register failed"
            ),
        }
    }
    registered
}

/// Unregister every tool previously registered from the given MCP server.
///
/// Walks the registry snapshot and removes every handler whose
/// `ToolSource` matches `Mcp { server_id }`. Returns the set of qualified
/// names that were removed.
pub fn unregister_mcp_tools(registry: &ToolRegistry, server_id: &str) -> Vec<String> {
    let snapshot = registry.snapshot();
    let victims: Vec<String> = snapshot
        .iter()
        .filter_map(|(name, handler)| match handler.definition().source {
            ToolSource::Mcp { server_id: ref sid } if sid == server_id => Some(name.clone()),
            _ => None,
        })
        .collect();
    drop(snapshot);
    for name in &victims {
        registry.unregister(name);
    }
    victims
}

/// Register every plugin-declared tool from one extension into the shared
/// `ToolRegistry`. Should be invoked after `ExtensionManager::load_runtime_plugin`
/// (or after `load_all` for each plugin that produced tools).
pub fn register_extension_tools(
    registry: &ToolRegistry,
    manager: Arc<ExtensionManager>,
    plugin_id: &str,
    tools: &[ToolRegistration],
) -> Vec<String> {
    let mut registered = Vec::with_capacity(tools.len());
    for tool in tools {
        if tool.plugin_id != plugin_id {
            continue;
        }
        let qualified = format!("ext__{}__{}", plugin_id, tool.name);
        let handler: Arc<dyn ToolHandler> = Arc::new(ExtensionHandler::new(
            Arc::clone(&manager),
            plugin_id.to_string(),
            tool.handler.clone(),
            tool.name.clone(),
            tool.description.clone(),
            tool.parameters.clone(),
        ));
        match registry.register(qualified.clone(), handler) {
            Ok(()) => registered.push(qualified),
            Err(e) => tracing::warn!(
                error = ?e,
                qualified = %qualified,
                "extension tool register failed"
            ),
        }
    }
    registered
}

/// Unregister every tool previously registered from the given extension.
pub fn unregister_extension_tools(registry: &ToolRegistry, plugin_id: &str) -> Vec<String> {
    let snapshot = registry.snapshot();
    let victims: Vec<String> = snapshot
        .iter()
        .filter_map(|(name, handler)| match handler.definition().source {
            ToolSource::Extension {
                plugin_id: ref pid,
            } if pid == plugin_id => Some(name.clone()),
            _ => None,
        })
        .collect();
    drop(snapshot);
    for name in &victims {
        registry.unregister(name);
    }
    victims
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(name: &str, desc: &str) -> McpTool {
        McpTool {
            name: name.into(),
            description: desc.into(),
            input_schema: json!({"type": "object"}),
            requires_confirmation: false,
        }
    }

    #[test]
    fn register_mcp_tools_applies_double_underscore_naming() {
        let reg = ToolRegistry::new();
        let client = Arc::new(McpClient::new());
        let tools = [tool("get_time", "a"), tool("set_tz", "b")];
        let names = register_mcp_tools(&reg, client, "clock", &tools);
        assert_eq!(names, vec!["clock__get_time", "clock__set_tz"]);
        let snap = reg.snapshot();
        assert!(snap.contains_key("clock__get_time"));
        assert!(snap.contains_key("clock__set_tz"));
    }

    #[test]
    fn unregister_mcp_tools_removes_only_matching_server() {
        let reg = ToolRegistry::new();
        let client = Arc::new(McpClient::new());
        register_mcp_tools(&reg, Arc::clone(&client), "alpha", &[tool("x", "d")]);
        register_mcp_tools(&reg, Arc::clone(&client), "beta", &[tool("y", "d")]);
        assert_eq!(reg.snapshot().len(), 2);
        let removed = unregister_mcp_tools(&reg, "alpha");
        assert_eq!(removed, vec!["alpha__x"]);
        let remaining: Vec<String> = reg.snapshot().keys().cloned().collect();
        assert_eq!(remaining, vec!["beta__y"]);
    }

    #[test]
    fn register_mcp_tools_duplicate_is_skipped_with_warning() {
        let reg = ToolRegistry::new();
        let client = Arc::new(McpClient::new());
        let t = [tool("dup", "d")];
        let first = register_mcp_tools(&reg, Arc::clone(&client), "s", &t);
        let second = register_mcp_tools(&reg, client, "s", &t);
        assert_eq!(first, vec!["s__dup"]);
        assert!(second.is_empty());
        assert_eq!(reg.snapshot().len(), 1);
    }
}
