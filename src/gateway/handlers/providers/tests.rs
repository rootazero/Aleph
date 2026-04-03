//! Tests for provider handlers.

use super::*;
use crate::config::{Config, ProviderConfig};
use crate::gateway::protocol::JsonRpcRequest;
use crate::gateway::security::{SecurityStore, SharedTokenManager};
use crate::sync_primitives::Arc;
use serde_json::json;
use tokio::sync::RwLock;

fn test_vault() -> Arc<SharedTokenManager> {
    let store = Arc::new(SecurityStore::in_memory().unwrap());
    let tmp = std::env::temp_dir().join(format!(
        "test_ai_provider_vault_{}_{}.vault",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let vault = Arc::new(SharedTokenManager::new(store, tmp));
    let _ = vault.generate_token().unwrap();
    vault
}

fn config_with_provider(name: &str) -> Config {
    let mut config = Config::default();
    config
        .providers
        .insert(name.to_string(), ProviderConfig::test_config("gpt-4o"));
    config
}

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

#[tokio::test]
async fn test_handle_list_injects_api_key_from_vault() {
    let config = Arc::new(RwLock::new(config_with_provider("toapis")));
    let vault = test_vault();
    vault
        .store_secret("ai:toapis", "test-toapis-key")
        .expect("store ai provider secret");

    let request = JsonRpcRequest::with_id("providers.list", None, json!(1));
    let response = handle_list(request, config, vault).await;
    assert!(response.is_success());

    let result = response.result.unwrap();
    let providers = result["providers"].as_array().unwrap();
    assert_eq!(providers.len(), 1);
    let provider = &providers[0];
    assert_eq!(provider["name"], "toapis");
    assert_eq!(provider["api_key"], "test-toapis-key");
    assert_eq!(provider["has_api_key"], true);
}

#[tokio::test]
async fn test_handle_get_injects_api_key_from_vault() {
    let config = Arc::new(RwLock::new(config_with_provider("toapis")));
    let vault = test_vault();
    vault
        .store_secret("ai:toapis", "test-toapis-key")
        .expect("store ai provider secret");

    let request =
        JsonRpcRequest::with_id("providers.get", Some(json!({ "name": "toapis" })), json!(1));
    let response = handle_get(request, config, vault).await;
    assert!(response.is_success());

    let result = response.result.unwrap();
    let provider = &result["provider"];
    assert_eq!(provider["name"], "toapis");
    assert_eq!(provider["api_key"], "test-toapis-key");
    assert_eq!(provider["has_api_key"], true);
}
