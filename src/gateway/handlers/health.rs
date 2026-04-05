//! Health Check Handler

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;

use super::schema::{HandlerSchema, NoParams, TypedHandler};
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse};

pub struct HealthHandler;

impl HandlerSchema for HealthHandler {
    type Params = NoParams;
    const METHOD: &'static str = "health";
    const DESCRIPTION: &'static str = "Returns the health status of the Gateway server";

    fn handle_with_params(
        id: Option<Value>,
        _params: Self::Params,
    ) -> impl Future<Output = JsonRpcResponse> + Send + 'static
    where
        Self: Sized,
    {
        async move {
            JsonRpcResponse::success(
                id,
                json!({
                    "status": "healthy",
                    "timestamp": chrono::Utc::now().to_rfc3339()
                }),
            )
        }
    }
}

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
    use super::super::schema::TypedHandler;
    use super::{JsonRpcRequest, HealthHandler};
    use serde_json::json;

    #[tokio::test]
    async fn test_health_response() {
        let handler = TypedHandler::<HealthHandler>::new();
        let request = JsonRpcRequest::with_id("health", None, json!(1));
        let response = handler.handle(&request).await;

        assert!(response.is_success());

        let result = response.result.unwrap();
        assert_eq!(result["status"], "healthy");
        assert!(result["timestamp"].is_string());
    }
}
