use crate::gateway::handlers::parse_params;
use crate::gateway::handlers::plugins::handlers::{build_marketplace_manager, get_extension_manager};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use serde_json::json;

/// List all registered marketplaces (including built-in)
pub async fn handle_marketplace_list(request: JsonRpcRequest) -> JsonRpcResponse {
    let manager = match build_marketplace_manager() {
        Ok(m) => m,
        Err(e) => return JsonRpcResponse::error(request.id, -32000, e),
    };

    let marketplaces = manager.list();
    let result: Vec<serde_json::Value> = marketplaces
        .iter()
        .map(|(name, config)| {
            let type_str = match config.source_type {
                crate::extension::marketplace::types::MarketplaceSourceType::Local => "local",
                crate::extension::marketplace::types::MarketplaceSourceType::Github => "github",
            };
            json!({
                "name": name,
                "source": config.source,
                "type": type_str,
            })
        })
        .collect();

    JsonRpcResponse::success(request.id, json!({ "marketplaces": result }))
}

/// Add a marketplace source
pub async fn handle_marketplace_add(request: JsonRpcRequest) -> JsonRpcResponse {
    use crate::config::PluginMarketplaceEntry;

    let params: crate::gateway::handlers::plugins::types::MarketplaceAddParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Derive name from source if not provided
    let name = params.name.unwrap_or_else(|| {
        // GitHub: "owner/repo" → "repo" (lowercased)
        // Local:  "/path/to/dir" → "dir" (last component)
        params
            .source
            .split('/')
            .next_back()
            .unwrap_or(&params.source)
            .to_lowercase()
    });

    // Determine source type: if source contains '/' but no path separator at start → github
    let source_type = if params.source.starts_with('/') || params.source.starts_with('.') {
        "local".to_string()
    } else {
        "github".to_string()
    };

    let mut config = match crate::config::Config::load() {
        Ok(c) => c,
        Err(e) => return JsonRpcResponse::error(request.id, -32000, format!("Config error: {e}")),
    };

    config.plugin_marketplaces.insert(
        name.clone(),
        PluginMarketplaceEntry {
            source: params.source.clone(),
            source_type,
        },
    );

    if let Err(e) = config.save_incremental(&["plugin_marketplaces"]) {
        return JsonRpcResponse::error(request.id, -32000, format!("Failed to save config: {e}"));
    }

    JsonRpcResponse::success(
        request.id,
        json!({ "ok": true, "name": name, "source": params.source }),
    )
}

/// Remove a marketplace source
pub async fn handle_marketplace_remove(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: crate::gateway::handlers::plugins::types::MarketplaceRemoveParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let mut config = match crate::config::Config::load() {
        Ok(c) => c,
        Err(e) => return JsonRpcResponse::error(request.id, -32000, format!("Config error: {e}")),
    };

    // Build a temporary manager just to perform the remove (handles cache cleanup + builtin guard)
    let mut manager = match build_marketplace_manager() {
        Ok(m) => m,
        Err(e) => return JsonRpcResponse::error(request.id, -32000, e),
    };

    if let Err(e) = manager.remove(&params.name) {
        return JsonRpcResponse::error(request.id, -32000, e);
    }

    config.plugin_marketplaces.remove(&params.name);

    if let Err(e) = config.save_incremental(&["plugin_marketplaces"]) {
        return JsonRpcResponse::error(request.id, -32000, format!("Failed to save config: {e}"));
    }

    JsonRpcResponse::success(request.id, json!({ "ok": true, "name": params.name }))
}

/// Update marketplace index (sync cache)
pub async fn handle_marketplace_update(request: JsonRpcRequest) -> JsonRpcResponse {
    // Params are optional — empty object is fine
    let params: crate::gateway::handlers::plugins::types::MarketplaceUpdateParams =
        serde_json::from_value(request.params.clone().unwrap_or(json!({})))
            .unwrap_or(crate::gateway::handlers::plugins::types::MarketplaceUpdateParams { name: None });

    let manager = match build_marketplace_manager() {
        Ok(m) => m,
        Err(e) => return JsonRpcResponse::error(request.id, -32000, e),
    };

    let result = if let Some(name) = &params.name {
        manager.update(name).map(|_| ())
    } else {
        manager.update_all()
    };

    match result {
        Ok(()) => JsonRpcResponse::success(request.id, json!({ "ok": true })),
        Err(e) => JsonRpcResponse::error(request.id, -32000, e),
    }
}

/// Install a plugin from a marketplace by name
pub async fn handle_marketplace_install(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: crate::gateway::handlers::plugins::types::MarketplaceInstallParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Parse scope (default: user)
    let scope_str = params.scope.as_deref().unwrap_or("user");
    let scope = match crate::extension::scope::parse_scope(scope_str) {
        Ok(s) => s,
        Err(e) => return JsonRpcResponse::error(request.id, -32000, e),
    };

    let manager = match build_marketplace_manager() {
        Ok(m) => m,
        Err(e) => return JsonRpcResponse::error(request.id, -32000, e),
    };

    // project_dir is None for now; project/local scope support requires workspace detection
    match manager.install_to_scope(&params.name, params.marketplace.as_deref(), scope, None) {
        Ok(dest) => JsonRpcResponse::success(
            request.id,
            json!({
                "ok": true,
                "name": params.name,
                "scope": scope_str,
                "installed_at": dest.to_string_lossy(),
            }),
        ),
        Err(e) => JsonRpcResponse::error(request.id, -32000, e),
    }
}
