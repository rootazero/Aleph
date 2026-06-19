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

    // Preset catalog (built-in recommended MCP servers)
    reg!("mcp.list_presets", mcp::handle_list_presets);
    reg!("mcp.install_preset", mcp::handle_install_preset);
}
