use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// JSON-RPC 2.0 request with u64 id (replaces old String-based id).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Request {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Response {
    pub jsonrpc: String,
    pub id: u64,
    pub result: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ErrorResponse {
    pub jsonrpc: String,
    pub id: Option<u64>,
    pub error: RpcError,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// Union over RPC message kinds (for parsing received messages on the client).
/// Order matters for untagged deserialize: most specific first.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    Error(ErrorResponse),
    Response(Response),
    Notification(Notification),
}


#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_roundtrip_with_u64_id() {
        let req = Request {
            jsonrpc: "2.0".into(),
            id: 42,
            method: "bridge.ping".into(),
            params: None,
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(
            v,
            json!({
                "jsonrpc": "2.0", "id": 42, "method": "bridge.ping"
            })
        );
        let back: Request = serde_json::from_value(v).unwrap();
        assert_eq!(back.id, 42);
    }

    #[test]
    fn notification_has_no_id() {
        let n = Notification {
            jsonrpc: "2.0".into(),
            method: "bridge.shutdown".into(),
            params: None,
        };
        let v = serde_json::to_value(&n).unwrap();
        assert!(v.get("id").is_none());
    }
}
