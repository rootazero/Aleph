//! Registration for the unified Extensions Store façade (`extensions.*`).
//!
//! Skills and plugins are reached through process-wide accessors inside the
//! handlers, so only the optional MCP handle and the shared catalog cache are
//! captured here.

use alephcore::extension::marketplace::MarketplaceManager;
use alephcore::gateway::handlers::extensions;
use alephcore::gateway::security::SharedTokenManager;
use alephcore::gateway::GatewayServer;
use alephcore::mcp::manager::McpManagerHandle;
use alephcore::store::cache::CatalogCache;
use alephcore::store::provider::ProviderRegistry;
use std::sync::Arc;

pub(in crate::commands::start) fn register_extensions_handlers(
    server: &mut GatewayServer,
    mcp: Option<McpManagerHandle>,
    cache: Arc<CatalogCache>,
) {
    {
        let cache = cache.clone();
        server.handlers_mut().register("extensions.catalog", move |req| {
            let cache = cache.clone();
            async move { extensions::catalog::handle_catalog(req, cache).await }
        });
    }
    {
        let mcp = mcp.clone();
        server.handlers_mut().register("extensions.installed", move |req| {
            let mcp = mcp.clone();
            async move { extensions::catalog::handle_installed(req, mcp).await }
        });
    }
    {
        let mcp = mcp.clone();
        server.handlers_mut().register("extensions.toggle", move |req| {
            let mcp = mcp.clone();
            async move { extensions::lifecycle::handle_toggle(req, mcp).await }
        });
    }
    {
        let mcp = mcp.clone();
        server.handlers_mut().register("extensions.uninstall", move |req| {
            let mcp = mcp.clone();
            async move { extensions::lifecycle::handle_uninstall(req, mcp).await }
        });
    }
}

pub(in crate::commands::start) fn register_extensions_install_handlers(
    server: &mut GatewayServer,
    mcp: Option<McpManagerHandle>,
    cache: Arc<CatalogCache>,
    registry: Arc<ProviderRegistry>,
    vault: Arc<SharedTokenManager>,
    marketplace: Arc<MarketplaceManager>,
) {
    {
        let cache = cache.clone();
        let registry = registry.clone();
        server.handlers_mut().register("extensions.disclosure", move |req| {
            let cache = cache.clone();
            let registry = registry.clone();
            async move { extensions::install::handle_disclosure(req, cache, registry).await }
        });
    }
    {
        let cache = cache.clone();
        let registry = registry.clone();
        server.handlers_mut().register("extensions.configure", move |req| {
            let cache = cache.clone();
            let registry = registry.clone();
            async move { extensions::install::handle_configure(req, cache, registry).await }
        });
    }
    {
        server.handlers_mut().register("extensions.install", move |req| {
            let mcp = mcp.clone();
            let cache = cache.clone();
            let registry = registry.clone();
            let vault = vault.clone();
            let marketplace = marketplace.clone();
            async move {
                extensions::install::handle_install(req, mcp, cache, registry, vault, marketplace)
                    .await
            }
        });
    }
}

pub(in crate::commands::start) fn register_extensions_sources_handlers(
    server: &mut GatewayServer,
    registry: Arc<ProviderRegistry>,
    cache: Arc<CatalogCache>,
) {
    {
        let registry = registry.clone();
        server.handlers_mut().register("extensions.sources.list", move |req| {
            let registry = registry.clone();
            async move { extensions::sources::handle_list(req, registry).await }
        });
    }
    {
        server.handlers_mut().register("extensions.sources.refresh", move |req| {
            let registry = registry.clone();
            let cache = cache.clone();
            async move { extensions::sources::handle_refresh(req, registry, cache).await }
        });
    }
}
