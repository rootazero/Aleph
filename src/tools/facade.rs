//! Compose the `ToolService` decorator chain at startup.
//!
//! Phase 2 Task 10: wires the full chain once the runtime has:
//!   - an `AlephToolServer` containing discovered builtin tools,
//!   - a `ToolServiceConfig` carrying timeout tunables.
//!
//! The chain order (top → bottom) is:
//!
//! ```text
//! ExecAuditLayer
//!   └─ PermissionLayer (SmartFilter + ApprovalGate; filter/approver are
//!                        swapped in later by callers via set_smart_filter /
//!                        set_approver as the policies land)
//!        └─ ContextRuleLayer (no rules installed; future policy hook)
//!             └─ TimeoutLayer (default + per-tool overrides from config)
//!                  └─ CoreDispatch → ToolRegistry
//! ```
//!
//! Builtin tools are drained from `AlephToolServer::all_builtin_handlers()`,
//! wrapped in `BuiltinHandler`, and inserted into the shared `ToolRegistry`.
//! The returned `(Arc<dyn ToolService>, Arc<ToolRegistry>)` tuple lets the
//! caller thread the registry into `McpClient::set_tool_registry` /
//! `ExtensionManager::set_tool_registry` so subsequent MCP connections and
//! plugin loads extend the same registry.

use std::sync::Arc;

use crate::config::types::tools::ToolServiceConfig;
use crate::tools::dispatch::CoreDispatch;
use crate::tools::handlers::builtin::BuiltinHandler;
use crate::tools::handlers::ToolHandler;
use crate::tools::middleware::{ContextRuleLayer, ExecAuditLayer, PermissionLayer, TimeoutLayer};
use crate::tools::registry::ToolRegistry;
use crate::tools::service::ToolService;
use crate::tools::AlephToolServer;

/// Compose the full `ToolService` decorator chain from a populated
/// `AlephToolServer` and a `ToolServiceConfig`.
///
/// Returns both the top-of-chain `Arc<dyn ToolService>` (what agent_loop
/// consumes) and the shared `Arc<ToolRegistry>` (what MCP / Extension
/// lifecycles mutate).
///
/// Callers that need to attach a live `SmartFilter` / `Approver` should use
/// [`build_tool_service_with_handles`] instead — it exposes the inner
/// `PermissionLayer` so Phase 3 boot wiring can call `set_smart_filter` /
/// `set_approver` without downcasting.
pub async fn build_tool_service(
    server: Arc<AlephToolServer>,
    config: &ToolServiceConfig,
) -> (Arc<dyn ToolService>, Arc<ToolRegistry>) {
    let (svc, registry, _handles) = build_tool_service_with_handles(server, config).await;
    (svc, registry)
}

/// Handles into the tool-service decorator chain. Exposed so Phase 3 boot
/// wiring can attach a live `LayeredPermissionResolver` + `ApprovalGate`
/// approver to the `PermissionLayer` after construction.
///
/// The `PermissionLayer` is kept as `Arc<PermissionLayer>` (concrete type)
/// rather than `Arc<dyn ToolService>` so its `set_smart_filter` /
/// `set_approver` setters remain reachable without a downcast.
pub struct ToolServiceHandles {
    pub permission_layer: Arc<PermissionLayer>,
}

/// Variant of [`build_tool_service`] that also returns typed handles to
/// inner middleware layers. The returned `Arc<dyn ToolService>` is the
/// same top-of-chain service — it wraps the exact `PermissionLayer` that
/// `handles.permission_layer` points to.
pub async fn build_tool_service_with_handles(
    server: Arc<AlephToolServer>,
    config: &ToolServiceConfig,
) -> (Arc<dyn ToolService>, Arc<ToolRegistry>, ToolServiceHandles) {
    let registry = Arc::new(ToolRegistry::new());
    register_builtins_into(&registry, &server).await;

    let core: Arc<dyn ToolService> = Arc::new(CoreDispatch::new(registry.clone()));
    let timeout: Arc<dyn ToolService> = Arc::new(TimeoutLayer::new(
        core,
        config.default_timeout(),
        config.per_tool_durations(),
    ));
    let ctxrule: Arc<dyn ToolService> = Arc::new(ContextRuleLayer::new(timeout));
    let permission_layer = Arc::new(PermissionLayer::new(ctxrule));
    let perm: Arc<dyn ToolService> = permission_layer.clone();
    let audit: Arc<dyn ToolService> = Arc::new(ExecAuditLayer::new(perm));
    let handles = ToolServiceHandles { permission_layer };
    (audit, registry, handles)
}

async fn register_builtins_into(registry: &ToolRegistry, server: &AlephToolServer) {
    for (name, tool) in server.all_builtin_handlers().await {
        let handler: Arc<dyn ToolHandler> = Arc::new(BuiltinHandler::new(name.clone(), tool));
        if let Err(e) = registry.register(name.clone(), handler) {
            tracing::warn!(error = ?e, tool = %name, "builtin register failed");
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn build_on_empty_server_yields_empty_list() {
        let server = Arc::new(AlephToolServer::new());
        let config = ToolServiceConfig::default();
        let (svc, registry) = build_tool_service(server, &config).await;
        assert!(svc.list().await.is_empty());
        assert_eq!(registry.snapshot().len(), 0);
    }

    #[tokio::test]
    async fn builtins_flow_through_to_list() {
        use crate::tools::AlephTool;
        use async_trait::async_trait;
        use schemars::JsonSchema;
        use serde::{Deserialize, Serialize};

        #[derive(Clone)]
        struct Echo;

        #[derive(Serialize, Deserialize, JsonSchema)]
        struct Args {
            msg: String,
        }

        #[derive(Serialize)]
        struct Out {
            echoed: String,
        }

        #[async_trait]
        impl AlephTool for Echo {
            const NAME: &'static str = "echo_facade";
            const DESCRIPTION: &'static str = "echo back";
            type Args = Args;
            type Output = Out;

            async fn call(&self, args: Self::Args) -> crate::error::Result<Self::Output> {
                Ok(Out { echoed: args.msg })
            }
        }

        let server = AlephToolServer::new().tool(Echo);
        let server = Arc::new(server);
        let (svc, registry) = build_tool_service(server, &ToolServiceConfig::default()).await;

        let names: Vec<String> = svc.list().await.into_iter().map(|d| d.name).collect();
        assert_eq!(names, vec!["echo_facade".to_string()]);
        assert!(registry.snapshot().contains_key("echo_facade"));
    }
}
