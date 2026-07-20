//! Version Handler

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse};
use serde_json::json;

pub async fn handle(request: JsonRpcRequest) -> JsonRpcResponse {
    let id = request.id.clone();
    JsonRpcResponse::success(
        id,
        json!({
            "name": "aleph-gateway",
            "version": env!("ALEPH_VERSION"),
            "protocol": "json-rpc-2.0"
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::{handle, JsonRpcRequest};
    use serde_json::json;

    #[tokio::test]
    async fn test_version_response() {
        let request = JsonRpcRequest::with_id("version", None, json!(1));
        let response = handle(request).await;

        assert!(response.is_success());

        let result = response.result.unwrap();
        assert_eq!(result["name"], "aleph-gateway");
        assert_eq!(result["protocol"], "json-rpc-2.0");
        assert!(result["version"].is_string());
    }
}
