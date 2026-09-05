use crate::config::types::PiiAction;
use crate::config::Config;
use crate::gateway::handlers::search_config::dto::{
    resolve_api_key, SearchBackendDto, SearchConfigDto,
};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use crate::gateway::security::SharedTokenManager;
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

/// Get search configuration
pub async fn handle_get(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    let cfg = config.read().await;

    // PII toggles are sourced from the canonical `[privacy]` config — the same
    // section consumed by `PiiEngine` at runtime. A category is "on" in the
    // simple Panel UI whenever its action is not `Off` (Block or Warn).
    let pii_enabled = cfg.privacy.pii_filtering;
    let pii_scrub_email = cfg.privacy.email != PiiAction::Off;
    let pii_scrub_phone = cfg.privacy.phone != PiiAction::Off;
    let pii_scrub_ssn = cfg.privacy.id_card != PiiAction::Off;
    let pii_scrub_credit_card = cfg.privacy.bank_card != PiiAction::Off;

    if let Some(search) = &cfg.search {
        let backends: Vec<SearchBackendDto> = search
            .backends
            .iter()
            .map(|(name, backend)| {
                let has_api_key = resolve_api_key(name, &vault).is_some();
                tracing::info!(
                    backend = %name,
                    has_key = has_api_key,
                    "search_config.get: resolved API key presence"
                );
                // Security (3def857c6): report presence only, never echo the secret.
                SearchBackendDto {
                    name: name.clone(),
                    api_key: None,
                    base_url: backend.base_url.clone(),
                    engine_id: backend.engine_id.clone(),
                    engines: backend.engines.clone(),
                    has_api_key,
                    verified: backend.verified,
                    allow_private_upstream: backend.allow_private_upstream,
                }
            })
            .collect();
        let dto = SearchConfigDto {
            enabled: search.enabled,
            default_provider: search.default_provider.clone(),
            max_results: search.max_results as u64,
            timeout_seconds: search.timeout_seconds,
            pii_enabled,
            pii_scrub_email,
            pii_scrub_phone,
            pii_scrub_ssn,
            pii_scrub_credit_card,
            backends,
        };
        match serde_json::to_value(dto) {
            Ok(v) => JsonRpcResponse::success(request.id, v),
            Err(e) => JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to serialize config: {e}"),
            ),
        }
    } else {
        // No search provider active by default — but PII toggles still reflect
        // the canonical `[privacy]` config so the page is truthful.
        let dto = SearchConfigDto {
            enabled: false,
            default_provider: String::new(),
            max_results: 5,
            timeout_seconds: 10,
            pii_enabled,
            pii_scrub_email,
            pii_scrub_phone,
            pii_scrub_ssn,
            pii_scrub_credit_card,
            backends: Vec::new(),
        };
        match serde_json::to_value(dto) {
            Ok(v) => JsonRpcResponse::success(request.id, v),
            Err(e) => JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to serialize config: {e}"),
            ),
        }
    }
}
