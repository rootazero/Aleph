//! Health Check Handler

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse};
use serde_json::json;

pub async fn handle(request: JsonRpcRequest) -> JsonRpcResponse {
    let id = request.id.clone();
    JsonRpcResponse::success(
        id,
        json!({
            "status": "healthy",
            "timestamp": chrono::Utc::now().to_rfc3339()
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::{handle, JsonRpcRequest};
    use serde_json::json;

    #[tokio::test]
    async fn test_health_response() {
        let request = JsonRpcRequest::with_id("health", None, json!(1));
        let response = handle(request).await;

        assert!(response.is_success());

        let result = response.result.unwrap();
        assert_eq!(result["status"], "healthy");
        assert!(result["timestamp"].is_string());
    }
}
