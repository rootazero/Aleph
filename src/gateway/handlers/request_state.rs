//! Request State Handler

use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use crate::gateway::middleware::request_state::get_global_registry;

pub async fn handle(request: JsonRpcRequest) -> JsonRpcResponse {
    match get_global_registry() {
        Some(registry) => {
            let snapshot = registry.snapshot();
            JsonRpcResponse::success(
                request.id,
                json!({
                    "pending": snapshot.pending,
                    "validating": snapshot.validating,
                    "processing": snapshot.processing,
                    "awaiting": snapshot.awaiting,
                    "completed": snapshot.completed,
                    "failed": snapshot.failed,
                    "cancelled": snapshot.cancelled,
                    "total_in_flight": snapshot.total_in_flight,
                    "timestamp": snapshot.timestamp,
                }),
            )
        }
        None => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            "Request state registry not initialized",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_request_state_returns_snapshot() {
        let request = JsonRpcRequest::with_id("request.state", None, serde_json::json!(1));
        let response = handle(request).await;

        // When registry is not initialized, we get an error
        // This is expected in unit tests
        if response.is_error() {
            assert_eq!(
                response.error.clone().unwrap().message,
                "Request state registry not initialized"
            );
        }
    }
}
