//! RPC handlers for System State Bus.

use crate::error::{AlephError, Result};
use crate::gateway::context::GatewayContext;
use crate::perception::state_bus::{SubscribeParams, SubscribeResult, UnsubscribeParams};
use serde_json::Value;
use std::sync::Arc;
use tracing::info;

/// Handle system.state.subscribe RPC method.
pub async fn handle_subscribe(
    ctx: Arc<GatewayContext>,
    params: Value,
) -> Result<Value> {
    let params: SubscribeParams = serde_json::from_value(params)
        .map_err(|e| AlephError::invalid_input(format!("Invalid subscribe params: {}", e)))?;

    info!("Subscribing to state patterns: {:?}", params.patterns);

    // Generate subscription ID
    let subscription_id = uuid::Uuid::new_v4().to_string();

    // TODO: Store subscription in session state
    // For now, just return success

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
    ctx: Arc<GatewayContext>,
    params: Value,
) -> Result<Value> {
    let params: UnsubscribeParams = serde_json::from_value(params)
        .map_err(|e| AlephError::invalid_input(format!("Invalid unsubscribe params: {}", e)))?;

    info!("Unsubscribing: {}", params.subscription_id);

    // TODO: Remove subscription from session state

    Ok(serde_json::json!({ "success": true }))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
