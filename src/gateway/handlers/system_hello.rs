//! system.hello Protocol Handshake Handler
//!
//! Negotiates the protocol version with the NSH desktop client and returns
//! a connection id plus the intersection of supported features. This is a
//! protocol-layer addition only: no auth, no gate, no business logic. It
//! coexists with the existing `connect` WebSocket device-auth handshake.

use aleph_protocol::system_hello::{HelloParams, HelloResult};
use uuid::Uuid;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};

/// Handle `system.hello` requests.
///
/// Parses `request.params` as `HelloParams`. Returns `INVALID_PARAMS` when the
/// client's protocol major version does not match the server's (currently 1).
/// On success, echoes the negotiated protocol, returns the server identity,
/// a fresh connection id, and the intersection of requested and known features.
pub async fn handle(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: HelloParams = match request.params.clone() {
        Some(p) => match serde_json::from_value(p) {
            Ok(v) => v,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid system.hello params: {e}"),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params");
        }
    };

    if params.protocol.major != 1 {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "incompatible protocol major version",
        );
    }

    // Narrow requested features to the known set. Unknown strings from
    // future clients are dropped, not rejected, so older servers stay
    // forward-compatible.
    let features: Vec<aleph_protocol::system_hello::Feature> = params
        .features
        .iter()
        .filter(|f| f.is_known())
        .cloned()
        .collect();

    let result = HelloResult {
        protocol: params.protocol,
        server: aleph_protocol::system_hello::ServerHello {
            name: "aleph-server".to_string(),
            version: env!("ALEPH_VERSION").to_string(),
        },
        connection_id: format!("conn-{}", Uuid::new_v4()),
        features,
        limits: aleph_protocol::system_hello::HelloLimits {
            max_message_bytes: 8_388_608,
            max_inflight_requests: 128,
        },
    };

    let result_value = match serde_json::to_value(&result) {
        Ok(v) => v,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                super::super::protocol::INTERNAL_ERROR,
                format!("Failed to serialize system.hello result: {e}"),
            );
        }
    };

    JsonRpcResponse::success(request.id, result_value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn req(params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest::with_id("system.hello", Some(params), json!(1))
    }

    #[tokio::test]
    async fn happy_path_returns_server_version_and_echoed_protocol() {
        let params = json!({
            "protocol": {"major": 1, "minor": 0},
            "client": {"name": "aleph-nsh-desktop", "version": "0.1.0", "platform": "macos", "ui_runtime": "nsh"},
            "features": ["event_replay", "snapshot"]
        });
        let resp = handle(req(params)).await;
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["protocol"]["major"], 1);
        assert_eq!(result["server"]["name"], "aleph-server");
        assert!(result["connection_id"].as_str().unwrap().starts_with("conn-"));
        assert!(result["features"].as_array().unwrap().contains(&json!("event_replay")));
    }

    #[tokio::test]
    async fn major_version_mismatch_returns_error_not_success() {
        let params = json!({
            "protocol": {"major": 2, "minor": 0},
            "client": {"name": "aleph-nsh-desktop", "version": "0.1.0", "platform": "macos", "ui_runtime": "nsh"},
            "features": []
        });
        let resp = handle(req(params)).await;
        assert!(resp.result.is_none());
        assert!(resp.error.is_some());
    }

    #[tokio::test]
    async fn unknown_feature_is_narrowed_not_rejected() {
        let params = json!({
            "protocol": {"major": 1, "minor": 0},
            "client": {"name": "aleph-nsh-desktop", "version": "0.1.0", "platform": "macos", "ui_runtime": "nsh"},
            "features": ["event_replay", "nonexistent_feature"]
        });
        let resp = handle(req(params)).await;
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let features = result["features"].as_array().unwrap();
        assert!(features.contains(&json!("event_replay")));
        assert!(!features.contains(&json!("nonexistent_feature")));
    }
}
