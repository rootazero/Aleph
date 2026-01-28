//! Echo Handler
//!
//! Echoes back the request parameters for testing purposes.

use serde_json::json;
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse};

/// Handle echo requests
///
/// Returns the request parameters wrapped in an "echo" field.
/// Useful for testing WebSocket connectivity and message parsing.
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"echo","params":{"hello":"world"},"id":1}
/// ```
///
/// # Example Response
///
/// ```json
/// {"jsonrpc":"2.0","result":{"echo":{"hello":"world"}},"id":1}
/// ```
pub async fn handle(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::success(
        request.id.clone(),
        json!({
            "echo": request.params
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_echo_with_params() {
        let request = JsonRpcRequest::new(
            "echo",
            Some(json!({"message": "hello", "count": 42})),
            Some(json!(1)),
        );
        let response = handle(request).await;

        assert!(response.is_success());

        let result = response.result.unwrap();
        assert_eq!(result["echo"]["message"], "hello");
        assert_eq!(result["echo"]["count"], 42);
    }

    #[tokio::test]
    async fn test_echo_without_params() {
        let request = JsonRpcRequest::new("echo", None, Some(json!(1)));
        let response = handle(request).await;

        assert!(response.is_success());

        let result = response.result.unwrap();
        assert!(result["echo"].is_null());
    }

    #[tokio::test]
    async fn test_echo_preserves_id() {
        let request = JsonRpcRequest::new("echo", Some(json!("test")), Some(json!("custom-id")));
        let response = handle(request).await;

        assert_eq!(response.id, Some(json!("custom-id")));
    }
}
