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

// -- P3 Stage I — per-agent MCP scope ----------------------------------------

/// Errors raised while provisioning or tearing down an [`McpScope`].
///
/// All variants are fail-loud: `subagent_spawner::spawn` maps any
/// `McpScopeError` to `"sub-agent failed: mcp scope: {err}"` and returns
/// `Err` (no fallback to global-only behavior).
#[derive(Debug, thiserror::Error)]
pub enum McpScopeError {
    #[error("name '{0}' is reserved by global registry; inline servers must use a fresh name")]
    NameConflict(String),
    #[error("reference '{0}' not found in global registry")]
    ReferenceNotFound(String),
    #[error("inline server '{name}' failed to start: {reason}")]
    InlineStartup { name: String, reason: String },
    #[error("inline server '{name}' failed to shut down: {reason}")]
    InlineShutdown { name: String, reason: String },
}

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// RAII handle for a single inline MCP server process spawned for one
/// subagent's lifetime (P3 Stage I).
///
/// Production callers should construct via `McpScope::provision`; the
/// `new_for_test` constructor is `pub(crate)` and exists only for unit
/// tests of the Drop safety-net wiring (Task 5). The real `process`
/// field is an `Option<crate::mcp::external::McpServerConnection>` — see
/// Task 7 for the full implementation.
pub struct InlineMcpHandle {
    pub(crate) name: String,
    /// `None` in `new_for_test`; `Some(_)` after a successful spawn.
    pub(crate) process: Option<crate::mcp::external::McpServerConnection>,
    pub(crate) cleaned_up: Arc<AtomicBool>,
}

impl InlineMcpHandle {
    #[cfg(test)]
    pub(crate) fn new_for_test(name: String) -> Self {
        Self {
            name,
            process: None,
            cleaned_up: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Mark the handle as already cleaned up so `Drop` skips the safety-net.
    /// Called by `McpScope::shutdown` on the explicit-cleanup path.
    pub(crate) fn mark_cleaned(&self) {
        self.cleaned_up.store(true, Ordering::Release);
    }
}

impl Drop for InlineMcpHandle {
    fn drop(&mut self) {
        if self.cleaned_up.load(Ordering::Acquire) {
            return;
        }
        // Safety net: process leaked through cancel/panic/timeout. Log via
        // tracing; do NOT panic from Drop. The actual kill happens in Task 7
        // once the McpServerConnection field is wired through provision().
        tracing::error!(
            name = %self.name,
            "InlineMcpHandle leaked — Drop safety-net firing"
        );
        if let Some(_proc) = self.process.take() {
            // Placeholder: Task 7 swaps this for a sync kill path
            // (std::thread::spawn → connection.shutdown()).
        }
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

    #[test]
    fn mcp_scope_error_displays_name_conflict() {
        let e = McpScopeError::NameConflict("github".into());
        let s = format!("{e}");
        assert!(s.contains("name 'github'"));
        assert!(s.contains("global registry"));
    }

    #[test]
    fn mcp_scope_error_displays_reference_not_found() {
        let e = McpScopeError::ReferenceNotFound("missing".into());
        assert!(format!("{e}").contains("reference 'missing' not found"));
    }

    #[test]
    fn mcp_scope_error_displays_inline_startup() {
        let e = McpScopeError::InlineStartup {
            name: "fresh".into(),
            reason: "exec failed: ENOENT".into(),
        };
        let s = format!("{e}");
        assert!(s.contains("inline server 'fresh'"));
        assert!(s.contains("ENOENT"));
    }

    #[test]
    fn mcp_scope_error_displays_inline_shutdown() {
        let e = McpScopeError::InlineShutdown {
            name: "fresh".into(),
            reason: "kill -TERM timed out".into(),
        };
        assert!(format!("{e}").contains("failed to shut down"));
    }

    #[test]
    fn inline_mcp_handle_drop_without_cleanup_logs_leak() {
        use std::sync::atomic::Ordering;
        let handle = InlineMcpHandle::new_for_test("zombie".into());
        let cleaned = handle.cleaned_up.clone();
        drop(handle);
        assert!(!cleaned.load(Ordering::Acquire), "no explicit cleanup → flag stays false");
    }

    #[test]
    fn inline_mcp_handle_mark_cleaned_skips_drop_safety_net() {
        use std::sync::atomic::Ordering;
        let handle = InlineMcpHandle::new_for_test("clean".into());
        let cleaned = handle.cleaned_up.clone();
        handle.mark_cleaned();
        drop(handle);
        assert!(cleaned.load(Ordering::Acquire), "explicit cleanup must flip the flag");
    }
}
