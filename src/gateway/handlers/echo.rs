//! Echo Handler

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::future::Future;
use std::pin::Pin;

use super::schema::{HandlerSchema, TypedHandler};
use super::super::protocol::{JsonRpcRequest, JsonRpcResponse};

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct EchoParams(pub Option<Value>);

impl EchoParams {
    pub fn new(value: Option<Value>) -> Self {
        Self(value)
    }
}

pub struct EchoHandler;

impl HandlerSchema for EchoHandler {
    type Params = EchoParams;
    const METHOD: &'static str = "echo";
    const DESCRIPTION: &'static str = "Echoes back the request parameters for testing purposes";

    fn handle_with_params(
        id: Option<Value>,
        params: Self::Params,
    ) -> impl Future<Output = JsonRpcResponse> + Send + 'static
    where
        Self: Sized,
    {
        async move {
            JsonRpcResponse::success(id, json!({ "echo": params.0 }))
        }
    }
}

pub async fn handle(request: JsonRpcRequest) -> JsonRpcResponse {
    let id = request.id.clone();
    JsonRpcResponse::success(id, json!({ "echo": request.params }))
}

#[cfg(test)]
mod tests {
    use super::super::schema::TypedHandler;
    use super::{JsonRpcRequest, EchoHandler};
    use serde_json::json;

    #[tokio::test]
    async fn test_echo_with_params() {
        let handler = TypedHandler::<EchoHandler>::new();
        let request = JsonRpcRequest::new(
            "echo",
            Some(json!({"message": "hello", "count": 42})),
            Some(json!(1)),
        );
        let response = handler.handle(&request).await;

        assert!(response.is_success());

        let result = response.result.unwrap();
        assert_eq!(result["echo"]["message"], "hello");
        assert_eq!(result["echo"]["count"], 42);
    }

    #[tokio::test]
    async fn test_echo_without_params() {
        let handler = TypedHandler::<EchoHandler>::new();
        let request = JsonRpcRequest::with_id("echo", None, json!(1));
        let response = handler.handle(&request).await;

        assert!(response.is_success());

        let result = response.result.unwrap();
        assert!(result["echo"].is_null());
    }

    #[tokio::test]
    async fn test_echo_preserves_id() {
        let handler = TypedHandler::<EchoHandler>::new();
        let request = JsonRpcRequest::new("echo", Some(json!("test")), Some(json!("custom-id")));
        let response = handler.handle(&request).await;

        assert_eq!(response.id, Some(json!("custom-id")));
    }
}
