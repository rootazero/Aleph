use crate::config::types::PiiAction;
use crate::config::Config;
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::handlers::normalize_optional_string;
use crate::gateway::handlers::search_config::dto::{vault_key, SearchConfigDto};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::security::SharedTokenManager;
use crate::sync_primitives::Arc;
use serde_json::Value;
use tokio::sync::RwLock;
use tracing::error;

/// Update search configuration
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    let params = match request.params {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params".to_string())
        }
    };

    let dto: SearchConfigDto = match serde_json::from_value(params) {
        Ok(d) => d,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid params: {e}"),
            )
        }
    };

    // Validate max_results
    if dto.max_results == 0 || dto.max_results > 100 {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "max_results must be between 1 and 100".to_string(),
        );
    }

    // Validate timeout
    if dto.timeout_seconds == 0 || dto.timeout_seconds > 300 {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "timeout_seconds must be between 1 and 300".to_string(),
        );
    }

    {
        let mut cfg = config.write().await;

        // Structural prerequisites, checked BEFORE any mutation. These mirror
        // config::validate — the loader hard-fails on a searxng backend
        // without base_url (or google without engine_id), so persisting such
        // an entry would brick every subsequent config load ("brick-on-save",
        // 2026-07-02 incident). Effective provider_type follows the upsert
        // semantics below: an existing entry keeps its type, a new entry
        // derives it from the DTO name.
        {
            let empty = std::collections::HashMap::new();
            let existing = cfg.search.as_ref().map(|s| &s.backends).unwrap_or(&empty);
            for backend_dto in &dto.backends {
                let provider_type = existing
                    .get(&backend_dto.name)
                    .map(|e| e.provider_type.as_str())
                    .unwrap_or(backend_dto.name.as_str());
                match provider_type {
                    "searxng"
                        if normalize_optional_string(backend_dto.base_url.clone()).is_none() =>
                    {
                        return JsonRpcResponse::error(
                            request.id,
                            INVALID_PARAMS,
                            format!(
                                "Search backend '{}' (SearXNG) requires a base_url — to remove \
                                 the provider, delete it instead of saving it with an empty URL",
                                backend_dto.name
                            ),
                        );
                    }
                    "google"
                        if normalize_optional_string(backend_dto.engine_id.clone()).is_none() =>
                    {
                        return JsonRpcResponse::error(
                            request.id,
                            INVALID_PARAMS,
                            format!(
                                "Search backend '{}' (Google) requires an engine_id",
                                backend_dto.name
                            ),
                        );
                    }
                    _ => {}
                }
            }
        }

        // Create search config if it doesn't exist
        if cfg.search.is_none() {
            cfg.search = Some(crate::config::types::SearchConfigInternal {
                enabled: false,
                default_provider: String::new(),
                fallback_providers: None,
                max_results: 5,
                timeout_seconds: 10,
                backends: std::collections::HashMap::new(),
                web_fetch_fallback: crate::config::types::search::default_web_fetch_fallback(),
            });
        }

        if let Some(search) = &mut cfg.search {
            search.enabled = dto.enabled;
            search.default_provider = dto.default_provider.clone();
            search.max_results = dto.max_results as usize;
            search.timeout_seconds = dto.timeout_seconds;

            // Update backend configs
            for backend_dto in &dto.backends {
                // Store API key in vault if provided
                if let Some(ref api_key) = normalize_optional_string(backend_dto.api_key.clone()) {
                    if let Err(e) = vault.store_secret(&vault_key(&backend_dto.name), api_key) {
                        error!(error = %e, "Failed to store search API key in vault");
                        return JsonRpcResponse::error(
                            request.id,
                            INTERNAL_ERROR,
                            format!("Failed to store API key: {e}"),
                        );
                    }
                }

                let entry = search
                    .backends
                    .entry(backend_dto.name.clone())
                    .or_insert_with(|| crate::config::types::SearchBackendConfig {
                        provider_type: backend_dto.name.clone(),
                        api_key: None,
                        base_url: None,
                        engine_id: None,
                        engines: None,
                        min_request_interval_ms: None,
                        verified: false,
                    });

                // api_key stays None in config — vault is the source
                entry.api_key = None;
                entry.base_url = backend_dto.base_url.clone();
                entry.engine_id = backend_dto.engine_id.clone();
                entry.engines = backend_dto.engines.clone();
                entry.verified = false; // Config change resets verified
            }
        }

        // Mirror the PII toggles into the canonical `[privacy]` config — the
        // section `PiiEngine` actually consumes. Checking a category that was
        // `Off` turns it `Block`; an already-on category keeps its `Block`/`Warn`
        // distinction (the simple on/off UI never downgrades Warn→Block).
        // Unchecking sets `Off`. Other categories (api_key, ssh_key, ip_address)
        // and custom rules are left untouched.
        let apply = |current: &mut PiiAction, on: bool| {
            *current = if on {
                if *current == PiiAction::Off {
                    PiiAction::Block
                } else {
                    current.clone()
                }
            } else {
                PiiAction::Off
            };
        };
        cfg.privacy.pii_filtering = dto.pii_enabled;
        apply(&mut cfg.privacy.email, dto.pii_scrub_email);
        apply(&mut cfg.privacy.phone, dto.pii_scrub_phone);
        apply(&mut cfg.privacy.id_card, dto.pii_scrub_ssn);
        apply(&mut cfg.privacy.bank_card, dto.pii_scrub_credit_card);

        // api_key is #[serde(skip)] — never persisted to config.toml
        if let Err(e) = cfg
            .save_incremental(&["search", "privacy"])
            .map_err(|e| e.to_string())
        {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }

        // Hot-reload the global PII engine so the new policy takes effect
        // without a restart (PiiEngine::global() is what the runtime reads).
        crate::pii::PiiEngine::reload(cfg.privacy.clone());
    }

    // Broadcast config change event
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("search".to_string()),
        value: serde_json::to_value(&dto).unwrap_or(Value::Null),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_gateway_event(&event);

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}
