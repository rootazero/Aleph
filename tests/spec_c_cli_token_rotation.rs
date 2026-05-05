//! Spec C Task 22 (part 2): when the bearer token rotates between
//! `forward_to_server`'s read and the HTTP send, the CLI must
//! transparently re-read the token once and retry. Regression target
//! is `forward_to_server`'s 401-self-heal arm.
//!
//! Test design:
//!   1. Seed `security.db` with `initial-token` via the production
//!      `SecurityStore::set_shared_token_with_secret` API.
//!   2. Spin up a tiny axum server on an ephemeral port that:
//!      - on call #1: rotates the DB to `rotated-token`, returns 401
//!      - on call #2: asserts the bearer header IS the rotated token,
//!        returns 200 with a JSON body
//!   3. Write `.ipc-endpoint.json` pointing at the mock.
//!   4. Drive `forward_to_server` directly (it's `pub`).
//!   5. Assert: result body parsed correctly, exactly TWO calls made,
//!      bearer headers were `initial-token` then `rotated-token`.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::Router;

use alephcore::cli::endpoint::{write_endpoint, IpcEndpoint};
use alephcore::cli::ipc_client::forward_to_server;
use alephcore::cli::policy::HttpMethod;
use alephcore::gateway::security::store::SecurityStore;

#[derive(Clone)]
struct MockState {
    counter: Arc<AtomicU32>,
    captured: Arc<tokio::sync::Mutex<Vec<String>>>,
    security_db: std::path::PathBuf,
}

async fn handler(State(state): State<MockState>, headers: HeaderMap) -> (StatusCode, &'static str) {
    let n = state.counter.fetch_add(1, Ordering::SeqCst);
    let auth = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_string();
    state.captured.lock().await.push(auth);

    if n == 0 {
        // Rotate the bearer token in the DB before returning 401, so
        // the CLI's re-read sees a different value and retries.
        let store = SecurityStore::open(&state.security_db).expect("reopen");
        store
            .set_shared_token_with_secret("hash-2", &[2u8; 32], "rotated-token")
            .expect("rotate");
        (StatusCode::UNAUTHORIZED, "rotated")
    } else {
        (StatusCode::OK, r#"{"key":"FOO"}"#)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn forward_retries_once_on_401_with_rotated_token() {
    let dir = tempfile::tempdir().expect("tempdir");
    let data_dir = dir.path().to_path_buf();
    let security_db = data_dir.join("security.db");

    {
        let store = SecurityStore::open(&security_db).expect("seed open");
        store
            .set_shared_token_with_secret("hash-1", &[1u8; 32], "initial-token")
            .expect("seed token");
    }

    let counter = Arc::new(AtomicU32::new(0));
    let captured = Arc::new(tokio::sync::Mutex::new(Vec::new()));

    let state = MockState {
        counter: counter.clone(),
        captured: captured.clone(),
        security_db: security_db.clone(),
    };
    let app = Router::new()
        .route("/v1/admin/secrets", post(handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    write_endpoint(&data_dir, &IpcEndpoint::current(format!("http://{addr}")))
        .expect("write endpoint");

    // forward_to_server uses reqwest::blocking; run in a blocking task
    // so we don't deadlock the runtime.
    let data_dir_for_call = data_dir.clone();
    let result: serde_json::Value = tokio::task::spawn_blocking(move || {
        forward_to_server::<serde_json::Value>(
            &data_dir_for_call,
            HttpMethod::Post,
            "/v1/admin/secrets",
            serde_json::json!({"key": "FOO", "value": "bar"}),
        )
    })
    .await
    .expect("spawn_blocking join")
    .expect("forward_to_server");

    assert_eq!(
        result.get("key").and_then(|v| v.as_str()),
        Some("FOO"),
        "expected key=FOO in response, got: {result}"
    );
    assert_eq!(
        counter.load(Ordering::SeqCst),
        2,
        "expected exactly two calls (1 fail + 1 retry)"
    );
    let auths = captured.lock().await;
    assert_eq!(auths.len(), 2, "captured auths: {:?}", *auths);
    assert_eq!(
        auths[0], "Bearer initial-token",
        "first call should use the seeded token"
    );
    assert_eq!(
        auths[1], "Bearer rotated-token",
        "retry should pick up the rotated token"
    );
}
