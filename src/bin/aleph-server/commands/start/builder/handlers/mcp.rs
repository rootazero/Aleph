use super::GatewayServer;

use alephcore::mcp::McpManagerHandle;

/// Register the `mcp.*` server-management JSON-RPC handlers against a live
/// [`McpManagerHandle`].
///
/// `McpManagerHandle` is itself cheap to clone (it wraps channel senders), so
/// it does not go through the `register_handler!` macro — that macro assumes
/// `Arc`-wrapped context and would force a needless double-Arc here.
///
/// Called from `start_server()` only when the MCP manager actor spawned
/// successfully; if it did not, these methods stay unregistered and the
/// gateway returns a standard method-not-found error.
pub(in crate::commands::start) fn register_mcp_handlers(
    server: &mut GatewayServer,
    handle: &McpManagerHandle,
) {
    use alephcore::gateway::handlers::mcp;

    macro_rules! reg {
        ($method:expr, $handler:path) => {{
            let handle = handle.clone();
            server.handlers_mut().register($method, move |req| {
                let handle = handle.clone();
                async move { $handler(req, handle).await }
            });
        }};
    }

    // Lifecycle
    reg!("mcp.list", mcp::handle_list);
    reg!("mcp.add", mcp::handle_add);
    reg!("mcp.update", mcp::handle_update);
    reg!("mcp.delete", mcp::handle_delete);
    reg!("mcp.status", mcp::handle_status);
    reg!("mcp.logs", mcp::handle_logs);
    reg!("mcp.start", mcp::handle_start);
    reg!("mcp.stop", mcp::handle_stop);
    reg!("mcp.restart", mcp::handle_restart);

    // Capability aggregation
    reg!("mcp.tools", mcp::handle_list_tools);
    reg!("mcp.resources", mcp::handle_list_resources);
    reg!("mcp.prompts", mcp::handle_list_prompts);
}

/// Register the Settings-page MCP CRUD handlers (`mcp_config.*`) against the
/// live [`McpManagerHandle`] + vault. Like [`register_mcp_handlers`], these use
/// manual closures (the handle is not `Arc`-wrapped). Registered only when the
/// MCP actor spawned; if it did not, the Settings MCP page returns
/// method-not-found — consistent with MCP being unavailable that run.
pub(in crate::commands::start) fn register_mcp_config_handlers(
    server: &mut GatewayServer,
    handle: &McpManagerHandle,
    vault: alephcore::sync_primitives::Arc<alephcore::gateway::security::SharedTokenManager>,
    event_bus: alephcore::sync_primitives::Arc<alephcore::gateway::event_bus::GatewayEventBus>,
) {
    use alephcore::gateway::handlers::mcp_config;

    {
        let handle = handle.clone();
        server
            .handlers_mut()
            .register("mcp_config.list", move |req| {
                let handle = handle.clone();
                async move { mcp_config::handle_list(req, handle).await }
            });
    }
    {
        let handle = handle.clone();
        server
            .handlers_mut()
            .register("mcp_config.get", move |req| {
                let handle = handle.clone();
                async move { mcp_config::handle_get(req, handle).await }
            });
    }
    {
        let handle = handle.clone();
        let vault = vault.clone();
        let event_bus = event_bus.clone();
        server
            .handlers_mut()
            .register("mcp_config.create", move |req| {
                let handle = handle.clone();
                let vault = vault.clone();
                let event_bus = event_bus.clone();
                async move { mcp_config::handle_create(req, handle, vault, event_bus).await }
            });
    }
    {
        let handle = handle.clone();
        let vault = vault.clone();
        let event_bus = event_bus.clone();
        server
            .handlers_mut()
            .register("mcp_config.update", move |req| {
                let handle = handle.clone();
                let vault = vault.clone();
                let event_bus = event_bus.clone();
                async move { mcp_config::handle_update(req, handle, vault, event_bus).await }
            });
    }
    {
        let handle = handle.clone();
        let event_bus = event_bus.clone();
        server
            .handlers_mut()
            .register("mcp_config.delete", move |req| {
                let handle = handle.clone();
                let event_bus = event_bus.clone();
                async move { mcp_config::handle_delete(req, handle, event_bus).await }
            });
    }
}
