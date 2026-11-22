//! system.hello Protocol Handshake Handler
//!
//! Legacy protocol probe after the WebSocket `connect` handshake.
//!
//! This handler neither admits a connection nor sets its identity.

use aleph_protocol::system_hello::{
    negotiate_features, negotiate_version, HelloLimits, HelloParams, HelloResult, ProtocolVersion,
    ServerHello,
};
use uuid::Uuid;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};

/// Handle `system.hello` requests.
///
/// Parses `request.params` as `HelloParams`. Returns `INVALID_PARAMS` when the
/// client's protocol major version does not match the server's (currently 1).
/// On success, returns a compatible version and the intersection of
/// requested and implemented features.
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

    let Some(protocol) =
        negotiate_version(&params.protocol, &ProtocolVersion { major: 1, minor: 0 })
    else {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "incompatible protocol major version",
        );
    };

    let result = HelloResult {
        protocol,
        server: ServerHello {
            name: "aleph-server".to_string(),
            version: env!("ALEPH_VERSION").to_string(),
        },
        connection_id: format!("conn-{}", Uuid::new_v4()),
        features: negotiate_features(&params.features, &[]),
        // Retained for legacy wire compatibility, not enforced transport limits.
        limits: HelloLimits {
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
    async fn hello_reports_implemented_features_and_negotiated_minor() {
        let params = json!({
            "protocol": {"major": 1, "minor": u32::MAX},
            "client": {"name": "aleph-nsh-desktop", "version": "0.1.0", "platform": "macos", "ui_runtime": "nsh"},
            "features": ["event_replay", "snapshot", "binary_channel", "future"]
        });
        let resp = handle(req(params)).await;
        assert_eq!(resp.id, Some(json!(1)));
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(
            (&result["protocol"], &result["features"]),
            (&json!({"major": 1, "minor": 0}), &json!([]))
        );
        assert_eq!(result["server"]["name"], "aleph-server");
        assert!(!result["connection_id"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn major_version_mismatch_returns_error_not_success() {
        let params = json!({
            "protocol": {"major": 2, "minor": 0},
            "client": {"name": "aleph-nsh-desktop", "version": "0.1.0", "platform": "macos", "ui_runtime": "nsh"},
            "features": []
        });
        let resp = handle(req(params)).await;
        assert_eq!(resp.id, Some(json!(1)));
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, -32602);
    }

    #[tokio::test]
    async fn invalid_hello_params_keep_request_id() {
        for params in [
            None,
            Some(json!({"protocol": {"major": 1, "minor": 0}, "features": []})),
            Some(json!({
                "protocol": {"major": 1, "minor": "0"},
                "client": {"name": "test", "version": "0.1.0", "platform": "macos", "ui_runtime": "nsh"},
                "features": []
            })),
        ] {
            let resp = handle(JsonRpcRequest::with_id("system.hello", params, json!(1))).await;
            assert_eq!(resp.id, Some(json!(1)));
            assert!(resp.result.is_none());
            assert_eq!(resp.error.unwrap().code, -32602);
        }
    }
}
