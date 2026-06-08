//! Pairing handlers
//!
//! Handles device pairing approval, rejection, and listing.

use crate::sync_primitives::Arc;
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::gateway::device_store::ApprovedDevice;
use crate::gateway::events::GatewayEventFrame;
use crate::gateway::handlers::parse_params;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, AUTH_FAILED};
use crate::gateway::security::store::DeviceUpsertData;
use crate::gateway::security::{DeviceRole, DeviceType, PairingRequest, PollState};

use super::AuthContext;

#[derive(Debug, Deserialize)]
struct ApproveParams {
    code: String,
    /// `"config"` → operator (full control). Anything else / missing →
    /// chat tier (safe default): chat + read, no Aleph-config rights.
    #[serde(default)]
    level: Option<String>,
}

/// Handle "pairing.approve" request (from authorized client or CLI)
pub async fn handle_pairing_approve(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    let params: ApproveParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Confirm pairing (removes from pending and returns the request)
    let pairing_request = match ctx.pairing_manager.confirm_pairing(&params.code) {
        Ok(req) => req,
        Err(e) => {
            warn!(code = %params.code, error = %e, "Invalid or expired pairing code");
            return JsonRpcResponse::error(
                request.id,
                AUTH_FAILED,
                "Invalid or expired pairing code",
            );
        }
    };

    // Extract device info from pairing request
    let (device_name, device_type): (String, Option<String>) = match &pairing_request {
        PairingRequest::Device {
            device_name,
            device_type,
            ..
        } => (
            device_name.clone(),
            device_type.map(|t: DeviceType| t.as_str().to_string()),
        ),
        PairingRequest::Channel {
            channel, sender_id, ..
        } => {
            // Channel pairing approved - approve the sender via SecurityStore
            if let Err(e) = ctx.security_store.approve_sender(channel, sender_id) {
                warn!(error = %e, "Failed to approve sender");
                return JsonRpcResponse::error(
                    request.id,
                    -32603,
                    format!("Failed to approve sender: {}", e),
                );
            }
            info!(channel = %channel, sender_id = %sender_id, "Channel sender approved");
            return JsonRpcResponse::success(
                request.id,
                json!({
                    "channel": channel,
                    "sender_id": sender_id,
                    "approved": true,
                }),
            );
        }
        PairingRequest::Browser {
            code,
            origin_label,
            user_agent,
            ..
        } => {
            // Browser pairing always lands at the Chat tier (chat + read, no
            // config rights). The operator can later elevate it to config via
            // the Devices list (devices.set_level). We mint a *persistent*
            // chat-tier device — identical to the Device branch but tier-forced
            // — and hand its device token to the browser via pairing.poll, so
            // it connects through Case 1 (role derived from permissions =
            // "guest"). This is remote-capable; it does NOT ride the
            // loopback-only shared-token cookie bootstrap.
            let tier = super::tier::Tier::Chat;
            let tier_permissions = tier.permissions();

            let device_id = format!("browser-{}", uuid::Uuid::new_v4());
            let device_name = if !origin_label.is_empty() {
                origin_label.clone()
            } else if !user_agent.is_empty() {
                user_agent.clone()
            } else {
                "Browser".to_string()
            };
            let mut device = ApprovedDevice::new(device_id.clone(), device_name.clone(), None);
            device.permissions = tier_permissions.clone();

            if let Err(e) = ctx.device_store.approve_device(&device) {
                warn!(error = %e, "Failed to store approved browser device");
                return JsonRpcResponse::error(
                    request.id,
                    -32603,
                    format!("Failed to store device: {}", e),
                );
            }

            let device_fingerprint: String = device_id.chars().take(16).collect();
            if let Err(e) = ctx.security_store.upsert_device(&DeviceUpsertData {
                device_id: &device_id,
                device_name: &device_name,
                device_type: None,
                public_key: &[0u8; 32],
                fingerprint: &device_fingerprint,
                role: super::tier::role_for_permissions(&tier_permissions),
                scopes: &tier_permissions,
            }) {
                warn!(error = %e, "Failed to register browser device in security store");
                return JsonRpcResponse::error(
                    request.id,
                    -32603,
                    format!("Failed to register device: {}", e),
                );
            }

            let signed_token = match ctx.token_manager.issue_token(
                &device_id,
                DeviceRole::Operator, // memory namespace only; authz uses connect-response role
                tier_permissions.clone(),
            ) {
                Ok(t) => t,
                Err(e) => {
                    warn!(error = %e, "Failed to issue browser device token");
                    return JsonRpcResponse::error(
                        request.id,
                        -32603,
                        format!("Failed to issue token: {}", e),
                    );
                }
            };
            let token = format!("{}:{}", signed_token.token, signed_token.signature);

            ctx.pairing_manager
                .record_browser_credential(code, &token, &device_id);

            if let Err(e) = ctx
                .event_bus
                .publish_frame(&GatewayEventFrame::PairingCompleted {
                    device_id: code.clone(),
                })
            {
                warn!(error = %e, "Failed to publish PairingCompleted frame");
            }

            info!(
                code = %code,
                device_id = %device_id,
                origin = %origin_label,
                "Browser pairing approved as chat-tier device"
            );
            return JsonRpcResponse::success(
                request.id,
                json!({
                    "code": code,
                    "kind": "browser",
                    "device_id": device_id,
                    "approved": true,
                }),
            );
        }
        PairingRequest::Node {
            node_name, code, ..
        } => {
            // Mint a node-role credential (mirrors cluster.enroll) and stash the
            // combined "token:signature" bearer keyed by the pairing code for the
            // node's `pairing.poll` to drain single-use. Panel-only: the CLI
            // approve path rejects node codes (in-process side-table).
            let device_id = uuid::Uuid::new_v4().to_string();
            let fingerprint: String = device_id.chars().take(16).collect();
            if let Err(e) = ctx.security_store.upsert_device(&DeviceUpsertData {
                device_id: &device_id,
                device_name: node_name,
                device_type: None,
                public_key: &[0u8; 32],
                fingerprint: &fingerprint,
                role: DeviceRole::Node.as_str(),
                scopes: &["node".to_string()],
            }) {
                warn!(error = %e, "Failed to register node device");
                return JsonRpcResponse::error(
                    request.id,
                    -32603,
                    format!("Failed to register node: {}", e),
                );
            }
            let signed = match ctx.token_manager.issue_token(
                &device_id,
                DeviceRole::Node,
                vec!["node".to_string()],
            ) {
                Ok(t) => t,
                Err(e) => {
                    warn!(error = %e, "Failed to issue node token");
                    return JsonRpcResponse::error(
                        request.id,
                        -32603,
                        format!("Failed to issue node token: {}", e),
                    );
                }
            };
            let bearer = format!("{}:{}", signed.token, signed.signature);
            ctx.pairing_manager
                .record_browser_credential(code, &bearer, &device_id);
            info!(code = %code, node = %node_name, "Node pairing approved");
            return JsonRpcResponse::success(
                request.id,
                json!({
                    "code": code,
                    "kind": "node",
                    "device_id": device_id,
                    "approved": true,
                }),
            );
        }
    };

    // Determine tier permissions from the requested level.
    // Missing or unrecognised → Chat (safe default: chat + read, no config rights).
    let tier = super::tier::Tier::from_level(params.level.as_deref());
    let tier_permissions = tier.permissions();

    // Generate device ID and create approved device
    let device_id = uuid::Uuid::new_v4().to_string();
    let mut device = ApprovedDevice::new(device_id.clone(), device_name.clone(), device_type);
    device.permissions = tier_permissions.clone();

    // Store in device store
    if let Err(e) = ctx.device_store.approve_device(&device) {
        warn!(error = %e, "Failed to store approved device");
        return JsonRpcResponse::error(
            request.id,
            -32603,
            format!("Failed to store device: {}", e),
        );
    }

    // Register device in SecurityStore for token FK constraint
    let device_fingerprint: String = device_id.chars().take(16).collect();
    if let Err(e) = ctx.security_store.upsert_device(&DeviceUpsertData {
        device_id: &device_id,
        device_name: &device_name,
        device_type: None,
        public_key: &[0u8; 32],           // placeholder public key
        fingerprint: &device_fingerprint, // use device_id prefix as fingerprint
        role: super::tier::role_for_permissions(&tier_permissions),
        scopes: &tier_permissions,
    }) {
        warn!(error = %e, "Failed to register device in security store");
        return JsonRpcResponse::error(
            request.id,
            -32603,
            format!("Failed to register device: {}", e),
        );
    }

    // Generate token for the new device.
    // DeviceRole::Operator is kept (memory namespace only; authz gate uses
    // the connect-response role derived from permissions).
    let signed_token = match ctx.token_manager.issue_token(
        &device_id,
        DeviceRole::Operator,
        tier_permissions.clone(),
    ) {
        Ok(t) => t,
        Err(e) => {
            warn!(error = %e, "Failed to issue token");
            return JsonRpcResponse::error(
                request.id,
                -32603,
                format!("Failed to issue token: {}", e),
            );
        }
    };

    info!(
        device_id = %device_id,
        device_name = %device_name,
        "Device pairing approved"
    );

    JsonRpcResponse::success(
        request.id,
        json!({
            "device_id": device_id,
            "device_name": device_name,
            "token": format!("{}:{}", signed_token.token, signed_token.signature),
            "approved_at": device.approved_at,
        }),
    )
}

/// Handle "pairing.reject" request
pub async fn handle_pairing_reject(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    #[derive(Debug, Deserialize)]
    struct RejectParams {
        code: String,
    }

    let params: RejectParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Browser and node rejects both need the side-table marker so
    // `poll_browser_pairing` reports Rejected (not Expired) after the DB row
    // is removed by `cancel_pairing`.
    let needs_reject_marker = matches!(
        ctx.pairing_manager.get_request(&params.code),
        Ok(Some(
            PairingRequest::Browser { .. } | PairingRequest::Node { .. }
        ))
    );

    match ctx.pairing_manager.cancel_pairing(&params.code) {
        Ok(true) => {
            if needs_reject_marker {
                ctx.pairing_manager.mark_browser_rejected(&params.code);
            }
            info!(code = %params.code, "Pairing rejected");
            JsonRpcResponse::success(request.id, json!({"rejected": true}))
        }
        Ok(false) => {
            JsonRpcResponse::error(request.id, AUTH_FAILED, "Invalid or expired pairing code")
        }
        Err(e) => {
            warn!(error = %e, "Failed to cancel pairing");
            JsonRpcResponse::error(
                request.id,
                -32603,
                format!("Failed to cancel pairing: {}", e),
            )
        }
    }
}

/// Handle "pairing.list" request
pub async fn handle_pairing_list(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    let pending = match ctx.pairing_manager.list_pending() {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "Failed to list pending pairings");
            return JsonRpcResponse::error(
                request.id,
                -32603,
                format!("Failed to list pending pairings: {}", e),
            );
        }
    };

    let items: Vec<Value> = pending
        .into_iter()
        .map(|req| match req {
            PairingRequest::Device {
                code,
                device_name,
                device_type,
                expires_at,
                created_at,
                ..
            } => {
                let remaining = if expires_at > created_at {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as i64;
                    if expires_at > now {
                        ((expires_at - now) / 1000) as u64
                    } else {
                        0
                    }
                } else {
                    0
                };
                json!({
                    "type": "device",
                    "code": code,
                    "device_name": device_name,
                    "device_type": device_type.map(|t: DeviceType| t.as_str()),
                    "expires_in": remaining,
                })
            }
            PairingRequest::Channel {
                code,
                channel,
                sender_id,
                expires_at,
                created_at,
                ..
            } => {
                let remaining = if expires_at > created_at {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as i64;
                    if expires_at > now {
                        ((expires_at - now) / 1000) as u64
                    } else {
                        0
                    }
                } else {
                    0
                };
                json!({
                    "type": "channel",
                    "code": code,
                    "channel": channel,
                    "sender_id": sender_id,
                    "expires_in": remaining,
                })
            }
            PairingRequest::Browser {
                code,
                origin_label,
                user_agent,
                peer_ip,
                expires_at,
                created_at,
                ..
            } => {
                let remaining = if expires_at > created_at {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as i64;
                    if expires_at > now {
                        ((expires_at - now) / 1000) as u64
                    } else {
                        0
                    }
                } else {
                    0
                };
                json!({
                    "type": "browser",
                    "code": code,
                    "origin_label": origin_label,
                    "user_agent": user_agent,
                    "peer_ip": peer_ip,
                    "expires_in": remaining,
                })
            }
            PairingRequest::Node {
                code,
                node_name,
                expires_at,
                created_at,
                ..
            } => {
                let remaining = if expires_at > created_at {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as i64;
                    if expires_at > now {
                        ((expires_at - now) / 1000) as u64
                    } else {
                        0
                    }
                } else {
                    0
                };
                json!({
                    "type": "node",
                    "code": code,
                    "node_name": node_name,
                    "expires_in": remaining,
                })
            }
        })
        .collect();

    JsonRpcResponse::success(request.id, json!({"pending": items}))
}

/// Handle "pairing.start_browser" — anonymous RPC from the cold `/pair`
/// HTML page. Creates a Browser pairing record and emits
/// `pairing.requested` so the desktop Panel can notify the operator.
///
/// This endpoint is reachable without authentication (registered in the
/// loopback allow-list at `server::handler::allow_unauth_loopback_wizard`-
/// adjacent block) precisely because the browser making the call has no
/// token yet — that's the whole point of the flow.
pub async fn handle_pairing_start_browser(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    #[derive(Debug, Deserialize)]
    struct StartBrowserParams {
        #[serde(default)]
        origin_label: String,
        #[serde(default)]
        user_agent: String,
        #[serde(default)]
        peer_ip: String,
    }

    let params: StartBrowserParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let origin = if params.origin_label.is_empty() {
        "Browser on this network".to_string()
    } else {
        params.origin_label.clone()
    };

    let (code, expires_at) = match ctx.pairing_manager.create_browser_pairing(
        &origin,
        &params.user_agent,
        &params.peer_ip,
    ) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "Failed to create browser pairing");
            return JsonRpcResponse::error(
                request.id,
                -32603,
                format!("Failed to create browser pairing: {}", e),
            );
        }
    };

    // Light up the PairingRequested event — the Panel subscribes via
    // `events.subscribe` on `pairing.**` and pops a notification card
    // with Approve/Reject buttons.
    if let Err(e) = ctx
        .event_bus
        .publish_frame(&GatewayEventFrame::PairingRequested {
            device_name: origin.clone(),
        })
    {
        warn!(error = %e, "Failed to publish PairingRequested frame");
    }

    // Compute remaining secs from the wall-clock now so the client knows
    // how long it has before the code self-destructs.
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let expires_in_secs = if expires_at > now_ms {
        ((expires_at - now_ms) / 1000) as u64
    } else {
        0
    };

    info!(code = %code, origin = %origin, "Browser pairing started");
    JsonRpcResponse::success(
        request.id,
        json!({
            "code": code,
            "expires_at": expires_at,
            "expires_in_secs": expires_in_secs,
        }),
    )
}

/// Handle "pairing.poll" — anonymous polling RPC the `/pair` HTML page
/// hits every ~2s. Returns one of:
/// - `{"status": "pending"}` while the operator hasn't decided yet
/// - `{"status": "approved", "token": "…", "device_id": "…"}` once approved —
///   the chat-tier device token the browser stores in
///   `localStorage["aleph_device_token"]` and connects with (single-use: a
///   second poll returns "expired")
/// - `{"status": "rejected"}` if the operator rejected
/// - `{"status": "expired"}` if unknown, never created, or TTL elapsed
pub async fn handle_pairing_poll(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    #[derive(Debug, Deserialize)]
    struct PollParams {
        code: String,
    }

    let params: PollParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match ctx.pairing_manager.poll_browser_pairing(&params.code) {
        PollState::Pending => JsonRpcResponse::success(request.id, json!({"status": "pending"})),
        PollState::Approved { token, device_id } => JsonRpcResponse::success(
            request.id,
            json!({"status": "approved", "token": token, "device_id": device_id}),
        ),
        PollState::Rejected => JsonRpcResponse::success(request.id, json!({"status": "rejected"})),
        PollState::Expired => JsonRpcResponse::success(request.id, json!({"status": "expired"})),
    }
}

#[cfg(test)]
mod tier_param_tests {
    use super::*;

    #[test]
    fn approve_params_defaults_level_to_none() {
        let req = serde_json::json!({ "code": "ABCD1234" });
        let p: ApproveParams = serde_json::from_value(req).unwrap();
        assert_eq!(p.code, "ABCD1234");
        assert_eq!(p.level, None);
    }

    #[test]
    fn approve_params_parses_config_level() {
        let req = serde_json::json!({ "code": "X", "level": "config" });
        let p: ApproveParams = serde_json::from_value(req).unwrap();
        assert_eq!(p.level.as_deref(), Some("config"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::handlers::auth::connect::handle_connect;

    #[tokio::test]
    async fn test_pairing_flow() {
        let ctx = super::super::tests::create_test_context();

        // Step 1: Try to connect (should get pairing code)
        let connect_req = JsonRpcRequest::new(
            "connect",
            Some(json!({"device_name": "Test Device"})),
            Some(json!(1)),
        );
        let connect_resp = handle_connect(connect_req, ctx.clone()).await;
        let code = connect_resp
            .error
            .unwrap()
            .data
            .unwrap()
            .get("code")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();

        // Step 2: Approve pairing
        let approve_req = JsonRpcRequest::new(
            "pairing.approve",
            Some(json!({"code": code})),
            Some(json!(2)),
        );
        let approve_resp = handle_pairing_approve(approve_req, ctx.clone()).await;
        assert!(approve_resp.is_success());

        let result = approve_resp.result.unwrap();
        let device_id = result.get("device_id").unwrap().as_str().unwrap();

        // Step 3: Connect with approved device_id
        let reconnect_req = JsonRpcRequest::new(
            "connect",
            Some(json!({"device_id": device_id})),
            Some(json!(3)),
        );
        let reconnect_resp = handle_connect(reconnect_req, ctx).await;
        assert!(reconnect_resp.is_success());
    }

    #[tokio::test]
    async fn browser_pairing_mints_chat_tier_device_and_connects_as_guest() {
        let ctx = super::super::tests::create_test_context();

        let start = handle_pairing_start_browser(
            JsonRpcRequest::new(
                "pairing.start_browser",
                Some(json!({
                    "origin_label": "Safari on 192.168.1.9",
                    "user_agent": "Mozilla/5.0",
                    "peer_ip": "192.168.1.9"
                })),
                Some(json!(1)),
            ),
            ctx.clone(),
        )
        .await;
        let code = start
            .result
            .unwrap()
            .get("code")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();

        let approve = handle_pairing_approve(
            JsonRpcRequest::new(
                "pairing.approve",
                Some(json!({ "code": code.clone() })),
                Some(json!(2)),
            ),
            ctx.clone(),
        )
        .await;
        assert!(approve.is_success(), "approve failed: {:?}", approve);

        let poll = handle_pairing_poll(
            JsonRpcRequest::new(
                "pairing.poll",
                Some(json!({ "code": code })),
                Some(json!(3)),
            ),
            ctx.clone(),
        )
        .await;
        let body = poll.result.unwrap();
        assert_eq!(body.get("status").unwrap().as_str().unwrap(), "approved");
        let token = body
            .get("token")
            .and_then(|v| v.as_str())
            .expect("approved poll must include a device token")
            .to_string();
        let device_id = body
            .get("device_id")
            .and_then(|v| v.as_str())
            .expect("approved poll must include device_id")
            .to_string();
        assert!(
            body.get("session_id").is_none(),
            "browser pairing no longer returns session_id"
        );

        assert!(ctx.device_store.is_approved(&device_id));
        let device = ctx.device_store.get_device(&device_id).unwrap();
        assert_eq!(
            device.permissions,
            vec!["chat".to_string(), "read".to_string()]
        );

        let connect = handle_connect(
            JsonRpcRequest::new(
                "connect",
                Some(json!({ "token": token, "device_name": "Web Panel" })),
                Some(json!(4)),
            ),
            ctx,
        )
        .await;
        assert!(connect.is_success(), "connect failed: {:?}", connect);
        let cr = connect.result.unwrap();
        assert_eq!(cr.get("role").unwrap().as_str().unwrap(), "guest");
        let perms: Vec<String> = cr
            .get("permissions")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert!(
            !perms.iter().any(|p| p == "*"),
            "chat tier must not hold wildcard"
        );
    }

    #[tokio::test]
    async fn start_browser_then_poll_pending() {
        let ctx = super::super::tests::create_test_context();

        let start = handle_pairing_start_browser(
            JsonRpcRequest::new(
                "pairing.start_browser",
                Some(json!({
                    "origin_label": "Safari on 192.168.1.5",
                    "user_agent": "Mozilla/5.0",
                    "peer_ip": "192.168.1.5"
                })),
                Some(json!(1)),
            ),
            ctx.clone(),
        )
        .await;
        assert!(
            start.is_success(),
            "start_browser must succeed: {:?}",
            start
        );
        let code = start
            .result
            .as_ref()
            .unwrap()
            .get("code")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        assert_eq!(code.len(), 6);

        let poll = handle_pairing_poll(
            JsonRpcRequest::new(
                "pairing.poll",
                Some(json!({ "code": code })),
                Some(json!(2)),
            ),
            ctx,
        )
        .await;
        let status = poll
            .result
            .unwrap()
            .get("status")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(status, "pending");
    }

    #[tokio::test]
    async fn approve_browser_pairing_makes_poll_return_approved() {
        let ctx = super::super::tests::create_test_context();

        let start = handle_pairing_start_browser(
            JsonRpcRequest::new(
                "pairing.start_browser",
                Some(json!({
                    "origin_label": "Firefox on 192.168.1.5",
                    "user_agent": "ua",
                    "peer_ip": "192.168.1.5"
                })),
                Some(json!(1)),
            ),
            ctx.clone(),
        )
        .await;
        let code = start
            .result
            .unwrap()
            .get("code")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();

        let approve = handle_pairing_approve(
            JsonRpcRequest::new(
                "pairing.approve",
                Some(json!({ "code": code.clone() })),
                Some(json!(2)),
            ),
            ctx.clone(),
        )
        .await;
        assert!(
            approve.is_success(),
            "approve must succeed for Browser variant: {:?}",
            approve
        );

        let poll = handle_pairing_poll(
            JsonRpcRequest::new(
                "pairing.poll",
                Some(json!({ "code": code })),
                Some(json!(3)),
            ),
            ctx,
        )
        .await;
        let body = poll.result.unwrap();
        assert_eq!(body.get("status").unwrap().as_str().unwrap(), "approved");
        assert!(
            body.get("token").is_some(),
            "approved poll must include token"
        );
        assert!(
            body.get("device_id").is_some(),
            "approved poll must include device_id"
        );
    }

    #[tokio::test]
    async fn node_pairing_mints_node_role_token_single_use() {
        use crate::gateway::security::PollState;
        let ctx = super::super::tests::create_test_context();

        // Create a node pairing record directly (start_node handler lands in a
        // later task; this exercises approve + poll + token minting).
        let (code, _expires) = ctx
            .pairing_manager
            .request_node_pairing("worker-1")
            .expect("create node pairing");

        let approve = handle_pairing_approve(
            JsonRpcRequest::new(
                "pairing.approve",
                Some(json!({ "code": code.clone() })),
                Some(json!(2)),
            ),
            ctx.clone(),
        )
        .await;
        assert!(
            approve.is_success(),
            "approve should succeed: {:?}",
            approve.error
        );
        assert_eq!(approve.result.unwrap().get("kind").unwrap(), "node");

        // Poll drains the credential single-use; token validates as Node role.
        match ctx.pairing_manager.poll_browser_pairing(&code) {
            PollState::Approved { token, .. } => {
                let (tok, sig) = token.split_once(':').expect("token:signature");
                let v = ctx.token_manager.validate_token(tok, sig).unwrap();
                assert_eq!(v.role, DeviceRole::Node);
            }
            other => panic!("expected Approved, got {other:?}"),
        }
        assert_eq!(
            ctx.pairing_manager.poll_browser_pairing(&code),
            PollState::Expired,
            "second poll is single-use Expired"
        );
    }

    #[tokio::test]
    async fn reject_browser_pairing_makes_poll_return_rejected() {
        let ctx = super::super::tests::create_test_context();

        let start = handle_pairing_start_browser(
            JsonRpcRequest::new(
                "pairing.start_browser",
                Some(json!({
                    "origin_label": "Chrome",
                    "user_agent": "ua",
                    "peer_ip": "192.168.1.5"
                })),
                Some(json!(1)),
            ),
            ctx.clone(),
        )
        .await;
        let code = start
            .result
            .unwrap()
            .get("code")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();

        let reject = handle_pairing_reject(
            JsonRpcRequest::new(
                "pairing.reject",
                Some(json!({ "code": code.clone() })),
                Some(json!(2)),
            ),
            ctx.clone(),
        )
        .await;
        assert!(reject.is_success(), "reject failed: {:?}", reject);

        let poll = handle_pairing_poll(
            JsonRpcRequest::new(
                "pairing.poll",
                Some(json!({ "code": code })),
                Some(json!(3)),
            ),
            ctx,
        )
        .await;
        assert_eq!(
            poll.result
                .unwrap()
                .get("status")
                .unwrap()
                .as_str()
                .unwrap(),
            "rejected"
        );
    }
}
