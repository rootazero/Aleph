//! Search configuration RPC handlers
//!
//! Provides RPC methods for managing search settings.

use crate::config::Config;
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;

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
}

/// Get search configuration
pub async fn handle_get(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    let cfg = config.read().await;

    if let Some(search) = &cfg.search {
        let pii = search.pii.as_ref();
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
        };
        JsonRpcResponse::success(request.id, serde_json::to_value(dto).unwrap())
    } else {
        // Return default values
        let dto = SearchConfigDto {
            enabled: false,
            default_provider: "tavily".to_string(),
            max_results: 5,
            timeout_seconds: 10,
            pii_enabled: false,
            pii_scrub_email: true,
            pii_scrub_phone: true,
            pii_scrub_ssn: true,
            pii_scrub_credit_card: true,
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

        if let Some(search) = &mut cfg.search {
            search.enabled = dto.enabled;
            search.default_provider = dto.default_provider.clone();
            search.max_results = dto.max_results as usize;
            search.timeout_seconds = dto.timeout_seconds;

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

        if let Err(e) = cfg.save() {
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
