//! Health Check Handler
//!
//! Returns the health status of the Gateway server.

use serde_json::json;
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse};

/// Handle health check requests
///
/// Returns a JSON object with:
/// - `status`: "healthy" if the server is operating normally
/// - `timestamp`: ISO 8601 formatted timestamp
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"health","id":1}
/// ```
///
/// # Example Response
///
/// ```json
/// {"jsonrpc":"2.0","result":{"status":"healthy","timestamp":"2024-01-15T10:30:00Z"},"id":1}
/// ```
pub async fn handle(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::success(
        request.id,
        json!({
            "status": "healthy",
            "timestamp": chrono::Utc::now().to_rfc3339()
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_health_response() {
        let request = JsonRpcRequest::new("health", None, Some(json!(1)));
        let response = handle(request).await;

        assert!(response.is_success());

        let result = response.result.unwrap();
        assert_eq!(result["status"], "healthy");
        assert!(result["timestamp"].is_string());
    }
}
