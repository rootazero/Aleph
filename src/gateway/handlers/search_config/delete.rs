use crate::config::Config;
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::handlers::remove_from_chain;
use crate::gateway::handlers::search_config::dto::vault_key;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::security::SharedTokenManager;
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;
use tracing::warn;

/// Delete a search backend by name
pub async fn handle_delete_backend(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    #[derive(serde::Deserialize)]
    struct Params {
        name: String,
    }

    let params: Params = match crate::gateway::handlers::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    {
        let mut cfg = config.write().await;

        let Some(search) = &mut cfg.search else {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "No search configuration found".to_string(),
            );
        };

        if !search.backends.contains_key(&params.name) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Backend '{}' not found", params.name),
            );
        }

        // Don't allow deleting the default provider
        if search.default_provider == params.name {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Cannot delete the default provider. Set a different default first.".to_string(),
            );
        }

        search.backends.remove(&params.name);

        // Scrub the deleted backend from the fallback list so it does not
        // linger as a dangling reference (mirrors AI-provider deletion). The
        // whole `[search]` section is re-serialized on save below, so the
        // change persists without naming an extra section.
        if let Some(fallbacks) = search.fallback_providers.as_mut() {
            remove_from_chain(fallbacks, &params.name);
        }

        // Delete API key from vault
        if let Err(e) = vault.delete_secret(&vault_key(&params.name)) {
            warn!(backend = %params.name, error = %e, "Failed to delete search API key from vault");
        }

        // Save config
        if let Err(e) = cfg.save_incremental(&["search"]).map_err(|e| e.to_string()) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }
    }

    // `[search]` is a declared-live section: deleting a backend must reach
    // the running registry too, or the deleted backend keeps answering (and
    // billing) until restart. Absent handle → the verdict downgrades to
    // `restart`, reported to the caller rather than claimed live.
    let impact = {
        let cfg = config.read().await;
        let landed = crate::config::live_apply::apply_live_sections(&cfg, &["search"]);
        crate::config::classify_verified("search", &landed)
    };

    // Broadcast config change event
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("search".to_string()),
        value: serde_json::json!({ "deleted_backend": params.name }),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_gateway_event(&event);

    JsonRpcResponse::success(
        request.id,
        serde_json::json!({ "success": true, "reload_impact": impact }),
    )
}
