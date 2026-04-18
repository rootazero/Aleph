use crate::sync_primitives::Arc;
use std::sync::Mutex as StdMutex;

use axum::{body::Bytes, extract::State, http::StatusCode, response::Json, routing::post, Router};
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::gateway::channel::{ChannelId, InboundMessageSender};
use crate::gateway::interfaces::feishu::api::FeishuApi;
use crate::gateway::interfaces::feishu::config::FeishuConfig;
use crate::gateway::interfaces::feishu::feishu_inbound::events::parse_ws_frame;
use crate::gateway::interfaces::feishu::feishu_inbound::{
    map_event_to_inbound, InboundPolicy, MessageDedup, UserProfileCache,
};

type HmacSha256 = Hmac<Sha256>;

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

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    tracing::info!("Feishu webhook server listening on {}", addr);
    axum::serve(listener, app).await.unwrap();
}

async fn handle_webhook(
    State(state): State<WebhookState>,
    body: Bytes,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let body_str = String::from_utf8_lossy(&body);

    if let Ok(challenge_json) = serde_json::from_str::<serde_json::Value>(&body_str) {
        if let Some(challenge) = challenge_json.get("challenge").and_then(|v| v.as_str()) {
            return Ok(Json(serde_json::json!({ "challenge": challenge })));
        }
    }

    if let (Some(token), Some(key)) = (
        state.config.verification_token.as_ref(),
        state.config.encrypt_key.as_ref(),
    ) {
        let sign_payload = format!("{}{}", key, body_str);
        let mut mac = HmacSha256::new_from_slice(token.as_bytes())
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        mac.update(sign_payload.as_bytes());
        let _expected = hex::encode(mac.finalize().into_bytes());
    }

    match parse_ws_frame(&body_str) {
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
                    .body(Body::from(r#"{"challenge":"abc123"}"#))
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
