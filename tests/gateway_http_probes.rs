//! Integration coverage for the gateway HTTP probes (Spec 2 / G2).
//!
//! Unlike `src/gateway/server/probe.rs`'s unit tests, which call the
//! handlers directly, these spawn a real `GatewayServer` and hit the
//! routes over HTTP — verifying the routes are actually mounted in
//! `build_router`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use alephcore::gateway::server::GatewayConfig;
use alephcore::gateway::GatewayServer;
use reqwest::StatusCode;

/// Spawn a `GatewayServer` on `port`. Returns the shared ready-flag and
/// an `Arc` guard that keeps the server alive for the test's duration.
/// Polls `/health` until the server answers so the test does not race
/// the TCP bind.
async fn spawn(port: u16) -> (Arc<AtomicBool>, Arc<GatewayServer>) {
    let addr = format!("127.0.0.1:{port}").parse().unwrap();
    let server = Arc::new(GatewayServer::with_config(addr, GatewayConfig::default()));
    let ready = server.ready.clone();

    let run_handle = server.clone();
    tokio::spawn(async move {
        let _ = run_handle.run().await;
    });

    let client = reqwest::Client::new();
    for _ in 0..60 {
        if client
            .get(format!("http://127.0.0.1:{port}/health"))
            .send()
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    (ready, server)
}

#[tokio::test]
async fn health_returns_200_with_identity_fields() {
    let (_ready, _guard) = spawn(18831).await;

    let resp = reqwest::get("http://127.0.0.1:18831/health")
        .await
        .expect("GET /health");
    assert_eq!(resp.status(), StatusCode::OK);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert!(body["instance_id"].is_string());
    assert!(body["version"].is_string());
    assert!(body["uptime_secs"].is_number());
    assert!(body["started_at_unix"].is_number());
}

#[tokio::test]
async fn ready_returns_503_before_flag_flip_then_200_after() {
    let (ready, _guard) = spawn(18832).await;

    // Before the flip: 503 SERVICE_UNAVAILABLE.
    let resp = reqwest::get("http://127.0.0.1:18832/ready")
        .await
        .expect("GET /ready (booting)");
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ready"], false);
    assert_eq!(body["phase"], "booting");

    // Simulate boot phase-2 completion.
    ready.store(true, Ordering::Release);

    // After the flip: 200 OK.
    let resp = reqwest::get("http://127.0.0.1:18832/ready")
        .await
        .expect("GET /ready (complete)");
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ready"], true);
    assert_eq!(body["phase"], "complete");
}
