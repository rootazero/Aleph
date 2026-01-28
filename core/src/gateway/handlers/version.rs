//! Version Handler
//!
//! Returns version information about the Gateway server.

use serde_json::json;
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse};

/// Handle version requests
///
/// Returns a JSON object with:
/// - `name`: Server name ("aether-gateway")
/// - `version`: Crate version from Cargo.toml
/// - `protocol`: Protocol version ("json-rpc-2.0")
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"version","id":1}
/// ```
///
/// # Example Response
///
/// ```json
/// {"jsonrpc":"2.0","result":{"name":"aether-gateway","version":"0.1.0","protocol":"json-rpc-2.0"},"id":1}
/// ```
pub async fn handle(request: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::success(
        request.id,
        json!({
            "name": "aether-gateway",
            "version": env!("CARGO_PKG_VERSION"),
            "protocol": "json-rpc-2.0"
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_version_response() {
        let request = JsonRpcRequest::new("version", None, Some(json!(1)));
        let response = handle(request).await;

        assert!(response.is_success());

        let result = response.result.unwrap();
        assert_eq!(result["name"], "aether-gateway");
        assert_eq!(result["protocol"], "json-rpc-2.0");
        assert!(result["version"].is_string());
    }
}
