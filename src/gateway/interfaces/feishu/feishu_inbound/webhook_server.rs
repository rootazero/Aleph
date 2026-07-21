use crate::sync_primitives::{Arc, Mutex as StdMutex};

use axum::{
    body::Bytes, extract::State, http::HeaderMap, http::StatusCode, response::Json, routing::post,
    Router,
};

use crate::gateway::channel::{ChannelId, InboundMessageSender};
use crate::gateway::interfaces::feishu::api::FeishuApi;
use crate::gateway::interfaces::feishu::config::FeishuConfig;
use crate::gateway::interfaces::feishu::feishu_inbound::crypto;
use crate::gateway::interfaces::feishu::feishu_inbound::events::parse_ws_frame;
use crate::gateway::interfaces::feishu::feishu_inbound::{
    map_event_to_inbound, InboundPolicy, MessageDedup, UserProfileCache,
};

#[derive(Clone)]
pub struct WebhookState {
    pub config: FeishuConfig,
    pub channel_id: ChannelId,
    pub bot_open_id: String,
    pub sender: InboundMessageSender,
    pub api: Arc<FeishuApi>,
    pub user_cache: Arc<UserProfileCache>,
    pub dedup: Arc<StdMutex<MessageDedup>>,
    pub policy: Arc<InboundPolicy>,
}

pub async fn run_webhook_server(state: WebhookState) {
    let addr = format!(
        "{}:{}",
        state.config.webhook_host, state.config.webhook_port
    );
    let app = Router::new()
        .route(&state.config.webhook_path, post(handle_webhook))
        .with_state(state);

    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("Feishu webhook server failed to bind {}: {e}", addr);
            return;
        }
    };
    tracing::info!("Feishu webhook server listening on {}", addr);
    if let Err(e) = axum::serve(listener, app).await {
        tracing::error!("Feishu webhook server error: {e}");
    }
}

async fn handle_webhook(
    State(state): State<WebhookState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let body_str = String::from_utf8_lossy(&body);

    // Resolve the real event payload: when an Encrypt Key is configured Feishu
    // AES-encrypts the body and signs it, so we must verify + decrypt before parsing.
    let payload = resolve_payload(&state, &headers, &body_str)?;
    if !verify_payload_token(&state, &payload) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    // URL-verification handshake — the challenge may live inside the decrypted payload.
    if let Ok(challenge_json) = serde_json::from_str::<serde_json::Value>(&payload) {
        if let Some(challenge) = challenge_json.get("challenge").and_then(|v| v.as_str()) {
            return Ok(Json(serde_json::json!({ "challenge": challenge })));
        }
    }

    match parse_ws_frame(&payload) {
        Ok(Some(event)) => {
            if let Some(inbound) = map_event_to_inbound(
                &event,
                &state.channel_id,
                &state.config,
                &state.bot_open_id,
                &state.user_cache,
                &state.api,
            ) {
                let mut dedup = state.dedup.lock().unwrap_or_else(|e| e.into_inner());
                if !dedup.is_duplicate(inbound.id.as_str()) {
                    drop(dedup);
                    match state.policy.evaluate(&inbound) {
                        crate::gateway::interfaces::feishu::feishu_inbound::InboundPolicyResult::Accept => {
                            let _ = state.sender.send(inbound);
                        }
                        crate::gateway::interfaces::feishu::feishu_inbound::InboundPolicyResult::Block(reason) => {
                            tracing::debug!(reason, "Inbound webhook blocked by policy");
                        }
                    }
                }
            }
        }
        Ok(None) => {}
        Err(e) => tracing::debug!("Failed to parse webhook body: {e}"),
    }

    Ok(Json(serde_json::json!({ "status": "ok" })))
}

/// Resolve the effective event payload string from a raw webhook body.
///
/// If an Encrypt Key is configured and the body carries an `encrypt` field, the
/// signature (when the signing headers are present) is verified and the payload is
/// AES-decrypted. Otherwise the raw body is returned unchanged (plaintext mode).
fn resolve_payload(
    state: &WebhookState,
    headers: &HeaderMap,
    body_str: &str,
) -> Result<String, StatusCode> {
    let encrypt_field = serde_json::from_str::<serde_json::Value>(body_str)
        .ok()
        .and_then(|v| {
            v.get("encrypt")
                .and_then(|e| e.as_str())
                .map(str::to_string)
        });

    match (state.config.encrypt_key.as_ref(), encrypt_field) {
        (Some(key), Some(encrypted)) => {
            let (Some(ts), Some(nonce), Some(sig)) = (
                header_str(headers, "x-lark-request-timestamp"),
                header_str(headers, "x-lark-request-nonce"),
                header_str(headers, "x-lark-signature"),
            ) else {
                return Err(StatusCode::UNAUTHORIZED);
            };
            if !timestamp_is_fresh(&ts) || !crypto::verify_signature(key, &ts, &nonce, body_str, &sig) {
                tracing::warn!("Feishu webhook signature mismatch; rejecting request");
                return Err(StatusCode::UNAUTHORIZED);
            }
            crypto::decrypt_event(key, &encrypted).map_err(|e| {
                tracing::warn!("Feishu webhook decrypt failed: {e}");
                StatusCode::BAD_REQUEST
            })
        }
        _ => Ok(body_str.to_string()),
    }
}

fn verify_payload_token(state: &WebhookState, payload: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return false;
    };
    let token = value
        .get("token")
        .or_else(|| value.pointer("/header/token"))
        .and_then(|v| v.as_str());
    match (state.config.verification_token.as_deref(), token) {
        (Some(expected), Some(actual)) => {
            crate::security::secret_equal_bytes(expected.as_bytes(), actual.as_bytes())
        }
        _ => false,
    }
}

fn timestamp_is_fresh(timestamp: &str) -> bool {
    let Ok(timestamp) = timestamp.parse::<u64>() else {
        return false;
    };
    let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) else {
        return false;
    };
    now.as_secs().abs_diff(timestamp) <= 300
}

fn header_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn make_state() -> WebhookState {
        WebhookState {
            config: FeishuConfig {
                app_id: "test".into(),
                app_secret: "secret".into(),
                domain: "feishu".into(),
                bot_name: None,
                dm_allowed: true,
                groups_allowed: true,
                group_policy: "open".into(),
                group_allowlist: vec![],
                require_mention: false,
                streaming: true,
                render_mode: "auto".into(),
                typing_indicator: true,
                reaction_notifications: true,
                group_session_scope:
                    crate::gateway::interfaces::feishu::config::GroupSessionScope::default(),
                connection_mode: "webhook".into(),
                webhook_port: 3000,
                webhook_host: "127.0.0.1".into(),
                webhook_path: "/feishu/events".into(),
                verification_token: Some("token".into()),
                encrypt_key: Some("key".into()),
                accounts: None,
                default_account: None,
            },
            channel_id: ChannelId::new("feishu"),
            bot_open_id: "bot".into(),
            sender: crate::gateway::channel::ChannelState::new(10).sender(),
            api: Arc::new(crate::gateway::interfaces::feishu::api::FeishuApi::new(
                Arc::new(crate::gateway::interfaces::feishu::auth::TokenManager::new(
                    "",
                    "",
                    "https://open.feishu.cn",
                    reqwest::Client::new(),
                )),
                "https://open.feishu.cn",
                reqwest::Client::new(),
            )),
            user_cache: Arc::new(UserProfileCache::new()),
            dedup: Arc::new(StdMutex::new(MessageDedup::new())),
            policy: Arc::new(InboundPolicy::new(
                FeishuConfig {
                    app_id: "test".into(),
                    app_secret: "secret".into(),
                    domain: "feishu".into(),
                    bot_name: None,
                    dm_allowed: true,
                    groups_allowed: true,
                    group_policy: "open".into(),
                    group_allowlist: vec![],
                    require_mention: false,
                    streaming: true,
                    render_mode: "auto".into(),
                    typing_indicator: true,
                    reaction_notifications: true,
                    group_session_scope:
                        crate::gateway::interfaces::feishu::config::GroupSessionScope::default(),
                    connection_mode: "webhook".into(),
                    webhook_port: 3000,
                    webhook_host: "127.0.0.1".into(),
                    webhook_path: "/feishu/events".into(),
                    verification_token: Some("token".into()),
                    encrypt_key: Some("key".into()),
                    accounts: None,
                    default_account: None,
                },
                "bot".into(),
            )),
        }
    }

    #[tokio::test]
    async fn test_challenge_response() {
        let state = make_state();
        let app = Router::new()
            .route("/feishu/events", post(handle_webhook))
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/feishu/events")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"challenge":"abc123","token":"token"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["challenge"].as_str(), Some("abc123"));
    }
}
