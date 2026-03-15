//! Tests for provider handlers.

use super::*;
use crate::config::{Config, ProviderConfig};
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;
use serde_json::json;
use crate::gateway::protocol::JsonRpcRequest;
use crate::gateway::security::SharedTokenManager;

#[test]
fn test_update_params() {
    let json = json!({
        "name": "openai",
        "config": {
            "enabled": true,
            "model": "gpt-4"
        }
    });
    let params: UpdateParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.name, "openai");
    assert_eq!(params.config.models, vec!["gpt-4"]);
}

#[test]
fn test_test_result_serialize() {
    let result = TestResult {
        success: true,
        error: None,
        latency_ms: Some(150),
    };
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["success"], true);
    assert_eq!(json["latency_ms"], 150);
}

#[tokio::test]
async fn test_needs_setup_empty_providers() {
    let config = Arc::new(RwLock::new(Config::default()));
    let request = JsonRpcRequest::with_id("providers.needsSetup", None, serde_json::json!(1));
    let response = handle_needs_setup(request, config).await;
    let result: serde_json::Value = serde_json::from_value(response.result.unwrap()).unwrap();
    assert_eq!(result["needs_setup"], true);
    assert_eq!(result["provider_count"], 0);
    assert_eq!(result["has_verified"], false);
}

#[tokio::test]
async fn test_needs_setup_has_verified_provider() {
    let mut config = Config::default();
    let mut provider_cfg = ProviderConfig::test_config("gpt-4o");
    provider_cfg.enabled = true;
    provider_cfg.verified = true;
    config.providers.insert("openai".to_string(), provider_cfg);
    let config = Arc::new(RwLock::new(config));
    let request = JsonRpcRequest::with_id("providers.needsSetup", None, serde_json::json!(1));
    let response = handle_needs_setup(request, config).await;
    let result: serde_json::Value = serde_json::from_value(response.result.unwrap()).unwrap();
    assert_eq!(result["needs_setup"], false);
    assert_eq!(result["provider_count"], 1);
    assert_eq!(result["has_verified"], true);
}

#[tokio::test]
async fn test_needs_setup_has_unverified_provider() {
    let mut config = Config::default();
    let mut provider_cfg = ProviderConfig::test_config("gpt-4o");
    provider_cfg.enabled = true;
    provider_cfg.verified = false;
    config.providers.insert("openai".to_string(), provider_cfg);
    let config = Arc::new(RwLock::new(config));
    let request = JsonRpcRequest::with_id("providers.needsSetup", None, serde_json::json!(1));
    let response = handle_needs_setup(request, config).await;
    let result: serde_json::Value = serde_json::from_value(response.result.unwrap()).unwrap();
    assert_eq!(result["needs_setup"], true);
    assert_eq!(result["provider_count"], 1);
    assert_eq!(result["has_verified"], false);
}
