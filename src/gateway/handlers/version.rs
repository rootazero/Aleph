//! Version Handler

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse};
use super::schema::{HandlerSchema, NoParams};
use serde_json::{json, Value};

pub struct VersionHandler;

impl HandlerSchema for VersionHandler {
    type Params = NoParams;
    const METHOD: &'static str = "version";
    const DESCRIPTION: &'static str = "Returns version information about the Gateway server";

    async fn handle_with_params(id: Option<Value>, _params: Self::Params) -> JsonRpcResponse {
        JsonRpcResponse::success(
            id,
            json!({
                "name": "aleph-gateway",
                "version": env!("ALEPH_VERSION"),
                "protocol": "json-rpc-2.0"
            }),
        )
    }
}

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
    use super::super::schema::TypedHandler;
    use super::{JsonRpcRequest, VersionHandler};
    use serde_json::json;

    #[tokio::test]
    async fn test_version_response() {
        let handler = TypedHandler::<VersionHandler>::new();
        let request = JsonRpcRequest::with_id("version", None, json!(1));
        let response = handler.handle(&request).await;

        assert!(response.is_success());

        let result = response.result.unwrap();
        assert_eq!(result["name"], "aleph-gateway");
        assert_eq!(result["protocol"], "json-rpc-2.0");
        assert!(result["version"].is_string());
    }
}
