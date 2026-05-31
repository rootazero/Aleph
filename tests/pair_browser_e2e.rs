//! End-to-end browser-pairing flow.
//!
//! 1. GET  /pair                                  → HTML with polling JS
//! 2. POST /rpc { pairing.start_browser }         → { code }
//! 3. (out-of-band) pairing.approve(code) by an authenticated client
//! 4. POST /rpc { pairing.poll(code) }            → { status: "approved", session_id }
//! 5. GET  /auth/bootstrap/from_pairing?code=…    → 303 + Set-Cookie
//!
//! Mirrors `tests/bootstrap_loopback_gate.rs`'s pattern: spawn a real
//! axum server on an ephemeral loopback port and drive it with `reqwest`
//! so `ConnectInfo<SocketAddr>` extractors see real peers.

use std::net::SocketAddr;
use std::sync::Arc;

use alephcore::gateway::auth_middleware::{auth_routes, AuthState};
use alephcore::gateway::bootstrap::BootstrapNonceManager;
use alephcore::gateway::config::AuthMode;
use alephcore::gateway::handlers::auth::{handle_pairing_approve, AuthContext, TransportPolicy};
use alephcore::gateway::protocol::JsonRpcRequest;
use alephcore::gateway::security::{
    InvitationManager, PairingManager, SecurityStore, SharedTokenManager, TokenManager,
};
use alephcore::gateway::session::HttpSessionManager;
use serde_json::json;
use tempfile::tempdir;

/// Build a fully-wired (AuthContext, AuthState) pair backed by an
/// in-memory SQLite store. Mirrors the production wiring in
/// `src/bin/aleph-server/commands/start/builder/subsystems.rs`.
fn setup() -> (Arc<AuthContext>, Arc<AuthState>) {
    let dir = tempdir().expect("tempdir");
    let store = Arc::new(SecurityStore::open(&dir.path().join("sec.db")).expect("open store"));

    let shared_token_mgr = Arc::new(SharedTokenManager::new(
        store.clone(),
        dir.path().join("vault"),
    ));
    let _ = shared_token_mgr.generate_token().expect("generate token");

    let session_mgr = Arc::new(HttpSessionManager::new(store.clone(), 24));
    let pairing_manager = Arc::new(PairingManager::new(store.clone()));
    let bootstrap_mgr = Arc::new(BootstrapNonceManager::default());
    let event_bus = Arc::new(alephcore::gateway::event_bus::GatewayEventBus::new());
    let device_store =
        Arc::new(alephcore::gateway::device_store::DeviceStore::in_memory().unwrap());

    // Provision a placeholder device so the TokenManager's FK constraints
    // are satisfied if the test ever issues device tokens.
    store
        .upsert_device(&alephcore::gateway::security::store::DeviceUpsertData {
            device_id: "test-dev",
            device_name: "Test",
            device_type: None,
            public_key: &[1u8; 32],
            fingerprint: "fp",
            role: "operator",
            scopes: &[],
        })
        .unwrap();

    let auth_ctx = Arc::new(AuthContext {
        token_manager: Arc::new(TokenManager::new(store.clone())),
        pairing_manager: pairing_manager.clone(),
        device_store,
        security_store: store.clone(),
        invitation_manager: Arc::new(InvitationManager::new()),
        guest_session_manager: Arc::new(alephcore::gateway::security::GuestSessionManager::new()),
        event_bus,
        auth_mode: AuthMode::Token,
        shared_token_mgr: shared_token_mgr.clone(),
        state_versions: Arc::new(alephcore::gateway::state_version::StateVersionTracker::new()),
        transport_policy: TransportPolicy::defaults(),
        instance_id: "test-instance".to_string(),
        started_at_unix: 1_700_000_000,
        presence: Arc::new(alephcore::gateway::presence::PresenceTracker::new()),
        max_connections: 1000,
        challenge_manager: Arc::new(alephcore::gateway::challenge::ChallengeManager::new()),
        require_challenge: false,
        bootstrap_mgr: bootstrap_mgr.clone(),
        session_mgr: session_mgr.clone(),
        bind_port: 18790,
        allow_guest: false,
        enable_pairing: true,
    });

    let auth_state = Arc::new(AuthState {
        shared_token_mgr,
        session_mgr,
        auth_mode: AuthMode::Token,
        bootstrap_mgr,
        pairing_mgr: pairing_manager,
        auth_ctx: Some(auth_ctx.clone()),
    });

    (auth_ctx, auth_state)
}

/// Spawn the router on an ephemeral loopback port and return the address.
async fn spawn_server(state: Arc<AuthState>) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral");
    let addr = listener.local_addr().expect("local_addr");
    let app = auth_routes(state).into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    addr
}

#[tokio::test]
async fn pair_page_serves_html() {
    let (_ctx, state) = setup();
    let addr = spawn_server(state).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{addr}/pair"))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.text().await.expect("body");
    assert!(body.contains("Pair this browser"));
    assert!(body.contains("pairing.start_browser"));
    assert!(body.contains("pairing.poll"));
}

#[tokio::test]
async fn anonymous_rpc_starts_and_polls_browser_pairing() {
    let (_ctx, state) = setup();
    let addr = spawn_server(state).await;

    let client = reqwest::Client::new();

    // 1. start_browser
    let start = client
        .post(format!("http://{addr}/rpc"))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "pairing.start_browser",
            "params": {
                "origin_label": "Test browser",
                "user_agent": "rust-test",
                "peer_ip": "127.0.0.1"
            }
        }))
        .send()
        .await
        .expect("start");
    let body: serde_json::Value = start.json().await.expect("json");
    let code = body
        .get("result")
        .and_then(|r| r.get("code"))
        .and_then(|c| c.as_str())
        .expect("code")
        .to_string();
    assert_eq!(code.len(), 6);

    // 2. poll → pending
    let poll = client
        .post(format!("http://{addr}/rpc"))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "pairing.poll",
            "params": {"code": code}
        }))
        .send()
        .await
        .expect("poll");
    let body: serde_json::Value = poll.json().await.expect("json");
    assert_eq!(
        body.get("result")
            .and_then(|r| r.get("status"))
            .and_then(|s| s.as_str()),
        Some("pending"),
    );
}

#[tokio::test]
async fn other_rpc_methods_are_blocked_at_anonymous_endpoint() {
    let (_ctx, state) = setup();
    let addr = spawn_server(state).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{addr}/rpc"))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 99,
            "method": "memory.search",
            "params": {"query": "x"}
        }))
        .send()
        .await
        .expect("send");
    let body: serde_json::Value = resp.json().await.expect("json");
    // -32601 method not allowed via /rpc
    assert_eq!(
        body.get("error")
            .and_then(|e| e.get("code"))
            .and_then(|c| c.as_i64()),
        Some(-32601),
    );
}

#[tokio::test]
async fn bootstrap_from_pairing_sets_cookie_when_approved() {
    let (ctx, state) = setup();
    let addr = spawn_server(state).await;

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build client");

    // 1. start_browser → grab code
    let start = client
        .post(format!("http://{addr}/rpc"))
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "pairing.start_browser",
            "params": {
                "origin_label": "Test browser",
                "user_agent": "rust-test",
                "peer_ip": "127.0.0.1"
            }
        }))
        .send()
        .await
        .expect("start");
    let body: serde_json::Value = start.json().await.expect("json");
    let code = body
        .get("result")
        .and_then(|r| r.get("code"))
        .and_then(|c| c.as_str())
        .expect("code")
        .to_string();

    // 2. Approve out-of-band via the JSON-RPC handler (simulating the
    //    authenticated Panel clicking "Approve").
    let approve = handle_pairing_approve(
        JsonRpcRequest::new(
            "pairing.approve",
            Some(json!({"code": code.clone()})),
            Some(json!(7)),
        ),
        ctx,
    )
    .await;
    assert!(approve.is_success(), "approve failed: {:?}", approve);

    // 3. GET /auth/bootstrap/from_pairing?code=… should 303 + Set-Cookie
    let resp = client
        .get(format!(
            "http://{addr}/auth/bootstrap/from_pairing?code={code}"
        ))
        .send()
        .await
        .expect("send bootstrap");
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    let cookie = resp
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .expect("cookie");
    assert!(cookie.starts_with("aleph_session="));
    assert!(cookie.contains("HttpOnly"));

    // 4. Single-use: a second hit returns 401
    let second = client
        .get(format!(
            "http://{addr}/auth/bootstrap/from_pairing?code={code}"
        ))
        .send()
        .await
        .expect("send second");
    assert_eq!(second.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn bootstrap_from_pairing_refuses_unapproved_code() {
    let (_ctx, state) = setup();
    let addr = spawn_server(state).await;

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build client");
    let resp = client
        .get(format!(
            "http://{addr}/auth/bootstrap/from_pairing?code=999999"
        ))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
}
