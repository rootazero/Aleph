//! JSON-RPC 2.0 protocol helpers for Desktop Bridge

use aleph_protocol::desktop_bridge::{
    BridgeErrorResponse, BridgeRpcError, BridgeRequest, BridgeSuccessResponse,
    ERR_INTERNAL, ERR_PARSE,
};
use serde_json::Value;

/// Parse a JSON line into a BridgeRequest
pub fn parse_request(line: &str) -> Result<BridgeRequest, BridgeErrorResponse> {
    serde_json::from_str::<BridgeRequest>(line).map_err(|e| BridgeErrorResponse {
        jsonrpc: "2.0".into(),
        id: "null".into(),
        error: BridgeRpcError {
            code: ERR_PARSE,
            message: format!("Parse error: {}", e),
        },
    })
}

/// Create a success response
pub fn success_response(id: &str, result: Value) -> String {
    let resp = BridgeSuccessResponse {
        jsonrpc: "2.0".into(),
        id: id.into(),
        result,
    };
    serde_json::to_string(&resp).unwrap_or_else(|_| error_response_str(id, ERR_INTERNAL, "encode failed"))
}

/// Create an error response
pub fn error_response(id: &str, code: i32, message: &str) -> String {
    error_response_str(id, code, message)
}

fn error_response_str(id: &str, code: i32, message: &str) -> String {
    let resp = BridgeErrorResponse {
        jsonrpc: "2.0".into(),
        id: id.into(),
        error: BridgeRpcError {
            code,
            message: message.into(),
        },
    };
    serde_json::to_string(&resp).unwrap_or_else(|_| {
        format!(
            r#"{{"jsonrpc":"2.0","id":"{}","error":{{"code":{},"message":"encode failed"}}}}"#,
            id, code
        )
    })
}
