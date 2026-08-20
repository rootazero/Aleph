use crate::gateway::handlers::parse_params;
use crate::gateway::handlers::plugins::handlers::build_marketplace_manager;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};
use serde_json::json;

/// List all registered marketplaces (including built-in).
///
/// Built from [`MarketplaceListResult`] rather than a `json!` literal: serde
/// ignores unknown keys on the way in, so a test that only *parses* a real
/// response is structurally blind to whatever else is on the wire. Constructing
/// from the contract type makes over-sending a compile error instead of an
/// assertion somebody has to remember to write.
///
/// Rows are sorted by name because `manager.list()` hands back a `HashMap`, and
/// an unsorted response reshuffles the Panel's list on every load.
pub async fn handle_marketplace_list(request: JsonRpcRequest) -> JsonRpcResponse {
    use crate::gateway::handlers::plugins::types::marketplace_registration_row;
    use aleph_protocol::plugins::{MarketplaceListResult, MarketplaceRow};

    let manager = match build_marketplace_manager() {
        Ok(m) => m,
        Err(e) => return JsonRpcResponse::error(request.id, -32000, e),
    };

    let mut rows: Vec<MarketplaceRow> = manager
        .list()
        .iter()
        .map(|(name, config)| marketplace_registration_row(name, config))
        .collect();
    rows.sort_by(|a, b| a.name.cmp(&b.name));

    match serde_json::to_value(MarketplaceListResult { marketplaces: rows }) {
        Ok(v) => JsonRpcResponse::success(request.id, v),
        Err(e) => JsonRpcResponse::error(request.id, -32000, format!("Serialisation error: {e}")),
    }
}

/// List the plugins a marketplace contains.
///
/// The sibling above lists **registrations** — name, source, type. Nothing
/// listed a marketplace's *contents*, and `search_plugin` only ever matched an
/// exact id, so the only way to install by name was to already know the name.
/// Panel's install dialog and `aleph plugin marketplace browse` are both this
/// call.
///
/// Not auto-syncing is deliberate: browsing is a local read of an already
/// fetched cache, and a browse that silently git-pulls turns opening a dialog
/// into network I/O. A marketplace with no cache comes back in `problems`
/// saying which command fetches it.
pub async fn handle_marketplace_browse(request: JsonRpcRequest) -> JsonRpcResponse {
    use crate::gateway::handlers::plugins::types::marketplace_row;
    use aleph_protocol::plugins::{MarketplaceBrowseResult, MarketplaceProblemRow};

    // Every field is optional, so an omitted `params` is a valid "browse
    // everything" rather than an error.
    let params: crate::gateway::handlers::plugins::types::MarketplaceBrowseParams =
        serde_json::from_value(request.params.clone().unwrap_or(json!({}))).unwrap_or_default();

    let manager = match build_marketplace_manager() {
        Ok(m) => m,
        Err(e) => return JsonRpcResponse::error(request.id, -32000, e),
    };

    let listing = manager.browse(params.marketplace.as_deref(), params.query.as_deref());

    // Built from the contract type rather than a `json!` literal: serde ignores
    // unknown keys on the way in, so a test that only parses a real response is
    // structurally blind to whatever else is on the wire.
    let result = MarketplaceBrowseResult {
        plugins: listing.entries.iter().map(marketplace_row).collect(),
        problems: listing
            .problems
            .into_iter()
            .map(|p| MarketplaceProblemRow {
                marketplace: p.marketplace,
                reason: p.reason,
            })
            .collect(),
    };

    match serde_json::to_value(result) {
        Ok(v) => JsonRpcResponse::success(request.id, v),
        Err(e) => JsonRpcResponse::error(request.id, -32000, format!("Serialisation error: {e}")),
    }
}

/// Add a marketplace source.
///
/// Name and type both come from
/// [`marketplace::classify`](crate::extension::marketplace::classify) rather
/// than from a heuristic written here. This handler used to carry one and
/// `aleph-server plugin marketplace add` carried another, and the two
/// disagreed about Windows paths, relative paths, bare words and name casing
/// — see that module for the table.
///
/// Registering does not fetch. The shipped subcommand syncs immediately after
/// adding, and both the Panel and `plugin_manage` compose the two documented
/// calls to land in the same place; `add` keeps one meaning for every client.
pub async fn handle_marketplace_add(request: JsonRpcRequest) -> JsonRpcResponse {
    use crate::config::PluginMarketplaceEntry;
    use crate::extension::marketplace::{classify, MarketplaceConfig};
    use crate::gateway::protocol::INVALID_PARAMS;

    let params: crate::gateway::handlers::plugins::types::MarketplaceAddParams =
        match parse_params(&request) {
            Ok(p) => p,
            Err(e) => return e,
        };

    // A source that cannot yield a usable registration is refused here rather
    // than written to config.toml to fail at sync time: the old handler stored
    // anything at all, so `C:\dir\mk` became a GitHub entry named
    // `c:\dir\mk` that no fetch could ever resolve.
    let spec = match classify(&params.source, params.name.as_deref()) {
        Ok(s) => s,
        Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, e),
    };

    let mut config = match crate::config::Config::load() {
        Ok(c) => c,
        Err(e) => return JsonRpcResponse::error(request.id, -32000, format!("Config error: {e}")),
    };

    config.plugin_marketplaces.insert(
        spec.name.clone(),
        PluginMarketplaceEntry::from(&MarketplaceConfig {
            source: spec.source.clone(),
            source_type: spec.source_type,
        }),
    );

    if let Err(e) = config.save_incremental(&["plugin_marketplaces"]) {
        return JsonRpcResponse::error(request.id, -32000, format!("Failed to save config: {e}"));
    }

    JsonRpcResponse::success(
        request.id,
        json!({ "ok": true, "name": spec.name, "source": spec.source }),
    )
}

/// Remove a marketplace source
pub async fn handle_marketplace_remove(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: crate::gateway::handlers::plugins::types::MarketplaceRemoveParams =
        match parse_params(&request) {
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
        serde_json::from_value(request.params.clone().unwrap_or(json!({}))).unwrap_or(
            crate::gateway::handlers::plugins::types::MarketplaceUpdateParams { name: None },
        );

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
    let params: crate::gateway::handlers::plugins::types::MarketplaceInstallParams =
        match parse_params(&request) {
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
        Ok(dest) => {
            // Refresh the live extension set, exactly as the git-clone
            // installer and `plugin.update` do.
            //
            // Installing has two routes — `plugins.install` (a git URL) and
            // this one — and only the other one refreshed. `plugin.install`
            // sends a bare name here, so the plugin landed on disk and stayed
            // invisible to `plugins.list` until the daemon was restarted: an
            // install that reports success and changes nothing an operator can
            // see. `handle_update`'s own comment says its reload "matches the
            // install handler's behaviour", which was true of one of the two.
            if let Ok(mgr) = crate::gateway::handlers::plugins::handlers::get_extension_manager() {
                if let Err(e) = mgr.reload().await {
                    tracing::warn!("Failed to refresh extensions after install: {}", e);
                }
            }
            JsonRpcResponse::success(
                request.id,
                json!({
                    "ok": true,
                    "name": params.name,
                    "scope": scope_str,
                    "installed_at": dest.to_string_lossy(),
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(request.id, -32000, e),
    }
}

/// Update an installed plugin to the latest marketplace version (in place).
pub async fn handle_update(request: JsonRpcRequest) -> JsonRpcResponse {
    use crate::extension::marketplace::UpdateOutcome;

    let params: crate::gateway::handlers::plugins::types::UpdatePluginParams =
        match parse_params(&request) {
            Ok(p) => p,
            Err(e) => return e,
        };

    let scope_str = params.scope.as_deref().unwrap_or("user");
    let scope = match crate::extension::scope::parse_scope(scope_str) {
        Ok(s) => s,
        Err(e) => return JsonRpcResponse::error(request.id, -32000, e),
    };

    let manager = match build_marketplace_manager() {
        Ok(m) => m,
        Err(e) => return JsonRpcResponse::error(request.id, -32000, e),
    };

    let outcome = match manager.update_to_scope(
        &params.name,
        params.marketplace.as_deref(),
        scope,
        None,
        params.force,
    ) {
        Ok(o) => o,
        Err(e) => return JsonRpcResponse::error(request.id, -32000, e),
    };

    // Refresh the live extension set so an updated plugin's new capabilities take
    // effect without a daemon restart (matches the install handler's behaviour).
    if matches!(outcome, UpdateOutcome::Updated { .. }) {
        if let Ok(mgr) = crate::gateway::handlers::plugins::handlers::get_extension_manager() {
            if let Err(e) = mgr.reload().await {
                tracing::warn!("Failed to refresh extensions after update: {}", e);
            }
        }
    }

    let result = match outcome {
        UpdateOutcome::Updated { from, to } => json!({
            "ok": true,
            "name": params.name,
            "scope": scope_str,
            "updated": true,
            "from": from,
            "to": to,
        }),
        UpdateOutcome::AlreadyLatest { version } => json!({
            "ok": true,
            "name": params.name,
            "scope": scope_str,
            "updated": false,
            "version": version,
        }),
    };
    JsonRpcResponse::success(request.id, result)
}
