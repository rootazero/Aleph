//! RPC handlers for System State Bus.

use crate::error::{AlephError, Result};
use crate::gateway::context::GatewayContext;
use crate::perception::state_bus::{SubscribeParams, SubscribeResult, UnsubscribeParams};
use crate::perception::{ActionRequest, ActionResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use tracing::info;

/// Query historical state parameters.
#[derive(Debug, Clone, Deserialize)]
pub struct QueryStateParams {
    /// Application bundle ID
    pub app_id: String,

    /// Target timestamp (seconds since epoch)
    pub timestamp: u64,

    /// Maximum age (seconds) - query fails if timestamp is older
    #[serde(default = "default_max_age")]
    pub max_age_secs: u64,
}

fn default_max_age() -> u64 {
    30
}

/// Query state result.
#[derive(Debug, Clone, Serialize)]
pub struct QueryStateResult {
    /// Application state at requested timestamp
    pub state: Option<Value>,

    /// Actual timestamp of returned state
    pub timestamp: u64,

    /// Whether state was found
    pub found: bool,
}

/// Handle system.state.subscribe RPC method.
pub async fn handle_subscribe(
    _ctx: Arc<GatewayContext>,
    params: Value,
) -> Result<Value> {
    let params: SubscribeParams = serde_json::from_value(params)
        .map_err(|e| AlephError::invalid_input(format!("Invalid subscribe params: {}", e)))?;

    info!("Subscribing to state patterns: {:?}", params.patterns);

    // Generate subscription ID
    let subscription_id = uuid::Uuid::new_v4().to_string();

    // TODO: Store subscription in session state

    let result = SubscribeResult {
        subscription_id,
        active_patterns: params.patterns,
        initial_snapshot: if params.include_snapshot {
            // TODO: Get initial snapshot from state cache
            Some(serde_json::json!({}))
        } else {
            None
        },
    };

    Ok(serde_json::to_value(result)?)
}

/// Handle system.state.unsubscribe RPC method.
pub async fn handle_unsubscribe(
    _ctx: Arc<GatewayContext>,
    params: Value,
) -> Result<Value> {
    let params: UnsubscribeParams = serde_json::from_value(params)
        .map_err(|e| AlephError::invalid_input(format!("Invalid unsubscribe params: {}", e)))?;

    info!("Unsubscribing: {}", params.subscription_id);

    // TODO: Remove subscription from session state

    Ok(serde_json::json!({ "success": true }))
}

/// Handle system.state.query RPC method.
pub async fn handle_query(
    _ctx: Arc<GatewayContext>,
    params: Value,
) -> Result<Value> {
    let params: QueryStateParams = serde_json::from_value(params)
        .map_err(|e| AlephError::invalid_input(format!("Invalid query params: {}", e)))?;

    info!("Querying state for {} at timestamp {}", params.app_id, params.timestamp);

    // TODO: Get state history from SystemStateBus
    // For now, return not found
    let result = QueryStateResult {
        state: None,
        timestamp: params.timestamp,
        found: false,
    };

    Ok(serde_json::to_value(result)?)
}

/// Handle system.action.execute RPC method.
pub async fn handle_execute_action(
    _ctx: Arc<GatewayContext>,
    params: Value,
) -> Result<Value> {
    let request: ActionRequest = serde_json::from_value(params)
        .map_err(|e| AlephError::invalid_input(format!("Invalid action request: {}", e)))?;

    info!("Executing action on element: {}", request.target_id);

    // TODO: Get ActionDispatcher from context
    // For now, return not implemented
    let result = ActionResult {
        success: false,
        error: Some("Action dispatcher not yet integrated with Gateway".to_string()),
        used_fallback: false,
    };

    Ok(serde_json::to_value(result)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perception::{ActionMethod, ConditionType, ExpectCondition};

    #[tokio::test]
    async fn test_subscribe_params_parsing() {
        let params = serde_json::json!({
            "patterns": ["system.state.com.apple.mail.*"],
            "include_snapshot": true
        });

        let parsed: SubscribeParams = serde_json::from_value(params).unwrap();
        assert_eq!(parsed.patterns.len(), 1);
        assert!(parsed.include_snapshot);
    }

    #[tokio::test]
    async fn test_unsubscribe_params_parsing() {
        let params = serde_json::json!({
            "subscription_id": "test-123"
        });

        let parsed: UnsubscribeParams = serde_json::from_value(params).unwrap();
        assert_eq!(parsed.subscription_id, "test-123");
    }

    #[tokio::test]
    async fn test_query_params_parsing() {
        let params = serde_json::json!({
            "app_id": "com.apple.mail",
            "timestamp": 1739268000,
            "max_age_secs": 30
        });

        let parsed: QueryStateParams = serde_json::from_value(params).unwrap();
        assert_eq!(parsed.app_id, "com.apple.mail");
        assert_eq!(parsed.timestamp, 1739268000);
        assert_eq!(parsed.max_age_secs, 30);
    }

    #[tokio::test]
    async fn test_action_request_parsing() {
        let params = serde_json::json!({
            "target_id": "btn_send_001",
            "method": {
                "type": "click"
            },
            "expect": {
                "condition": {
                    "type": "element_disappear"
                },
                "timeout_ms": 500
            }
        });

        let parsed: ActionRequest = serde_json::from_value(params).unwrap();
        assert_eq!(parsed.target_id, "btn_send_001");
        assert!(matches!(parsed.method, ActionMethod::Click));
        assert!(matches!(parsed.expect.condition, ConditionType::ElementDisappear));
    }
}
