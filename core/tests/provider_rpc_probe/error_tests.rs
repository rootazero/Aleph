//! Error path tests for provider RPC endpoints.
//!
//! Validates that the server returns proper JSON-RPC errors for invalid
//! operations like updating/deleting nonexistent providers or creating duplicates.

use serde_json::json;
use serial_test::serial;

use super::harness::get_server;

#[tokio::test]
#[serial]
async fn update_nonexistent_provider_returns_error() {
    let server = get_server().await;
    server.clean_providers().await;

    let err = server
        .rpc_err(
            "providers.update",
            json!({
                "name": "does-not-exist",
                "config": {
                    "protocol": "openai",
                    "enabled": true,
                    "models": ["x"],
                    "base_url": "http://localhost:9999/v1"
                }
            }),
        )
        .await;

    let message = err["message"].as_str().unwrap_or("");
    assert!(
        message.contains("not found"),
        "Expected 'not found' in error message, got: {}",
        message
    );
}

#[tokio::test]
#[serial]
async fn delete_nonexistent_provider_returns_error() {
    let server = get_server().await;
    server.clean_providers().await;

    let err = server
        .rpc_err("providers.delete", json!({ "name": "does-not-exist" }))
        .await;

    let message = err["message"].as_str().unwrap_or("");
    assert!(
        message.contains("not found"),
        "Expected 'not found' in error message, got: {}",
        message
    );
}

#[tokio::test]
#[serial]
async fn create_duplicate_provider_returns_error() {
    let server = get_server().await;
    server.clean_providers().await;

    // Create a provider
    server
        .rpc_ok(
            "providers.create",
            json!({
                "name": "probe-dup",
                "config": {
                    "protocol": "openai",
                    "enabled": true,
                    "models": ["m1"],
                    "base_url": "http://localhost:9999/v1"
                }
            }),
        )
        .await;

    // Try to create the same name again
    let err = server
        .rpc_err(
            "providers.create",
            json!({
                "name": "probe-dup",
                "config": {
                    "protocol": "openai",
                    "enabled": true,
                    "models": ["m2"],
                    "base_url": "http://localhost:9999/v1"
                }
            }),
        )
        .await;

    let message = err["message"].as_str().unwrap_or("");
    assert!(
        message.contains("already exists"),
        "Expected 'already exists' in error message, got: {}",
        message
    );

    // Cleanup
    let _ = server
        .rpc_call("providers.delete", json!({ "name": "probe-dup" }))
        .await;
}

#[tokio::test]
#[serial]
async fn test_provider_with_unreachable_url_returns_failure() {
    let server = get_server().await;

    // Use localhost on a port that's definitely not listening (connection refused = fast failure)
    let response = server
        .rpc_call(
            "providers.test",
            json!({
                "config": {
                    "protocol": "openai",
                    "enabled": true,
                    "models": ["test-model"],
                    "base_url": "http://127.0.0.1:1/v1",
                    "api_key": "fake-key"
                }
            }),
        )
        .await;

    // providers.test may return either:
    // - RPC success with { success: false, error: "..." } in result
    // - RPC error if the handler itself fails
    if let Some(result) = response.get("result") {
        assert_eq!(
            result["success"], false,
            "Expected success: false for unreachable provider, got: {:?}",
            result
        );
    } else {
        // An RPC-level error is also acceptable
        assert!(
            response.get("error").is_some(),
            "Expected either result.success=false or error, got: {:?}",
            response
        );
    }
}
