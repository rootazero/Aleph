//! Registration for the unified Extensions Store façade (`extensions.*`).
//!
//! Skills and plugins are reached through process-wide accessors inside the
//! handlers, so only the optional MCP handle and the shared catalog cache are
//! captured here.

use alephcore::extension::marketplace::MarketplaceManager;
use alephcore::gateway::handlers::extensions;
use alephcore::gateway::security::SharedTokenManager;
use alephcore::gateway::GatewayServer;
use alephcore::hub::cache::CatalogCache;
use alephcore::mcp::manager::McpManagerHandle;
use std::sync::Arc;

pub(in crate::commands::start) fn register_extensions_handlers(
    server: &mut GatewayServer,
    mcp: Option<McpManagerHandle>,
    cache: Arc<CatalogCache>,
) {
    {
        let cache = cache.clone();
        let mcp = mcp.clone();
        server
            .handlers_mut()
            .register("extensions.catalog", move |req| {
                let cache = cache.clone();
                let mcp = mcp.clone();
                async move { extensions::catalog::handle_catalog(req, cache, mcp).await }
            });
    }
    {
        let mcp = mcp.clone();
        let cache = cache.clone();
        server
            .handlers_mut()
            .register("extensions.installed", move |req| {
                let mcp = mcp.clone();
                let cache = cache.clone();
                async move { extensions::catalog::handle_installed(req, mcp, cache).await }
            });
    }
    {
        let mcp = mcp.clone();
        server
            .handlers_mut()
            .register("extensions.toggle", move |req| {
                let mcp = mcp.clone();
                async move { extensions::lifecycle::handle_toggle(req, mcp).await }
            });
    }
    {
        let mcp = mcp.clone();
        let cache = cache.clone();
        server
            .handlers_mut()
            .register("extensions.uninstall", move |req| {
                let mcp = mcp.clone();
                let cache = cache.clone();
                async move { extensions::lifecycle::handle_uninstall(req, mcp, cache).await }
            });
    }
}

pub(in crate::commands::start) fn register_extensions_install_handlers(
    server: &mut GatewayServer,
    mcp: Option<McpManagerHandle>,
    cache: Arc<CatalogCache>,
    vault: Arc<SharedTokenManager>,
    marketplace: Arc<MarketplaceManager>,
) {
    {
        let cache = cache.clone();
        server
            .handlers_mut()
            .register("extensions.disclosure", move |req| {
                let cache = cache.clone();
                async move { extensions::install::handle_disclosure(req, cache).await }
            });
    }
    {
        server
            .handlers_mut()
            .register("extensions.install", move |req| {
                let mcp = mcp.clone();
                let cache = cache.clone();
                let vault = vault.clone();
                let marketplace = marketplace.clone();
                async move {
                    extensions::install::handle_install(req, mcp, cache, vault, marketplace).await
                }
            });
    }
}
