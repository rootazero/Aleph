use crate::gateway::handlers::parse_params;
use crate::gateway::handlers::plugins::handlers::build_marketplace_manager;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};
use serde_json::json;

/// List all registered marketplaces (including built-in).
///
/// Built from `MarketplaceListResult` rather than a `json!` literal: serde
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
    use crate::extension::marketplace::{MarketplaceConfig, classify};
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
    let scope = match crate::extension::visibility::parse_scope(scope_str) {
        Ok(s) => s,
        Err(e) => return JsonRpcResponse::error(request.id, -32000, e),
    };

    let extension = match super::get_extension_manager() {
        Ok(manager) => manager,
        Err(response) => return response.with_id(request.id),
    };
    let marketplace = match build_marketplace_manager() {
        Ok(manager) => crate::sync_primitives::Arc::new(manager),
        Err(error) => return JsonRpcResponse::error(request.id, -32000, error),
    };
    let access = crate::gateway::handlers::plugins::current_plugin_access();
    match extension
        .install_marketplace_plugin(
            marketplace,
            params.name.clone(),
            params.marketplace,
            scope,
            &access,
        )
        .await
    {
        Ok((installed, instance)) => JsonRpcResponse::success(
            request.id,
            json!({
                "ok": true,
                "name": params.name,
                "scope": scope_str,
                "installed_at": installed.path.to_string_lossy(),
                "warnings": installed.warnings,
                "selector": instance.selector(),
            }),
        ),
        Err(error) => JsonRpcResponse::error(request.id, -32000, error),
    }
}

/// Update an installed plugin to the latest marketplace version (in place).
pub async fn handle_update(request: JsonRpcRequest) -> JsonRpcResponse {
    use crate::extension::marketplace::{MarketplaceUpdate, UpdateOutcome};
    use crate::extension::plugin_target::PluginTarget;
    use crate::gateway::handlers::plugins::types::UpdatePluginParams;

    let params: UpdatePluginParams = match parse_params(&request) {
        Ok(params) => params,
        Err(response) => return response,
    };
    let scope = match params
        .scope
        .as_deref()
        .map(crate::extension::visibility::parse_scope)
        .transpose()
    {
        Ok(scope) => scope,
        Err(error) => return JsonRpcResponse::error(request.id, -32000, error),
    };
    let extension = match super::get_extension_manager() {
        Ok(manager) => manager,
        Err(response) => return response.with_id(request.id),
    };
    let marketplace = match build_marketplace_manager() {
        Ok(manager) => manager,
        Err(error) => return JsonRpcResponse::error(request.id, -32000, error),
    };
    let target = PluginTarget::from_fields(params.name, params.selector);
    let access = crate::gateway::handlers::plugins::current_plugin_access();
    let update = MarketplaceUpdate {
        scope,
        marketplace: params.marketplace,
        force: params.force,
    };
    let (instance, outcome) = match extension
        .update_marketplace_plugin(marketplace, &target, &access, update)
        .await
    {
        Ok((instance, Ok(outcome))) => (instance, outcome),
        Ok((_, Err(error))) => return JsonRpcResponse::error(request.id, -32000, error),
        Err(error) => return super::plugin_target_error_response(request.id, error),
    };
    let (outcome, runtime_error) = outcome;
    let scope = params
        .scope
        .as_deref()
        .unwrap_or_else(|| match &instance.scope {
            crate::extension::visibility::ScopeKey::Global => "user",
            crate::extension::visibility::ScopeKey::Project(_) => "project",
        });
    let result = match outcome {
        UpdateOutcome::Updated { from, to, warnings } => json!({
            "ok": true,
            "name": instance.plugin_id,
            "scope": scope,
            "selector": instance.selector(),
            "updated": true,
            "from": from,
            "to": to,
            "warnings": warnings,
            "runtimeError": runtime_error,
        }),
        UpdateOutcome::AlreadyLatest { version } => json!({
            "ok": true,
            "name": instance.plugin_id,
            "scope": scope,
            "selector": instance.selector(),
            "updated": false,
            "version": version,
        }),
    };
    JsonRpcResponse::success(request.id, result)
}
