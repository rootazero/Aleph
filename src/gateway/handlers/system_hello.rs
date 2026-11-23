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
    negotiate(
        request,
        format!("conn-{}", Uuid::new_v4()),
        // Legacy wire hints, not enforced transport limits.
        HelloLimits {
            max_message_bytes: 8_388_608,
            max_inflight_requests: 128,
        },
        None,
    )
}

const SUBJECT_DOMAIN: &str = "aleph.panel.subject.v1\0";

/// Opaque per-user value for client storage keys: the first 16 bytes of
/// HMAC-SHA256 under the Gateway's shared-token secret, hex encoded. It
/// never contains the user id and changes when the secret is rotated, which
/// only resets client presentation state.
pub(crate) fn panel_subject(secret: &[u8; 32], user: &str) -> String {
    let mut mac =
        crate::gateway::security::crypto::hmac_sign(secret, &format!("{SUBJECT_DOMAIN}{user}"));
    mac.truncate(32);
    mac
}

/// The subject for `user`, or `None` when the store has no secret. A missing
/// subject only makes the client fall back to an endpoint-wide layout.
pub(crate) fn subject_for(
    store: &crate::gateway::security::SecurityStore,
    user: &str,
) -> Option<String> {
    let secret = store.get_shared_token_secret().ok().flatten()?;
    Some(panel_subject(&secret, user))
}

pub(crate) fn negotiate(
    request: JsonRpcRequest,
    connection_id: String,
    limits: HelloLimits,
    subject: Option<String>,
) -> JsonRpcResponse {
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
        connection_id,
        features: negotiate_features(&params.features, &[]),
        limits,
        subject,
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

    #[test]
    fn panel_subject_is_stable_and_user_scoped() {
        let secret = [7u8; 32];
        let owner = super::panel_subject(&secret, "u-owner");
        assert_eq!(owner, super::panel_subject(&secret, "u-owner"));
        assert_ne!(owner, super::panel_subject(&secret, "u-member"));
        assert_ne!(owner, super::panel_subject(&[8u8; 32], "u-owner"));
        assert_eq!(owner.len(), 32);
        assert!(owner.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(!owner.contains("owner"));
    }

    #[test]
    fn hello_without_secret_omits_subject() {
        let store = crate::gateway::security::SecurityStore::in_memory().unwrap();
        assert_eq!(super::subject_for(&store, "u-owner"), None);
    }
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
