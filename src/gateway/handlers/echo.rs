//! Echo Handler

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse};
use serde_json::json;

pub async fn handle(request: JsonRpcRequest) -> JsonRpcResponse {
    let id = request.id.clone();
    JsonRpcResponse::success(id, json!({ "echo": request.params }))
}

#[cfg(test)]
mod tests {
    use super::{handle, JsonRpcRequest};
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
        let request = JsonRpcRequest::with_id("echo", None, json!(1));
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
