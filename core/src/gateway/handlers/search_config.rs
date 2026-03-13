//! Search configuration RPC handlers
//!
//! Provides RPC methods for managing search settings.

use crate::config::Config;
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchBackendDto {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_id: Option<String>,
    #[serde(default)]
    pub verified: bool,
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|v| {
        let trimmed = v.trim();
        if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfigDto {
    pub enabled: bool,
    pub default_provider: String,
    pub max_results: u64,
    pub timeout_seconds: u64,
    pub pii_enabled: bool,
    pub pii_scrub_email: bool,
    pub pii_scrub_phone: bool,
    pub pii_scrub_ssn: bool,
    pub pii_scrub_credit_card: bool,
    #[serde(default)]
    pub backends: Vec<SearchBackendDto>,
}

/// Get search configuration
pub async fn handle_get(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    let cfg = config.read().await;

    if let Some(search) = &cfg.search {
        let pii = search.pii.as_ref();
        let backends: Vec<SearchBackendDto> = search
            .backends
            .iter()
            .map(|(name, backend)| SearchBackendDto {
                name: name.clone(),
                api_key: backend.api_key.clone(),
                secret_name: backend.secret_name.clone(),
                base_url: backend.base_url.clone(),
                engine_id: backend.engine_id.clone(),
                verified: backend.verified,
            })
            .collect();
        let dto = SearchConfigDto {
            enabled: search.enabled,
            default_provider: search.default_provider.clone(),
            max_results: search.max_results as u64,
            timeout_seconds: search.timeout_seconds,
            pii_enabled: pii.map(|p| p.enabled).unwrap_or(false),
            pii_scrub_email: pii.map(|p| p.scrub_email).unwrap_or(true),
            pii_scrub_phone: pii.map(|p| p.scrub_phone).unwrap_or(true),
            pii_scrub_ssn: pii.map(|p| p.scrub_ssn).unwrap_or(true),
            pii_scrub_credit_card: pii.map(|p| p.scrub_credit_card).unwrap_or(true),
            backends,
        };
        JsonRpcResponse::success(request.id, serde_json::to_value(dto).unwrap())
    } else {
        // Return default values — no provider active by default
        let dto = SearchConfigDto {
            enabled: false,
            default_provider: String::new(),
            max_results: 5,
            timeout_seconds: 10,
            pii_enabled: false,
            pii_scrub_email: true,
            pii_scrub_phone: true,
            pii_scrub_ssn: true,
            pii_scrub_credit_card: true,
            backends: Vec::new(),
        };
        JsonRpcResponse::success(request.id, serde_json::to_value(dto).unwrap())
    }
}

/// Update search configuration
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params = match request.params {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params".to_string(),
            )
        }
    };

    let dto: SearchConfigDto = match serde_json::from_value(params) {
        Ok(d) => d,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid params: {}", e),
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

        // Create search config if it doesn't exist
        if cfg.search.is_none() {
            cfg.search = Some(crate::config::types::SearchConfigInternal {
                enabled: false,
                default_provider: String::new(),
                fallback_providers: None,
                max_results: 5,
                timeout_seconds: 10,
                backends: std::collections::HashMap::new(),
                pii: Some(crate::config::types::PIIConfig::default()),
            });
        }

        if let Some(search) = &mut cfg.search {
            search.enabled = dto.enabled;
            search.default_provider = dto.default_provider.clone();
            search.max_results = dto.max_results as usize;
            search.timeout_seconds = dto.timeout_seconds;

            // Update backend configs
            for backend_dto in &dto.backends {
                let entry = search
                    .backends
                    .entry(backend_dto.name.clone())
                    .or_insert_with(|| crate::config::types::SearchBackendConfig {
                        provider_type: backend_dto.name.clone(),
                        api_key: None,
                        secret_name: None,
                        base_url: None,
                        engine_id: None,
                        verified: false,
                    });

                entry.api_key = normalize_optional_string(backend_dto.api_key.clone());
                entry.secret_name = normalize_optional_string(backend_dto.secret_name.clone());

                entry.base_url = backend_dto.base_url.clone();
                entry.engine_id = backend_dto.engine_id.clone();
                entry.verified = false; // Config change resets verified
            }

            // Update PII config
            if search.pii.is_none() {
                search.pii = Some(crate::config::types::PIIConfig::default());
            }
            if let Some(pii) = &mut search.pii {
                pii.enabled = dto.pii_enabled;
                pii.scrub_email = dto.pii_scrub_email;
                pii.scrub_phone = dto.pii_scrub_phone;
                pii.scrub_ssn = dto.pii_scrub_ssn;
                pii.scrub_credit_card = dto.pii_scrub_credit_card;
            }
        }

        // Redact vault-backed api_keys before saving
        let mut sanitized = cfg.clone();
        if let Some(search) = &mut sanitized.search {
            for backend in search.backends.values_mut() {
                if backend.secret_name.is_some() {
                    backend.api_key = None;
                }
            }
        }
        if let Err(e) = sanitized.save().map_err(|e| e.to_string()) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {}", e),
            );
        }
    }

    // Broadcast config change event
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("search".to_string()),
        value: serde_json::to_value(&dto).unwrap_or(Value::Null),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_json(&event);

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}

// ============================================================================
// Test Connection
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchTestResult {
    pub success: bool,
    pub message: String,
}

/// Test a search backend connection
pub async fn handle_test(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        /// Backend name (used to persist verified=true on success)
        name: String,
        #[serde(default)]
        api_key: Option<String>,
        #[serde(default)]
        base_url: Option<String>,
        #[serde(default)]
        engine_id: Option<String>,
    }

    let params: Params = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Determine provider type from config or fallback to name
    let provider_type = {
        let cfg = config.read().await;
        cfg.search
            .as_ref()
            .and_then(|s| s.backends.get(&params.name))
            .map(|b| b.provider_type.clone())
            .unwrap_or_else(|| params.name.clone())
    };

    // Create a temporary search provider and test it
    use crate::search::providers::*;
    use crate::search::{SearchOptions, SearchProvider};

    let test_result: SearchTestResult = match provider_type.as_str() {
        "tavily" => {
            let Some(ref api_key) = params.api_key else {
                return JsonRpcResponse::success(
                    request.id,
                    serde_json::to_value(SearchTestResult {
                        success: false,
                        message: "API key is required for Tavily".to_string(),
                    }).unwrap(),
                );
            };
            match TavilyProvider::new(api_key.clone()) {
                Ok(provider) => {
                    let opts = SearchOptions { max_results: 1, ..Default::default() };
                    match provider.search("test", &opts).await {
                        Ok(_) => SearchTestResult { success: true, message: "Connection successful".to_string() },
                        Err(e) => SearchTestResult { success: false, message: format!("Search failed: {}", e) },
                    }
                }
                Err(e) => SearchTestResult { success: false, message: format!("Failed to create provider: {}", e) },
            }
        }
        "brave" => {
            let Some(ref api_key) = params.api_key else {
                return JsonRpcResponse::success(
                    request.id,
                    serde_json::to_value(SearchTestResult {
                        success: false,
                        message: "API key is required for Brave".to_string(),
                    }).unwrap(),
                );
            };
            match BraveProvider::new(api_key.clone()) {
                Ok(provider) => {
                    let opts = SearchOptions { max_results: 1, ..Default::default() };
                    match provider.search("test", &opts).await {
                        Ok(_) => SearchTestResult { success: true, message: "Connection successful".to_string() },
                        Err(e) => SearchTestResult { success: false, message: format!("Search failed: {}", e) },
                    }
                }
                Err(e) => SearchTestResult { success: false, message: format!("Failed to create provider: {}", e) },
            }
        }
        "searxng" => {
            let base_url = params.base_url.unwrap_or_else(|| "http://localhost:8888".to_string());
            match SearxngProvider::new(base_url) {
                Ok(provider) => {
                    let opts = SearchOptions { max_results: 1, ..Default::default() };
                    match provider.search("test", &opts).await {
                        Ok(_) => SearchTestResult { success: true, message: "Connection successful".to_string() },
                        Err(e) => SearchTestResult { success: false, message: format!("Search failed: {}", e) },
                    }
                }
                Err(e) => SearchTestResult { success: false, message: format!("Failed to create provider: {}", e) },
            }
        }
        "google" => {
            let Some(ref api_key) = params.api_key else {
                return JsonRpcResponse::success(
                    request.id,
                    serde_json::to_value(SearchTestResult {
                        success: false,
                        message: "API key is required for Google".to_string(),
                    }).unwrap(),
                );
            };
            let Some(ref engine_id) = params.engine_id else {
                return JsonRpcResponse::success(
                    request.id,
                    serde_json::to_value(SearchTestResult {
                        success: false,
                        message: "Engine ID (cx) is required for Google CSE".to_string(),
                    }).unwrap(),
                );
            };
            match GoogleProvider::new(api_key.clone(), engine_id.clone()) {
                Ok(provider) => {
                    let opts = SearchOptions { max_results: 1, ..Default::default() };
                    match provider.search("test", &opts).await {
                        Ok(_) => SearchTestResult { success: true, message: "Connection successful".to_string() },
                        Err(e) => SearchTestResult { success: false, message: format!("Search failed: {}", e) },
                    }
                }
                Err(e) => SearchTestResult { success: false, message: format!("Failed to create provider: {}", e) },
            }
        }
        "bing" => {
            let Some(ref api_key) = params.api_key else {
                return JsonRpcResponse::success(
                    request.id,
                    serde_json::to_value(SearchTestResult {
                        success: false,
                        message: "API key is required for Bing".to_string(),
                    }).unwrap(),
                );
            };
            match BingProvider::new(api_key.clone()) {
                Ok(provider) => {
                    let opts = SearchOptions { max_results: 1, ..Default::default() };
                    match provider.search("test", &opts).await {
                        Ok(_) => SearchTestResult { success: true, message: "Connection successful".to_string() },
                        Err(e) => SearchTestResult { success: false, message: format!("Search failed: {}", e) },
                    }
                }
                Err(e) => SearchTestResult { success: false, message: format!("Failed to create provider: {}", e) },
            }
        }
        "exa" => {
            let Some(ref api_key) = params.api_key else {
                return JsonRpcResponse::success(
                    request.id,
                    serde_json::to_value(SearchTestResult {
                        success: false,
                        message: "API key is required for Exa".to_string(),
                    }).unwrap(),
                );
            };
            match ExaProvider::new(api_key.clone()) {
                Ok(provider) => {
                    let opts = SearchOptions { max_results: 1, ..Default::default() };
                    match provider.search("test", &opts).await {
                        Ok(_) => SearchTestResult { success: true, message: "Connection successful".to_string() },
                        Err(e) => SearchTestResult { success: false, message: format!("Search failed: {}", e) },
                    }
                }
                Err(e) => SearchTestResult { success: false, message: format!("Failed to create provider: {}", e) },
            }
        }
        _ => {
            SearchTestResult {
                success: false,
                message: format!("Unknown provider type: {}", provider_type),
            }
        }
    };

    // Persist verified=true on success
    if test_result.success {
        let mut cfg = config.write().await;
        if let Some(search) = &mut cfg.search {
            if let Some(backend) = search.backends.get_mut(&params.name) {
                backend.verified = true;
                if let Err(e) = cfg.save() {
                    tracing::error!(error = %e, "Failed to save config after search test");
                }
            }
        }
    }

    JsonRpcResponse::success(request.id, serde_json::to_value(test_result).unwrap())
}

// ============================================================================
// Delete Backend
// ============================================================================

/// Delete a search backend by name
pub async fn handle_delete_backend(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        name: String,
    }

    let params: Params = match super::parse_params(&request) {
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

        // Save config
        let mut sanitized = cfg.clone();
        if let Some(search) = &mut sanitized.search {
            for backend in search.backends.values_mut() {
                if backend.secret_name.is_some() {
                    backend.api_key = None;
                }
            }
        }
        if let Err(e) = sanitized.save().map_err(|e| e.to_string()) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {}", e),
            );
        }
    }

    // Broadcast config change event
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("search".to_string()),
        value: serde_json::json!({ "deleted_backend": params.name }),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_json(&event);

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}
