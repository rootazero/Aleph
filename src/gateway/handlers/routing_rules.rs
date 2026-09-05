//! Routing Rules RPC Handlers
//!
//! Handlers for routing rule management: list, create, update, delete, move.

use crate::sync_primitives::Arc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::RwLock;
use tracing::{error, info};

use super::super::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::parse_params;
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

/// Parameters for `routing_rules.get`
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

/// Parameters for `routing_rules.create`
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
    pub intent_type: Option<String>,
    #[serde(default)]
    pub preferred_model: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
}

/// Refuse a rule that would be stored but never registered.
///
/// Keyword rules are retired (see `config::types::routing`'s module doc). The
/// load path is fail-open so existing files keep booting; this path is
/// fail-closed, because accepting one here would write a fresh rule into the
/// operator's TOML that silently does nothing — a button that reports success
/// and has no effect is worse than the retirement it survives.
///
/// The regex predicate is `RoutingRuleConfig::is_registered_command`, the same
/// one `register_custom_commands` skips on, so "refused here" and "registered
/// there" cannot drift apart. The message names the fix, not just the verdict.
///
/// There are **two ways to name the retired concept**, so there are two
/// refusals. Rejecting only the regex would still let a client store
/// `rule_type = "keyword"` on a `^/` rule: that rule *is* registered, but
/// `routing_rules.list` reports `get_rule_type()` verbatim, so the Panel would
/// label a working command "KEYWORD". A wrong label costs more than a missing
/// one — it reads as a fact.
///
/// The label check is deliberately **only here**, not in `Config::validate`:
/// on-disk files carrying that label boot today and must keep booting.
fn reject_unregistrable(rule: &RoutingRuleConfig) -> Option<String> {
    if let Some(declared) = rule.rule_type.as_deref() {
        if !declared.eq_ignore_ascii_case("command") {
            return Some(format!(
                "Unknown rule_type '{declared}'. Keyword rules are retired and reach no code; \
                 'command' is the only kind. Omit rule_type and it is derived from the regex."
            ));
        }
    }
    if rule.is_registered_command() {
        return None;
    }
    Some(format!(
        "Keyword routing rules are retired and reach no code: regex '{}' does not start with \
         '^/', so this rule would never be registered. Use a '^/'-prefixed regex to define a \
         slash command.",
        rule.regex
    ))
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
        intent_type: params.rule.intent_type,
        preferred_model: params.rule.preferred_model,
        icon: params.rule.icon,
    };

    if let Some(reason) = reject_unregistrable(&rule_config) {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, reason);
    }

    // Add rule
    {
        let mut cfg = config.write().await;
        cfg.add_rule_at_top(rule_config);

        // Save to file
        if let Err(e) = cfg.save_incremental(&["rules"]) {
            error!(error = %e, "Failed to save config");
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
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

    if let Err(e) = event_bus.publish_gateway_event(&event) {
        error!(error = %e, "Failed to broadcast event");
    }

    info!(regex = %params.rule.regex, "Routing rule created");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

// ============================================================================
// Update
// ============================================================================

/// Parameters for `routing_rules.update`
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
            intent_type: params.rule.intent_type,
            preferred_model: params.rule.preferred_model,
            icon: params.rule.icon,
        };

        // Same gate as `create`. Without it there is a two-step path that is
        // legal at every step and equivalent in effect: create `^/x`, then
        // update it to a keyword regex.
        if let Some(reason) = reject_unregistrable(&rule_config) {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, reason);
        }

        // Replace the rule
        cfg.rules[params.index] = rule_config;

        // Save to file
        if let Err(e) = cfg.save_incremental(&["rules"]) {
            error!(error = %e, "Failed to save config");
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
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

    if let Err(e) = event_bus.publish_gateway_event(&event) {
        error!(error = %e, "Failed to broadcast event");
    }

    info!(index = %params.index, "Routing rule updated");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

// ============================================================================
// Delete
// ============================================================================

/// Parameters for `routing_rules.delete`
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
                format!("Failed to remove rule: {e}"),
            );
        }

        // Save to file
        if let Err(e) = cfg.save_incremental(&["rules"]) {
            error!(error = %e, "Failed to save config");
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
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

    if let Err(e) = event_bus.publish_gateway_event(&event) {
        error!(error = %e, "Failed to broadcast event");
    }

    info!(index = %params.index, "Routing rule deleted");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

// ============================================================================
// Move
// ============================================================================

/// Parameters for `routing_rules.move`
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
                format!("Failed to move rule: {e}"),
            );
        }

        // Save to file
        if let Err(e) = cfg.save_incremental(&["rules"]) {
            error!(error = %e, "Failed to save config");
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
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

    if let Err(e) = event_bus.publish_gateway_event(&event) {
        error!(error = %e, "Failed to broadcast event");
    }

    info!(from = %params.from, to = %params.to, "Routing rule moved");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::paths::IsolatedAlephHome;
    use serde_json::Value;

    fn request(method: &str, params: Value) -> JsonRpcRequest {
        JsonRpcRequest::new(method, Some(params), Some(json!(1)))
    }

    /// The refusal message, lowercased. Lowercased because what the assertions
    /// care about is that the operator is told *which concept* was refused and
    /// *what to type instead* — not the capitalisation of the first word.
    fn error_message(response: &JsonRpcResponse) -> String {
        response
            .error
            .as_ref()
            .map(|e| e.message.to_lowercase())
            .unwrap_or_default()
    }

    fn empty_config() -> Arc<RwLock<Config>> {
        Arc::new(RwLock::new(Config::default()))
    }

    /// A keyword rule reaches no code: `register_custom_commands` skips every
    /// rule whose regex does not start with `^/`. Accepting one over RPC
    /// therefore writes a rule to the operator's TOML that will never fire —
    /// a gate that lets the user do a thing that silently does nothing.
    ///
    /// The assertion is on the *effect* (nothing was added to `rules`), not on
    /// the fact that an error object came back.
    #[tokio::test]
    async fn create_refuses_a_keyword_rule_and_stores_nothing() {
        let _home = IsolatedAlephHome::new();
        let config = empty_config();
        let bus = Arc::new(GatewayEventBus::new());

        let response = handle_create(
            request(
                "routing_rules.create",
                json!({ "rule": { "regex": "translate to English",
                                  "system_prompt": "Translate to English" } }),
            ),
            Arc::clone(&config),
            bus,
        )
        .await;

        assert!(
            config.read().await.rules.is_empty(),
            "a refused rule must not be stored"
        );
        let message = error_message(&response);
        assert!(
            message.contains("keyword") && message.contains("^/"),
            "the operator must be able to learn why the rule was refused; got {message:?}"
        );
    }

    /// `update` is the second face of the same verb. A gate on `create` alone
    /// leaves a two-step path that is legal at every step and equivalent in
    /// effect: create `^/x`, then update it to a keyword regex.
    #[tokio::test]
    async fn update_refuses_a_keyword_rule_and_leaves_the_stored_rule_intact() {
        let _home = IsolatedAlephHome::new();
        let config = empty_config();
        config
            .write()
            .await
            .rules
            .push(RoutingRuleConfig::command("^/draw", "openai", None));
        let bus = Arc::new(GatewayEventBus::new());

        let response = handle_update(
            request(
                "routing_rules.update",
                json!({ "index": 0, "rule": { "regex": "draw me a picture" } }),
            ),
            Arc::clone(&config),
            bus,
        )
        .await;

        assert_eq!(
            config.read().await.rules[0].regex,
            "^/draw",
            "a refused update must leave the stored rule untouched"
        );
        let message = error_message(&response);
        assert!(
            message.contains("keyword") && message.contains("^/"),
            "the operator must be able to learn why the update was refused; got {message:?}"
        );
    }

    /// The other spelling of the retired concept. A `^/` regex labelled
    /// `"keyword"` *would* be registered, so the regex gate alone lets it
    /// through — and then `routing_rules.list` reports `get_rule_type()`
    /// verbatim and the Panel labels a working command "KEYWORD".
    #[tokio::test]
    async fn create_refuses_a_rule_labelled_keyword_even_with_a_slash_regex() {
        let _home = IsolatedAlephHome::new();
        let config = empty_config();
        let bus = Arc::new(GatewayEventBus::new());

        let response = handle_create(
            request(
                "routing_rules.create",
                json!({ "rule": { "regex": "^/draw", "rule_type": "keyword",
                                  "provider": "openai" } }),
            ),
            Arc::clone(&config),
            bus,
        )
        .await;

        assert!(
            config.read().await.rules.is_empty(),
            "a rule naming the retired kind must not be stored"
        );
        let message = error_message(&response);
        assert!(
            message.contains("keyword") && message.contains("command"),
            "the refusal must name the retired kind and the surviving one; got {message:?}"
        );
    }

    /// The half being kept. A command rule must still round-trip through
    /// `create` and land in `rules` with its prompt intact.
    #[tokio::test]
    async fn create_still_accepts_a_command_rule() {
        let _home = IsolatedAlephHome::new();
        let config = empty_config();
        let bus = Arc::new(GatewayEventBus::new());

        let response = handle_create(
            request(
                "routing_rules.create",
                json!({ "rule": { "regex": "^/draw", "provider": "openai",
                                  "system_prompt": "Draw a picture" } }),
            ),
            Arc::clone(&config),
            bus,
        )
        .await;

        assert!(
            response.error.is_none(),
            "a command rule must still be accepted; got {:?}",
            response.error
        );
        let cfg = config.read().await;
        assert_eq!(cfg.rules.len(), 1);
        assert_eq!(cfg.rules[0].regex, "^/draw");
        assert_eq!(
            cfg.rules[0].system_prompt.as_deref(),
            Some("Draw a picture")
        );
    }
}
