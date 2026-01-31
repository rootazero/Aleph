//! Integration tests for models.* and chat.* RPC handlers
//!
//! These tests verify the handlers work correctly with real Config objects.
//!
//! Tests cover:
//! - models.list with empty config
//! - models.list with providers configured
//! - models.list with enabled_only filter
//! - models.get found case
//! - models.get not found case
//! - models.capabilities with enabled providers
//! - chat.* param deserialization

#![cfg(feature = "gateway")]

use aethecore::gateway::handlers::chat::{ClearParams, HistoryParams, SendParams};
use aethecore::gateway::handlers::models;
use aethecore::gateway::protocol::JsonRpcRequest;
use aethecore::{Config, ProviderConfig};
use serde_json::json;
use std::sync::Arc;

// ============================================================================
// models.list Tests
// ============================================================================

/// Test: models.list with empty config returns empty models array
#[tokio::test]
async fn test_models_list_empty_config() {
    let config = Arc::new(Config::default());
    let request = JsonRpcRequest::new("models.list", None, Some(json!(1)));

    let response = models::handle_list(request, config).await;

    assert!(response.result.is_some(), "Response should be successful");
    let result = response.result.unwrap();
    let models_array = result["models"].as_array().unwrap();
    assert!(models_array.is_empty(), "Models array should be empty");
}

/// Test: models.list with providers configured returns all models
#[tokio::test]
async fn test_models_list_with_providers() {
    let mut config = Config::default();

    // Add OpenAI provider
    config.providers.insert(
        "openai".to_string(),
        ProviderConfig::test_config("gpt-4o"),
    );

    // Add Claude provider
    config.providers.insert(
        "claude".to_string(),
        ProviderConfig::test_config("claude-3-5-sonnet-20241022"),
    );

    // Add Gemini provider
    config.providers.insert(
        "gemini".to_string(),
        ProviderConfig::test_config("gemini-2.0-flash"),
    );

    config.general.default_provider = Some("openai".to_string());

    let config = Arc::new(config);
    let request = JsonRpcRequest::new("models.list", None, Some(json!(1)));

    let response = models::handle_list(request, config).await;

    assert!(response.result.is_some());
    let result = response.result.unwrap();
    let models_array = result["models"].as_array().unwrap();
    assert_eq!(models_array.len(), 3, "Should have 3 models");

    // Verify model fields
    for model in models_array {
        assert!(model["id"].is_string());
        assert!(model["provider"].is_string());
        assert!(model["provider_type"].is_string());
        assert!(model["enabled"].is_boolean());
        assert!(model["is_default"].is_boolean());
        assert!(model["capabilities"].is_array());
    }

    // Verify default provider is marked
    let default_model = models_array
        .iter()
        .find(|m| m["is_default"].as_bool().unwrap())
        .expect("Should have a default model");
    assert_eq!(default_model["provider"], "openai");
}

/// Test: models.list with enabled_only filter
#[tokio::test]
async fn test_models_list_enabled_only_filter() {
    let mut config = Config::default();

    // Add enabled provider
    config.providers.insert(
        "openai".to_string(),
        ProviderConfig::test_config("gpt-4o"),
    );

    // Add disabled provider
    let mut disabled_config = ProviderConfig::test_config("claude-3-5-sonnet-20241022");
    disabled_config.enabled = false;
    config.providers.insert("claude".to_string(), disabled_config);

    let config = Arc::new(config);
    let request = JsonRpcRequest::new(
        "models.list",
        Some(json!({ "enabled_only": true })),
        Some(json!(1)),
    );

    let response = models::handle_list(request, config).await;

    assert!(response.result.is_some());
    let result = response.result.unwrap();
    let models_array = result["models"].as_array().unwrap();
    assert_eq!(models_array.len(), 1, "Should only have 1 enabled model");
    assert_eq!(models_array[0]["provider"], "openai");
}

/// Test: models.list with provider filter
#[tokio::test]
async fn test_models_list_provider_filter() {
    let mut config = Config::default();

    config.providers.insert(
        "openai".to_string(),
        ProviderConfig::test_config("gpt-4o"),
    );
    config.providers.insert(
        "claude".to_string(),
        ProviderConfig::test_config("claude-3-5-sonnet-20241022"),
    );

    let config = Arc::new(config);
    let request = JsonRpcRequest::new(
        "models.list",
        Some(json!({ "provider": "claude" })),
        Some(json!(1)),
    );

    let response = models::handle_list(request, config).await;

    assert!(response.result.is_some());
    let result = response.result.unwrap();
    let models_array = result["models"].as_array().unwrap();
    assert_eq!(models_array.len(), 1);
    assert_eq!(models_array[0]["provider"], "claude");
}

// ============================================================================
// models.get Tests
// ============================================================================

/// Test: models.get with existing provider returns model info
#[tokio::test]
async fn test_models_get_found() {
    let mut config = Config::default();

    config.providers.insert(
        "openai".to_string(),
        ProviderConfig::test_config("gpt-4o"),
    );
    config.general.default_provider = Some("openai".to_string());

    let config = Arc::new(config);
    let request = JsonRpcRequest::new(
        "models.get",
        Some(json!({ "provider": "openai" })),
        Some(json!(1)),
    );

    let response = models::handle_get(request, config).await;

    assert!(response.result.is_some());
    let result = response.result.unwrap();
    assert_eq!(result["model"]["id"], "gpt-4o");
    assert_eq!(result["model"]["provider"], "openai");
    assert_eq!(result["model"]["provider_type"], "openai");
    assert!(result["model"]["enabled"].as_bool().unwrap());
    assert!(result["model"]["is_default"].as_bool().unwrap());

    // Verify capabilities
    let caps = result["model"]["capabilities"].as_array().unwrap();
    assert!(caps.iter().any(|c| c == "chat"));
    assert!(caps.iter().any(|c| c == "vision"));
    assert!(caps.iter().any(|c| c == "tools"));
}

/// Test: models.get with non-existent provider returns error
#[tokio::test]
async fn test_models_get_not_found() {
    let config = Arc::new(Config::default());
    let request = JsonRpcRequest::new(
        "models.get",
        Some(json!({ "provider": "nonexistent" })),
        Some(json!(1)),
    );

    let response = models::handle_get(request, config).await;

    assert!(response.error.is_some());
    let error = response.error.unwrap();
    assert!(error.message.contains("not found"));
}

/// Test: models.get without required params returns error
#[tokio::test]
async fn test_models_get_missing_params() {
    let config = Arc::new(Config::default());
    let request = JsonRpcRequest::new("models.get", None, Some(json!(1)));

    let response = models::handle_get(request, config).await;

    assert!(response.error.is_some());
    let error = response.error.unwrap();
    assert!(error.message.contains("params"));
}

// ============================================================================
// models.capabilities Tests
// ============================================================================

/// Test: models.capabilities with enabled provider
#[tokio::test]
async fn test_models_capabilities_with_provider() {
    let mut config = Config::default();

    config.providers.insert(
        "claude".to_string(),
        ProviderConfig::test_config("claude-3-5-sonnet-20241022"),
    );

    let config = Arc::new(config);
    let request = JsonRpcRequest::new(
        "models.capabilities",
        Some(json!({ "provider": "claude" })),
        Some(json!(1)),
    );

    let response = models::handle_capabilities(request, config).await;

    assert!(response.result.is_some());
    let result = response.result.unwrap();
    let caps = result["capabilities"].as_array().unwrap();

    // Claude 3.5 Sonnet should have all capabilities
    assert!(caps.iter().any(|c| c == "chat"));
    assert!(caps.iter().any(|c| c == "vision"));
    assert!(caps.iter().any(|c| c == "tools"));
    assert!(caps.iter().any(|c| c == "thinking"));
}

/// Test: models.capabilities with Gemini model
#[tokio::test]
async fn test_models_capabilities_gemini() {
    let mut config = Config::default();

    config.providers.insert(
        "gemini".to_string(),
        ProviderConfig::test_config("gemini-2.0-flash"),
    );

    let config = Arc::new(config);
    let request = JsonRpcRequest::new(
        "models.capabilities",
        Some(json!({ "provider": "gemini" })),
        Some(json!(1)),
    );

    let response = models::handle_capabilities(request, config).await;

    assert!(response.result.is_some());
    let result = response.result.unwrap();
    let caps = result["capabilities"].as_array().unwrap();

    // Gemini 2.0 Flash should have thinking
    assert!(caps.iter().any(|c| c == "chat"));
    assert!(caps.iter().any(|c| c == "thinking"));
}

/// Test: models.capabilities with non-existent provider returns error
#[tokio::test]
async fn test_models_capabilities_not_found() {
    let config = Arc::new(Config::default());
    let request = JsonRpcRequest::new(
        "models.capabilities",
        Some(json!({ "provider": "nonexistent" })),
        Some(json!(1)),
    );

    let response = models::handle_capabilities(request, config).await;

    assert!(response.error.is_some());
}

// ============================================================================
// chat.* Param Deserialization Tests
// ============================================================================

/// Test: SendParams deserialization with all fields
#[test]
fn test_send_params_full_deserialization() {
    let json = json!({
        "message": "Hello, world!",
        "session_key": "agent:main:main",
        "channel": "gui:window1",
        "stream": true,
        "thinking": "high"
    });

    let params: SendParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.message, "Hello, world!");
    assert_eq!(params.session_key, Some("agent:main:main".to_string()));
    assert_eq!(params.channel, Some("gui:window1".to_string()));
    assert!(params.stream);
    assert_eq!(params.thinking, Some("high".to_string()));
}

/// Test: SendParams deserialization with minimal fields (defaults)
#[test]
fn test_send_params_minimal_deserialization() {
    let json = json!({
        "message": "Test message"
    });

    let params: SendParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.message, "Test message");
    assert!(params.session_key.is_none());
    assert!(params.channel.is_none());
    assert!(params.stream); // default true
    assert!(params.thinking.is_none());
}

/// Test: SendParams deserialization with stream=false
#[test]
fn test_send_params_stream_false() {
    let json = json!({
        "message": "No streaming",
        "stream": false
    });

    let params: SendParams = serde_json::from_value(json).unwrap();
    assert!(!params.stream);
}

/// Test: HistoryParams deserialization with all fields
#[test]
fn test_history_params_full_deserialization() {
    let json = json!({
        "session_key": "agent:main:main",
        "limit": 100,
        "before": "2024-01-01T00:00:00Z"
    });

    let params: HistoryParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.session_key, "agent:main:main");
    assert_eq!(params.limit, Some(100));
    assert_eq!(params.before, Some("2024-01-01T00:00:00Z".to_string()));
}

/// Test: HistoryParams deserialization with minimal fields
#[test]
fn test_history_params_minimal_deserialization() {
    let json = json!({
        "session_key": "agent:main:task:cron:daily"
    });

    let params: HistoryParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.session_key, "agent:main:task:cron:daily");
    assert!(params.limit.is_none());
    assert!(params.before.is_none());
}

/// Test: ClearParams deserialization with all fields
#[test]
fn test_clear_params_full_deserialization() {
    let json = json!({
        "session_key": "agent:main:main",
        "keep_system": false
    });

    let params: ClearParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.session_key, "agent:main:main");
    assert!(!params.keep_system);
}

/// Test: ClearParams deserialization with defaults
#[test]
fn test_clear_params_default_deserialization() {
    let json = json!({
        "session_key": "agent:main:main"
    });

    let params: ClearParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.session_key, "agent:main:main");
    assert!(params.keep_system); // default true
}
