//! Robustness tests for provider RPC endpoints.
//!
//! Tests concurrent access and large payloads to verify the server
//! handles edge cases gracefully.

use serde_json::json;
use serial_test::serial;

use super::harness::get_server;

#[tokio::test]
#[serial]
async fn concurrent_providers_list() {
    let server = get_server().await;

    // Fire 10 concurrent providers.list requests. `rpc_call` opens a fresh
    // WS connection per call (including the LAN-trust `connect` handshake),
    // so each task exercises its own concurrent connection.
    let mut handles = Vec::new();
    for _ in 0..10 {
        let handle =
            tokio::spawn(async move { server.rpc_call("providers.list", json!({})).await });
        handles.push(handle);
    }

    // All should succeed
    for (i, handle) in handles.into_iter().enumerate() {
        let response = handle.await.expect("Task panicked");
        assert!(
            response.get("error").is_none(),
            "Concurrent request {} got error: {:?}",
            i,
            response
        );
        assert!(
            response["result"]["providers"].is_array(),
            "Concurrent request {} missing providers array: {:?}",
            i,
            response
        );
    }
}

#[tokio::test]
#[serial]
async fn provider_with_100_models() {
    let server = get_server().await;
    server.clean_providers().await;

    // Create a provider with 100 models
    let models: Vec<String> = (0..100).map(|i| format!("model-{:03}", i)).collect();

    server
        .rpc_ok(
            "providers.create",
            json!({
                "name": "probe-many-models",
                "config": {
                    "protocol": "openai",
                    "enabled": true,
                    "models": models,
                    "base_url": "http://localhost:9999/v1"
                }
            }),
        )
        .await;

    // Get it back and verify all 100 models are there
    let result = server
        .rpc_ok("providers.get", json!({ "name": "probe-many-models" }))
        .await;

    let stored_models = result["provider"]["models"]
        .as_array()
        .expect("models should be array");
    assert_eq!(
        stored_models.len(),
        100,
        "Expected 100 models, got {}",
        stored_models.len()
    );

    // Spot-check a few
    assert!(stored_models.contains(&json!("model-000")));
    assert!(stored_models.contains(&json!("model-050")));
    assert!(stored_models.contains(&json!("model-099")));

    // Cleanup
    let _ = server
        .rpc_call("providers.delete", json!({ "name": "probe-many-models" }))
        .await;
}
