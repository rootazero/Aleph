//! Integration: BlueBubbles transport against a mock server (runs on any OS).
use alephcore::gateway::interfaces::imessage::bluebubbles::api::{BlueBubblesApi, LruGuidCache};
use axum::{routing::{get, post}, Json, Router};
use std::net::SocketAddr;

async fn spawn_mock() -> String {
    let app = Router::new()
        .route("/api/v1/ping", get(|| async { Json(serde_json::json!({"status":200})) }))
        .route("/api/v1/server/info", get(|| async {
            Json(serde_json::json!({"data":{"private_api":true,"helper_connected":true}})) }))
        .route("/api/v1/chat/query", post(|| async {
            Json(serde_json::json!({"data":[{"guid":"iMessage;-;+1","chatIdentifier":"+1"}]})) }))
        .route("/api/v1/message/text", post(|| async {
            Json(serde_json::json!({"data":{"guid":"sent-1"}})) }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
    format!("http://{addr}")
}

#[tokio::test]
async fn ping_and_caps_and_send_roundtrip() {
    let url = spawn_mock().await;
    let api = BlueBubblesApi::new(url, "pw".into());
    api.ping().await.expect("ping ok");
    let caps = api.server_caps().await;
    assert!(caps.private_api && caps.helper_connected);

    let cache = tokio::sync::Mutex::new(LruGuidCache::new(8));
    let guid = api.resolve_chat_guid("+1", &cache).await.expect("resolved");
    assert_eq!(guid, "iMessage;-;+1");

    let msg_guid = api.send_text_chunk(&guid, "hi", None, true).await.expect("sent");
    assert_eq!(msg_guid, "sent-1");
}
