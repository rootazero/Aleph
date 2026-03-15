//! RPC endpoint validation tests for provider configuration.
//!
//! Tests the providers.* JSON-RPC methods through a real WebSocket connection
//! to a running Aleph server.

use serde_json::json;
use serial_test::serial;

use super::harness::get_server;

#[tokio::test]
#[serial]
async fn websocket_connects_and_providers_list_works() {
    let server = get_server().await;
    let result = server.rpc_ok("providers.list", json!({})).await;

    // Result should contain a "providers" array
    assert!(
        result.get("providers").is_some(),
        "Missing 'providers' field in response: {:?}",
        result
    );
}

#[tokio::test]
#[serial]
async fn providers_list_returns_providers_array() {
    let server = get_server().await;
    server.clean_providers().await;
    let result = server.rpc_ok("providers.list", json!({})).await;

    let providers = result["providers"].as_array().expect("providers should be an array");
    // providers should be an array (already checked by .as_array() above)

    // Each provider should have a "models" field that is an array
    for provider in providers {
        assert!(
            provider["models"].is_array(),
            "Provider '{}' should have models array, got: {:?}",
            provider["name"],
            provider["models"]
        );
    }

    // Verify we can create and list a provider
    server
        .rpc_ok(
            "providers.create",
            json!({
                "name": "probe-list-check",
                "config": {
                    "protocol": "openai",
                    "enabled": true,
                    "models": ["list-model"],
                    "base_url": "http://localhost:9999/v1"
                }
            }),
        )
        .await;

    let result2 = server.rpc_ok("providers.list", json!({})).await;
    let providers2 = result2["providers"].as_array().expect("providers should be an array");
    assert!(
        providers2.iter().any(|p| p["name"] == "probe-list-check"),
        "Expected 'probe-list-check' in list after create, got: {:?}",
        providers2.iter().map(|p| p["name"].as_str()).collect::<Vec<_>>()
    );

    // Cleanup
    let _ = server
        .rpc_call("providers.delete", json!({ "name": "probe-list-check" }))
        .await;
}

#[tokio::test]
#[serial]
async fn providers_create_and_get() {
    let server = get_server().await;
    server.clean_providers().await;

    // Create a new provider with multiple models
    let create_result = server
        .rpc_ok(
            "providers.create",
            json!({
                "name": "probe-test-create",
                "config": {
                    "protocol": "openai",
                    "enabled": true,
                    "models": ["model-a", "model-b", "model-c"],
                    "base_url": "http://localhost:9999/v1"
                }
            }),
        )
        .await;
    assert_eq!(create_result["ok"], true);

    // Get it back
    let get_result = server
        .rpc_ok("providers.get", json!({ "name": "probe-test-create" }))
        .await;

    let provider = &get_result["provider"];
    assert_eq!(provider["name"], "probe-test-create");
    assert_eq!(provider["enabled"], true);

    let models = provider["models"].as_array().expect("models should be array");
    assert_eq!(models.len(), 3);
    assert!(models.contains(&json!("model-a")));
    assert!(models.contains(&json!("model-b")));
    assert!(models.contains(&json!("model-c")));

    // Cleanup
    let _ = server
        .rpc_call("providers.delete", json!({ "name": "probe-test-create" }))
        .await;
}

#[tokio::test]
#[serial]
async fn providers_update_models() {
    let server = get_server().await;
    server.clean_providers().await;

    // Create provider
    server
        .rpc_ok(
            "providers.create",
            json!({
                "name": "probe-test-update",
                "config": {
                    "protocol": "openai",
                    "enabled": true,
                    "models": ["old-model"],
                    "base_url": "http://localhost:9999/v1"
                }
            }),
        )
        .await;

    // Update with new models
    server
        .rpc_ok(
            "providers.update",
            json!({
                "name": "probe-test-update",
                "config": {
                    "protocol": "openai",
                    "enabled": true,
                    "models": ["new-model-1", "new-model-2"],
                    "base_url": "http://localhost:9999/v1"
                }
            }),
        )
        .await;

    // Verify update
    let get_result = server
        .rpc_ok("providers.get", json!({ "name": "probe-test-update" }))
        .await;

    let models = get_result["provider"]["models"]
        .as_array()
        .expect("models should be array");
    assert_eq!(models.len(), 2);
    assert!(models.contains(&json!("new-model-1")));
    assert!(models.contains(&json!("new-model-2")));
    assert!(!models.contains(&json!("old-model")));

    // Cleanup
    let _ = server
        .rpc_call("providers.delete", json!({ "name": "probe-test-update" }))
        .await;
}

#[tokio::test]
#[serial]
async fn providers_delete() {
    let server = get_server().await;
    server.clean_providers().await;

    // Create provider
    server
        .rpc_ok(
            "providers.create",
            json!({
                "name": "probe-test-delete",
                "config": {
                    "protocol": "openai",
                    "enabled": true,
                    "models": ["x"],
                    "base_url": "http://localhost:9999/v1"
                }
            }),
        )
        .await;

    // Delete it
    server
        .rpc_ok("providers.delete", json!({ "name": "probe-test-delete" }))
        .await;

    // Verify it's gone
    let err = server
        .rpc_err("providers.get", json!({ "name": "probe-test-delete" }))
        .await;
    let message = err["message"].as_str().unwrap_or("");
    assert!(
        message.contains("not found"),
        "Expected 'not found' error, got: {}",
        message
    );
}

#[tokio::test]
#[serial]
async fn unknown_method_returns_error() {
    let server = get_server().await;

    // models.list is not a registered method (it's only in the lane router as a query)
    let err = server.rpc_err("models.list", json!({})).await;
    assert!(
        err["code"].is_number(),
        "Error should have a numeric code: {:?}",
        err
    );
}

#[tokio::test]
#[serial]
async fn unknown_providers_method_returns_error() {
    let server = get_server().await;

    // providers.probe is not a registered method
    let err = server.rpc_err("providers.probe", json!({})).await;
    assert!(
        err["code"].is_number(),
        "Error should have a numeric code: {:?}",
        err
    );
}
