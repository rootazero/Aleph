//! Spec C Task 22 (part 1): when the singleton lock is held, the
//! `secret set` CLI must route through the LockOrIpc arm and POST
//! to `/v1/admin/secrets`. We replace the real server with a tiny
//! axum mock so the test stays deterministic — the regression target
//! is `cli::policy::with_policy`'s LockOrIpc dispatch, not the full
//! gateway stack.
//!
//! Test design:
//!   1. Hold the singleton lock IN THIS test process via
//!      `instance_lock::try_acquire`. The CLI subprocess will see
//!      `HeldByLive` and fall to the IPC arm.
//!   2. Seed `security.db` with a bearer token so `forward_to_server`
//!      has something to read.
//!   3. Bind a mock axum server on an ephemeral port.
//!   4. Write `.ipc-endpoint.json` pointing at the mock.
//!   5. Spawn `aleph-server secret set FOO --value bar`.
//!   6. Assert the mock received exactly one POST with the expected
//!      JSON body, and the CLI exited 0.

use std::process::{Command, Stdio};
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};

use alephcore::cli::endpoint::{write_endpoint, IpcEndpoint};
use alephcore::gateway::security::store::SecurityStore;
use alephcore::utils::instance_lock::{self, AcquireOutcome};

#[derive(Clone)]
struct MockState {
    captured: Arc<tokio::sync::Mutex<Vec<serde_json::Value>>>,
}

async fn mock_handler(
    State(state): State<MockState>,
    Json(body): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    state.captured.lock().await.push(body.clone());
    let key = body
        .get("key")
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string();
    (StatusCode::OK, Json(serde_json::json!({ "key": key })))
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_secret_set_forwards_to_admin_endpoint_when_lock_held() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().to_path_buf();
    let data_dir = home.join(".aleph").join("data");
    std::fs::create_dir_all(&data_dir).expect("mkdir data dir");

    // Seed bearer token. forward_to_server will read it via
    // read_current_token_readonly and pass as Bearer auth.
    {
        let store = SecurityStore::open(data_dir.join("security.db")).expect("open security db");
        store
            .set_shared_token_with_secret("hash-1", &[1u8; 32], Some("test-bearer-token"))
            .expect("seed token");
    }

    // Hold the singleton lock so the CLI sees HeldByLive and takes
    // the IPC arm.
    let _lock = match instance_lock::try_acquire(&data_dir).expect("acquire lock") {
        AcquireOutcome::Acquired(g) => g,
        other => panic!("expected Acquired, got {other:?}"),
    };

    // Mock server.
    let captured = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let state = MockState {
        captured: captured.clone(),
    };
    let app = Router::new()
        .route("/v1/admin/secrets", post(mock_handler))
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

    let bin = env!("CARGO_BIN_EXE_aleph-server");
    let home_for_cli = home.clone();
    let out = tokio::task::spawn_blocking(move || {
        Command::new(bin)
            .args(["secret", "set", "FOO", "--value", "bar"])
            // `ALEPH_HOME` outranks the `HOME` fallback (see
            // `utils::paths::get_config_dir`), so set both. With only `HOME`,
            // this child resolves its state outside the tempdir on any machine
            // that has `ALEPH_HOME` set — and `ALEPH_HOME` is a product knob,
            // not a test-only switch.
            .env("HOME", &home_for_cli)
            .env("ALEPH_HOME", home_for_cli.join(".aleph"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("aleph secret set")
    })
    .await
    .expect("spawn_blocking join");

    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();

    assert!(
        out.status.success(),
        "secret set failed (status {:?}): stdout={stdout} stderr={stderr}",
        out.status.code()
    );
    assert!(
        stdout.contains("FOO"),
        "expected stdout to confirm FOO; got: {stdout}"
    );

    let captured = captured.lock().await;
    assert_eq!(
        captured.len(),
        1,
        "mock should have received exactly one call; got: {:?}",
        *captured
    );
    let body = &captured[0];
    assert_eq!(body.get("key").and_then(|v| v.as_str()), Some("FOO"));
    assert_eq!(body.get("value").and_then(|v| v.as_str()), Some("bar"));
}
