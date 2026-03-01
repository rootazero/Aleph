//! Routing Rules RPC Handlers
//!
//! Handlers for routing rule management: list, create, update, delete, move.

use serde::{Deserialize, Serialize};
use serde_json::json;
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;
use tracing::{error, info};

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::parse_params;
use super::super::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::config::{Config, RoutingRuleConfig};

// ============================================================================
// List
// ============================================================================

/// Routing rule info for JSON serialization
#[derive(Debug, Clone, Serialize)]
pub struct RoutingRuleInfo {
    pub index: usize,
    pub rule_type: String,
    pub regex: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    pub is_builtin: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_model: Option<String>,
}

/// List all routing rules
pub async fn handle_list(request: JsonRpcRequest, config: Arc<RwLock<Config>>) -> JsonRpcResponse {
    let config = config.read().await;

    let rules: Vec<RoutingRuleInfo> = config
        .rules
        .iter()
        .enumerate()
        .map(|(index, rule)| RoutingRuleInfo {
            index,
            rule_type: rule.get_rule_type().to_string(),
            regex: rule.regex.clone(),
            provider: rule.provider.clone(),
            system_prompt: rule.system_prompt.clone(),
            is_builtin: rule.is_builtin,
            intent_type: rule.intent_type.clone(),
            preferred_model: rule.preferred_model.clone(),
        })
        .collect();

    JsonRpcResponse::success(request.id, json!({ "rules": rules }))
}

// ============================================================================
// Get
// ============================================================================

/// Parameters for routing_rules.get
#[derive(Debug, Deserialize)]
pub struct GetParams {
    pub index: usize,
}

/// Get a single routing rule
pub async fn handle_get(request: JsonRpcRequest, config: Arc<RwLock<Config>>) -> JsonRpcResponse {
    let params: GetParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let config = config.read().await;
    match config.get_rule(params.index) {
        Some(rule) => {
            let info = RoutingRuleInfo {
                index: params.index,
                rule_type: rule.get_rule_type().to_string(),
                regex: rule.regex.clone(),
                provider: rule.provider.clone(),
                system_prompt: rule.system_prompt.clone(),
                is_builtin: rule.is_builtin,
                intent_type: rule.intent_type.clone(),
                preferred_model: rule.preferred_model.clone(),
            };
            JsonRpcResponse::success(request.id, json!({ "rule": info }))
        }
        None => JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Rule not found at index: {}", params.index),
        ),
    }
}

// ============================================================================
// Create
// ============================================================================

/// Parameters for routing_rules.create
#[derive(Debug, Deserialize)]
pub struct CreateParams {
    pub rule: RoutingRuleConfigJson,
}

/// Routing rule config from JSON
#[derive(Debug, Clone, Deserialize)]
pub struct RoutingRuleConfigJson {
    #[serde(default)]
    pub rule_type: Option<String>,
    pub regex: String,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub strip_prefix: Option<bool>,
    #[serde(default)]
    pub capabilities: Option<Vec<String>>,
    #[serde(default)]
    pub intent_type: Option<String>,
    #[serde(default)]
    pub preferred_model: Option<String>,
    #[serde(default)]
    pub context_format: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
}

/// Create a new routing rule
pub async fn handle_create(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: CreateParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Convert JSON config to RoutingRuleConfig
    let rule_config = RoutingRuleConfig {
        rule_type: params.rule.rule_type,
        is_builtin: false,
        regex: params.rule.regex.clone(),
        provider: params.rule.provider,
        system_prompt: params.rule.system_prompt,
        strip_prefix: params.rule.strip_prefix,
        capabilities: params.rule.capabilities,
        intent_type: params.rule.intent_type,
        preferred_model: params.rule.preferred_model,
        context_format: params.rule.context_format,
        icon: params.rule.icon,
    };

    // Add rule
    {
        let mut cfg = config.write().await;
        cfg.add_rule_at_top(rule_config);

        // Save to file
        if let Err(e) = cfg.save() {
            error!(error = %e, "Failed to save config");
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {}", e),
            );
        }
    }

    // Broadcast event
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("routing_rules".to_string()),
        value: json!({ "action": "created", "regex": params.rule.regex }),
        timestamp,
    });

    if let Err(e) = event_bus.publish_json(&event) {
        error!(error = %e, "Failed to broadcast event");
    }

    info!(regex = %params.rule.regex, "Routing rule created");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

// ============================================================================
// Update
// ============================================================================

/// Parameters for routing_rules.update
#[derive(Debug, Deserialize)]
pub struct UpdateParams {
    pub index: usize,
    pub rule: RoutingRuleConfigJson,
}

/// Update a routing rule
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: UpdateParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Update rule
    {
        let mut cfg = config.write().await;

        // Check if rule exists
        if params.index >= cfg.rule_count() {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Rule not found at index: {}", params.index),
            );
        }

        // Convert JSON config to RoutingRuleConfig
        let rule_config = RoutingRuleConfig {
            rule_type: params.rule.rule_type,
            is_builtin: false,
            regex: params.rule.regex.clone(),
            provider: params.rule.provider,
            system_prompt: params.rule.system_prompt,
            strip_prefix: params.rule.strip_prefix,
            capabilities: params.rule.capabilities,
            intent_type: params.rule.intent_type,
            preferred_model: params.rule.preferred_model,
            context_format: params.rule.context_format,
            icon: params.rule.icon,
        };

        // Replace the rule
        cfg.rules[params.index] = rule_config;

        // Save to file
        if let Err(e) = cfg.save() {
            error!(error = %e, "Failed to save config");
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {}", e),
            );
        }
    }

    // Broadcast event
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("routing_rules".to_string()),
        value: json!({ "action": "updated", "index": params.index }),
        timestamp,
    });

    if let Err(e) = event_bus.publish_json(&event) {
        error!(error = %e, "Failed to broadcast event");
    }

    info!(index = %params.index, "Routing rule updated");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

// ============================================================================
// Delete
// ============================================================================

/// Parameters for routing_rules.delete
#[derive(Debug, Deserialize)]
pub struct DeleteParams {
    pub index: usize,
}

/// Delete a routing rule
pub async fn handle_delete(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: DeleteParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Delete rule
    {
        let mut cfg = config.write().await;

        // Check if rule exists
        if params.index >= cfg.rule_count() {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Rule not found at index: {}", params.index),
            );
        }

        // Check if it's a builtin rule
        if let Some(rule) = cfg.get_rule(params.index) {
            if rule.is_builtin {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    "Cannot delete builtin rule".to_string(),
                );
            }
        }

        // Remove rule
        if let Err(e) = cfg.remove_rule(params.index) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to remove rule: {}", e),
            );
        }

        // Save to file
        if let Err(e) = cfg.save() {
            error!(error = %e, "Failed to save config");
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {}", e),
            );
        }
    }

    // Broadcast event
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("routing_rules".to_string()),
        value: json!({ "action": "deleted", "index": params.index }),
        timestamp,
    });

    if let Err(e) = event_bus.publish_json(&event) {
        error!(error = %e, "Failed to broadcast event");
    }

    info!(index = %params.index, "Routing rule deleted");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

// ============================================================================
// Move
// ============================================================================

/// Parameters for routing_rules.move
#[derive(Debug, Deserialize)]
pub struct MoveParams {
    pub from: usize,
    pub to: usize,
}

/// Move a routing rule
pub async fn handle_move(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: MoveParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Move rule
    {
        let mut cfg = config.write().await;

        // Move rule
        if let Err(e) = cfg.move_rule(params.from, params.to) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Failed to move rule: {}", e),
            );
        }

        // Save to file
        if let Err(e) = cfg.save() {
            error!(error = %e, "Failed to save config");
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {}", e),
            );
        }
    }

    // Broadcast event
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("routing_rules".to_string()),
        value: json!({ "action": "moved", "from": params.from, "to": params.to }),
        timestamp,
    });

    if let Err(e) = event_bus.publish_json(&event) {
        error!(error = %e, "Failed to broadcast event");
    }

    info!(from = %params.from, to = %params.to, "Routing rule moved");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}
