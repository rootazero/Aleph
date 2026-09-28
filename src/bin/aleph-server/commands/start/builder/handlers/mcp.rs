use super::GatewayServer;

use alephcore::mcp::McpManagerHandle;

/// Register the `mcp.list` JSON-RPC handler against a live
/// [`McpManagerHandle`].
///
/// `McpManagerHandle` is itself cheap to clone (it wraps channel senders), so
/// it does not go through the `register_handler!` macro — that macro assumes
/// `Arc`-wrapped context and would force a needless double-Arc here.
///
/// Called from `start_server()` only when the MCP manager actor spawned
/// successfully; if it did not, the method stays unregistered and the
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

    // `mcp.list` is the only server-management verb with a client (the CLI's
    // `doctor`). The other eleven (`add/update/delete/status/logs/start/stop/
    // restart`, `tools/resources/prompts`) were cut 2026-09-20: zero clients;
    // persistent CRUD lives on `mcp_config.*`, the model already sees prompts /
    // resources through `mcp_list_*` tools and tools through the catalog.
    reg!("mcp.list", mcp::handle_list);
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

#[cfg(test)]
mod tests {
    /// The eleven `mcp.*` management / aggregation RPCs had zero clients
    /// (user ruling 2026-09-20). Only `mcp.list` — the CLI's `doctor` calls
    /// it — is registered from this file. Source-level because
    /// `register_mcp_handlers` needs a live actor to run.
    #[test]
    fn only_mcp_list_is_registered_here() {
        let src = include_str!("mcp.rs");
        // Production half only: this test's own text names the macro.
        let production = src.split("#[cfg(test)]").next().unwrap_or(src);
        let registered: Vec<&str> = production
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .filter(|l| l.contains("reg!("))
            .collect();
        assert_eq!(
            registered.len(),
            1,
            "exactly one reg!() line expected (mcp.list); found:\n{}",
            registered.join("\n")
        );
        assert!(
            registered[0].contains("\"mcp.list\""),
            "the surviving registration must be mcp.list, got: {}",
            registered[0]
        );
    }
}
