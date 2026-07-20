//! Inbound webhook listener. Mirrors feishu_inbound::webhook_server.

use crate::sync_primitives::{Arc, Mutex as StdMutex};
use axum::{
    body::Bytes,
    extract::{Query, State},
    http::StatusCode,
    response::Json,
    routing::post,
    Router,
};
use std::collections::HashMap;

use crate::gateway::channel::InboundMessageSender;
use crate::gateway::interfaces::imessage::bluebubbles::api::BlueBubblesApi;
use crate::gateway::interfaces::imessage::bluebubbles::inbound::dedup::BbDedup;
use crate::gateway::interfaces::imessage::bluebubbles::inbound::mapper::{
    map_webhook_record, to_inbound,
};

#[derive(Clone)]
pub struct WebhookState {
    pub password: String,
    pub sender: InboundMessageSender,
    pub api: Arc<BlueBubblesApi>,
    pub dedup: Arc<StdMutex<BbDedup>>,
    pub send_read_receipts: bool,
}

pub async fn run_webhook_server(state: WebhookState, host: String, port: u16, path: String) {
    let addr = format!("{host}:{port}");
    let app = Router::new()
        .route(&path, post(handle_webhook))
        .with_state(state);
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("BlueBubbles webhook bind {addr} failed: {e}");
            return;
        }
    };
    tracing::info!("BlueBubbles webhook listening on {addr}{path}");
    if let Err(e) = axum::serve(listener, app).await {
        tracing::error!("BlueBubbles webhook server error: {e}");
    }
}

pub async fn handle_webhook(
    State(state): State<WebhookState>,
    Query(q): Query<HashMap<String, String>>,
    body: Bytes,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if q.get("password").map(String::as_str) != Some(state.password.as_str()) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let payload: serde_json::Value =
        serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;

    if let Some(m) = map_webhook_record(&payload) {
        // Group mention gating is enforced downstream in the inbound router's
        // permission layer (single source of truth), so it applies uniformly to
        // both this webhook path and the catch-up poll path. Here we only drop
        // what can never be a routable inbound: our own echoes, remove-tapbacks,
        // and records with no GUID (add-tapbacks surface as reactions).
        if m.is_routable() {
            let dup = {
                state
                    .dedup
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_duplicate(&m.guid)
            };
            if !dup {
                let atts = super::download_attachments(&state.api, &m.attachment_guids).await;
                let inbound = to_inbound(&m, atts);
                let _ = state.sender.send(inbound);
                if state.send_read_receipts {
                    let api = state.api.clone();
                    let cg = m.chat_guid.clone();
                    tokio::spawn(async move {
                        if let Err(e) = api.mark_read(&cg).await {
                            tracing::debug!("mark_read {cg}: {e}");
                        }
                    });
                }
            }
        }
    }
    Ok(Json(serde_json::json!({ "status": "ok" })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request, routing::post, Router};
    use tower::ServiceExt;

    fn state(tx: crate::gateway::channel::InboundMessageSender) -> WebhookState {
        WebhookState {
            password: "secret".into(),
            sender: tx,
            api: std::sync::Arc::new(
                crate::gateway::interfaces::imessage::bluebubbles::api::BlueBubblesApi::new(
                    "http://x".into(),
                    "secret".into(),
                ),
            ),
            dedup: std::sync::Arc::new(std::sync::Mutex::new(BbDedup::new())),
            send_read_receipts: false,
        }
    }

    #[tokio::test]
    async fn rejects_bad_password() {
        let st = crate::gateway::channel::ChannelState::new(10);
        let app = Router::new()
            .route("/wh", post(handle_webhook))
            .with_state(state(st.sender()));
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/wh?password=wrong")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"type":"new-message","data":{}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn accepts_and_emits_inbound() {
        let st = crate::gateway::channel::ChannelState::new(10);
        let mut rx = st.inbound_subscribe();
        let app = Router::new()
            .route("/wh", post(handle_webhook))
            .with_state(state(st.sender()));
        let body = r#"{"type":"new-message","data":{"guid":"g1","text":"hi","isFromMe":false,"chatGuid":"iMessage;-;+1","handle":{"address":"+1"}}}"#;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/wh?password=secret")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let msg = rx.try_recv().expect("inbound emitted");
        assert_eq!(msg.text, "hi");
        assert_eq!(msg.sender_id.as_str(), "+1");
    }

    #[tokio::test]
    async fn add_tapback_emits_reaction_inbound() {
        let st = crate::gateway::channel::ChannelState::new(10);
        let mut rx = st.inbound_subscribe();
        let app = Router::new()
            .route("/wh", post(handle_webhook))
            .with_state(state(st.sender()));
        // An "add love" tapback against message g-target.
        let body = r#"{"type":"new-message","data":{"guid":"react-1","associatedMessageType":2000,"associatedMessageGuid":"g-target","isFromMe":false,"chatGuid":"iMessage;-;+1","handle":{"address":"+1"}}}"#;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/wh?password=secret")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let msg = rx.try_recv().expect("reaction inbound emitted");
        assert_eq!(msg.text, "Reacted with: ❤️");
        assert_eq!(msg.reply_to.as_ref().map(|r| r.as_str()), Some("g-target"));
    }

    #[tokio::test]
    async fn remove_tapback_emits_nothing() {
        let st = crate::gateway::channel::ChannelState::new(10);
        let mut rx = st.inbound_subscribe();
        let app = Router::new()
            .route("/wh", post(handle_webhook))
            .with_state(state(st.sender()));
        let body = r#"{"type":"new-message","data":{"guid":"unreact-1","associatedMessageType":3000,"associatedMessageGuid":"g-target","isFromMe":false,"chatGuid":"iMessage;-;+1","handle":{"address":"+1"}}}"#;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/wh?password=secret")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        assert!(rx.try_recv().is_err(), "remove tapback must not emit");
    }
}
