//! Registration helpers for MCP tool handlers.
//!
//! Encapsulates the "scan → build handler → register" logic so that the MCP
//! connection lifecycle can call a single entry point when a server connects,
//! and a matching cleanup path when it tears down. Extension/plugin variants
//! were removed 2026-05-20 — see `tools::handlers::mod` for rationale.

use std::sync::Arc;

use crate::mcp::{McpClient, McpTool};
use crate::tool_metadata::ToolCatalog;
use crate::tools::handlers::mcp::McpHandler;
use crate::tools::handlers::ToolHandler;
use crate::tools::probes::mcp::McpServerProbe;
use crate::tools::registry::ToolHandlerRegistry;
use crate::tools::service::ToolSource;

/// Register every tool from one MCP server into the shared executor `ToolHandlerRegistry`,
/// and (when a tool catalog is supplied) attach a single
/// [`McpServerProbe`] per qualified name so the `<tool_runtime_state>` block
/// can surface a "server transport down" hint to the LLM.
///
/// Should be invoked *after* a successful `McpClient::start_external_server`
/// or `start_remote_server` for the matching `server_id`. Safe to call
/// repeatedly — collisions log a warning and are skipped. Returns the list of
/// qualified names that were newly registered so the caller can tear them down
/// in a matching `unregister_mcp_tools` on disconnect.
pub fn register_mcp_tools(
    registry: &ToolHandlerRegistry,
    tool_catalog: Option<&Arc<ToolCatalog>>,
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
            Ok(()) => {
                if let Some(disp) = tool_catalog {
                    disp.register_health_probe(
                        qualified.clone(),
                        Arc::new(McpServerProbe::new(Arc::clone(&client), server_id)),
                    );
                }
                registered.push(qualified);
            }
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
/// `ToolSource` matches `Mcp { server_id }`. When `tool_catalog` is
/// supplied, also tears down the matching health probes. Returns the set of
/// qualified names that were removed.
pub fn unregister_mcp_tools(
    registry: &ToolHandlerRegistry,
    tool_catalog: Option<&Arc<ToolCatalog>>,
    server_id: &str,
) -> Vec<String> {
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
        if let Some(disp) = tool_catalog {
            disp.health().unregister_probe(name);
        }
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
        let reg = ToolHandlerRegistry::new();
        let client = Arc::new(McpClient::new());
        let tools = [tool("get_time", "a"), tool("set_tz", "b")];
        let names = register_mcp_tools(&reg, None, client, "clock", &tools);
        assert_eq!(names, vec!["clock__get_time", "clock__set_tz"]);
        let snap = reg.snapshot();
        assert!(snap.contains_key("clock__get_time"));
        assert!(snap.contains_key("clock__set_tz"));
    }

    #[test]
    fn unregister_mcp_tools_removes_only_matching_server() {
        let reg = ToolHandlerRegistry::new();
        let client = Arc::new(McpClient::new());
        register_mcp_tools(&reg, None, Arc::clone(&client), "alpha", &[tool("x", "d")]);
        register_mcp_tools(&reg, None, Arc::clone(&client), "beta", &[tool("y", "d")]);
        assert_eq!(reg.snapshot().len(), 2);
        let removed = unregister_mcp_tools(&reg, None, "alpha");
        assert_eq!(removed, vec!["alpha__x"]);
        let remaining: Vec<String> = reg.snapshot().keys().cloned().collect();
        assert_eq!(remaining, vec!["beta__y"]);
    }

    #[test]
    fn register_mcp_tools_duplicate_is_skipped_with_warning() {
        let reg = ToolHandlerRegistry::new();
        let client = Arc::new(McpClient::new());
        let t = [tool("dup", "d")];
        let first = register_mcp_tools(&reg, None, Arc::clone(&client), "s", &t);
        let second = register_mcp_tools(&reg, None, client, "s", &t);
        assert_eq!(first, vec!["s__dup"]);
        assert!(second.is_empty());
        assert_eq!(reg.snapshot().len(), 1);
    }

    #[test]
    fn register_with_tool_catalog_attaches_probe() {
        let reg = ToolHandlerRegistry::new();
        let disp = Arc::new(ToolCatalog::new());
        let client = Arc::new(McpClient::new());
        register_mcp_tools(
            &reg,
            Some(&disp),
            client,
            "srv",
            &[tool("a", "d"), tool("b", "d")],
        );
        // No public introspection of registered probes; instead, force a
        // refresh and observe that an entry materialises (since fresh
        // McpClient reports no live servers, the probe returns Unhealthy).
        let cache = disp.health();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let _ = cache.refresh("srv__a").await;
        });
        let snap = cache.snapshot();
        // `is_healthy` returns true for empty entries; an entry now exists
        // and reports unhealthy, so the snapshot's `reason` is Some.
        assert!(snap.reason("srv__a").is_some());
    }

    #[test]
    fn unregister_with_tool_catalog_drops_probe() {
        let reg = ToolHandlerRegistry::new();
        let disp = Arc::new(ToolCatalog::new());
        let client = Arc::new(McpClient::new());
        register_mcp_tools(&reg, Some(&disp), client, "srv", &[tool("a", "d")]);
        let removed = unregister_mcp_tools(&reg, Some(&disp), "srv");
        assert_eq!(removed, vec!["srv__a"]);
        // Re-registering immediately should not collide with a leftover probe
        // (no public assertion on probe count exists, but unregister_probe
        // returns true only when a probe was actually removed; a second
        // unregister returns false).
        assert!(!disp.health().unregister_probe("srv__a"));
    }
}
