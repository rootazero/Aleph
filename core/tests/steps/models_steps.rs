//! Step definitions for Models and Chat handler features

use cucumber::{given, when, then};
use serde_json::json;

use crate::world::{AlephWorld, ModelsContext};
use alephcore::gateway::handlers::chat::{ClearParams, HistoryParams, SendParams};
use alephcore::gateway::handlers::models;
use alephcore::gateway::protocol::JsonRpcRequest;

// =========================================================================
// Given Steps - Config Setup
// =========================================================================

#[given("an empty config for models testing")]
async fn given_empty_config(w: &mut AlephWorld) {
    let ctx = w.models.get_or_insert_with(ModelsContext::default);
    ctx.init_empty_config();
}

#[given(expr = "a config with providers {string} {string} and {string}")]
async fn given_config_with_three_providers(w: &mut AlephWorld, p1: String, p2: String, p3: String) {
    let ctx = w.models.get_or_insert_with(ModelsContext::default);
    let model1 = match p1.as_str() {
        "openai" => "gpt-4o",
        "claude" => "claude-3-5-sonnet-20241022",
        "gemini" => "gemini-2.0-flash",
        _ => "test-model",
    };
    let model2 = match p2.as_str() {
        "openai" => "gpt-4o",
        "claude" => "claude-3-5-sonnet-20241022",
        "gemini" => "gemini-2.0-flash",
        _ => "test-model",
    };
    let model3 = match p3.as_str() {
        "openai" => "gpt-4o",
        "claude" => "claude-3-5-sonnet-20241022",
        "gemini" => "gemini-2.0-flash",
        _ => "test-model",
    };
    ctx.init_config_with_providers(vec![
        (&p1, model1),
        (&p2, model2),
        (&p3, model3),
    ]);
}

#[given(expr = "a config with providers {string} and {string}")]
async fn given_config_with_two_providers(w: &mut AlephWorld, p1: String, p2: String) {
    let ctx = w.models.get_or_insert_with(ModelsContext::default);
    let model1 = match p1.as_str() {
        "openai" => "gpt-4o",
        "claude" => "claude-3-5-sonnet-20241022",
        "gemini" => "gemini-2.0-flash",
        _ => "test-model",
    };
    let model2 = match p2.as_str() {
        "openai" => "gpt-4o",
        "claude" => "claude-3-5-sonnet-20241022",
        "gemini" => "gemini-2.0-flash",
        _ => "test-model",
    };
    ctx.init_config_with_providers(vec![(&p1, model1), (&p2, model2)]);
}

#[given(expr = "the default provider is {string}")]
async fn given_default_provider(w: &mut AlephWorld, provider: String) {
    let ctx = w.models.as_mut().expect("Models context not initialized");
    ctx.set_default_provider(&provider);
}

#[given(expr = "a config with enabled provider {string} and disabled provider {string}")]
async fn given_config_mixed_providers(w: &mut AlephWorld, enabled: String, disabled: String) {
    let ctx = w.models.get_or_insert_with(ModelsContext::default);
    let enabled_model = match enabled.as_str() {
        "openai" => "gpt-4o",
        "claude" => "claude-3-5-sonnet-20241022",
        "gemini" => "gemini-2.0-flash",
        _ => "test-model",
    };
    let disabled_model = match disabled.as_str() {
        "openai" => "gpt-4o",
        "claude" => "claude-3-5-sonnet-20241022",
        "gemini" => "gemini-2.0-flash",
        _ => "test-model",
    };
    ctx.init_config_with_mixed_providers(
        vec![(&enabled, enabled_model)],
        vec![(&disabled, disabled_model)],
    );
}

#[given(expr = "a config with provider {string} as default")]
async fn given_config_with_default_provider(w: &mut AlephWorld, provider: String) {
    let ctx = w.models.get_or_insert_with(ModelsContext::default);
    let model = match provider.as_str() {
        "openai" => "gpt-4o",
        "claude" => "claude-3-5-sonnet-20241022",
        "gemini" => "gemini-2.0-flash",
        _ => "test-model",
    };
    ctx.init_config_with_providers(vec![(&provider, model)]);
    ctx.set_default_provider(&provider);
}

#[given(expr = "a config with provider {string} with model {string}")]
async fn given_config_with_provider_model(w: &mut AlephWorld, provider: String, model: String) {
    let ctx = w.models.get_or_insert_with(ModelsContext::default);
    ctx.init_config_with_providers(vec![(&provider, &model)]);
}

// =========================================================================
// Given Steps - JSON Params
// =========================================================================

#[given("JSON for SendParams with all fields")]
async fn given_send_params_full(w: &mut AlephWorld) {
    let ctx = w.models.get_or_insert_with(ModelsContext::default);
    ctx.json_value = Some(json!({
        "message": "Hello, world!",
        "session_key": "agent:main:main",
        "channel": "gui:window1",
        "stream": true,
        "thinking": "high"
    }));
}

#[given(expr = "JSON for SendParams with only message {string}")]
async fn given_send_params_minimal(w: &mut AlephWorld, message: String) {
    let ctx = w.models.get_or_insert_with(ModelsContext::default);
    ctx.json_value = Some(json!({
        "message": message
    }));
}

#[given("JSON for SendParams with stream false")]
async fn given_send_params_stream_false(w: &mut AlephWorld) {
    let ctx = w.models.get_or_insert_with(ModelsContext::default);
    ctx.json_value = Some(json!({
        "message": "No streaming",
        "stream": false
    }));
}

#[given("JSON for HistoryParams with all fields")]
async fn given_history_params_full(w: &mut AlephWorld) {
    let ctx = w.models.get_or_insert_with(ModelsContext::default);
    ctx.json_value = Some(json!({
        "session_key": "agent:main:main",
        "limit": 100,
        "before": "2024-01-01T00:00:00Z"
    }));
}

#[given(expr = "JSON for HistoryParams with only session_key {string}")]
async fn given_history_params_minimal(w: &mut AlephWorld, session_key: String) {
    let ctx = w.models.get_or_insert_with(ModelsContext::default);
    ctx.json_value = Some(json!({
        "session_key": session_key
    }));
}

#[given("JSON for ClearParams with keep_system false")]
async fn given_clear_params_keep_system_false(w: &mut AlephWorld) {
    let ctx = w.models.get_or_insert_with(ModelsContext::default);
    ctx.json_value = Some(json!({
        "session_key": "agent:main:main",
        "keep_system": false
    }));
}

#[given("JSON for ClearParams with only session_key")]
async fn given_clear_params_minimal(w: &mut AlephWorld) {
    let ctx = w.models.get_or_insert_with(ModelsContext::default);
    ctx.json_value = Some(json!({
        "session_key": "agent:main:main"
    }));
}

// =========================================================================
// When Steps - models.list
// =========================================================================

#[when("I call models.list with no params")]
async fn when_call_models_list(w: &mut AlephWorld) {
    let ctx = w.models.as_mut().expect("Models context not initialized");
    let config = ctx.get_config();
    let request = JsonRpcRequest::new("models.list", None, Some(json!(1)));
    let response = models::handle_list(request, config).await;
    ctx.response = Some(response);
}

#[when("I call models.list with enabled_only filter")]
async fn when_call_models_list_enabled_only(w: &mut AlephWorld) {
    let ctx = w.models.as_mut().expect("Models context not initialized");
    let config = ctx.get_config();
    let request = JsonRpcRequest::new(
        "models.list",
        Some(json!({ "enabled_only": true })),
        Some(json!(1)),
    );
    let response = models::handle_list(request, config).await;
    ctx.response = Some(response);
}

#[when(expr = "I call models.list with provider filter {string}")]
async fn when_call_models_list_provider(w: &mut AlephWorld, provider: String) {
    let ctx = w.models.as_mut().expect("Models context not initialized");
    let config = ctx.get_config();
    let request = JsonRpcRequest::new(
        "models.list",
        Some(json!({ "provider": provider })),
        Some(json!(1)),
    );
    let response = models::handle_list(request, config).await;
    ctx.response = Some(response);
}

// =========================================================================
// When Steps - models.get
// =========================================================================

#[when(expr = "I call models.get for provider {string}")]
async fn when_call_models_get(w: &mut AlephWorld, provider: String) {
    let ctx = w.models.as_mut().expect("Models context not initialized");
    let config = ctx.get_config();
    let request = JsonRpcRequest::new(
        "models.get",
        Some(json!({ "provider": provider })),
        Some(json!(1)),
    );
    let response = models::handle_get(request, config).await;
    ctx.response = Some(response);
}

#[when("I call models.get with no params")]
async fn when_call_models_get_no_params(w: &mut AlephWorld) {
    let ctx = w.models.as_mut().expect("Models context not initialized");
    let config = ctx.get_config();
    let request = JsonRpcRequest::new("models.get", None, Some(json!(1)));
    let response = models::handle_get(request, config).await;
    ctx.response = Some(response);
}

// =========================================================================
// When Steps - models.capabilities
// =========================================================================

#[when(expr = "I call models.capabilities for provider {string}")]
async fn when_call_models_capabilities(w: &mut AlephWorld, provider: String) {
    let ctx = w.models.as_mut().expect("Models context not initialized");
    let config = ctx.get_config();
    let request = JsonRpcRequest::new(
        "models.capabilities",
        Some(json!({ "provider": provider })),
        Some(json!(1)),
    );
    let response = models::handle_capabilities(request, config).await;
    ctx.response = Some(response);
}

// =========================================================================
// When Steps - Param Deserialization
// =========================================================================

#[when("I deserialize the SendParams")]
async fn when_deserialize_send_params(w: &mut AlephWorld) {
    let ctx = w.models.as_mut().expect("Models context not initialized");
    let json = ctx.json_value.clone().expect("JSON value not set");
    let params: SendParams = serde_json::from_value(json).expect("Failed to deserialize SendParams");
    ctx.send_params = Some(params);
}

#[when("I deserialize the HistoryParams")]
async fn when_deserialize_history_params(w: &mut AlephWorld) {
    let ctx = w.models.as_mut().expect("Models context not initialized");
    let json = ctx.json_value.clone().expect("JSON value not set");
    let params: HistoryParams = serde_json::from_value(json).expect("Failed to deserialize HistoryParams");
    ctx.history_params = Some(params);
}

#[when("I deserialize the ClearParams")]
async fn when_deserialize_clear_params(w: &mut AlephWorld) {
    let ctx = w.models.as_mut().expect("Models context not initialized");
    let json = ctx.json_value.clone().expect("JSON value not set");
    let params: ClearParams = serde_json::from_value(json).expect("Failed to deserialize ClearParams");
    ctx.clear_params = Some(params);
}

// =========================================================================
// Then Steps - Response
// =========================================================================

#[then("the response should be successful")]
async fn then_response_successful(w: &mut AlephWorld) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    assert!(ctx.is_response_successful(), "Expected successful response");
}

#[then("the response should have an error")]
async fn then_response_error(w: &mut AlephWorld) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    assert!(!ctx.is_response_successful(), "Expected error response");
    assert!(ctx.get_error().is_some(), "Expected error in response");
}

#[then(expr = "the models error should contain {string}")]
async fn then_error_contains(w: &mut AlephWorld, expected: String) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let error = ctx.get_error().expect("No error in response");
    assert!(
        error.message.contains(&expected),
        "Error '{}' does not contain '{}'",
        error.message,
        expected
    );
}

// =========================================================================
// Then Steps - Models Array
// =========================================================================

#[then("the models array should be empty")]
async fn then_models_empty(w: &mut AlephWorld) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let models = ctx.get_models_array().expect("No models array in response");
    assert!(models.is_empty(), "Models array should be empty");
}

#[then(expr = "the models array should have {int} models")]
async fn then_models_count(w: &mut AlephWorld, count: usize) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let models = ctx.get_models_array().expect("No models array in response");
    assert_eq!(models.len(), count, "Expected {} models, got {}", count, models.len());
}

#[then(expr = "the models array should have {int} model")]
async fn then_models_count_singular(w: &mut AlephWorld, count: usize) {
    then_models_count(w, count).await;
}

#[then("each model should have required fields")]
async fn then_models_have_fields(w: &mut AlephWorld) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let models = ctx.get_models_array().expect("No models array in response");
    for model in models {
        assert!(model["id"].is_string(), "Model should have id");
        assert!(model["provider"].is_string(), "Model should have provider");
        assert!(model["provider_type"].is_string(), "Model should have provider_type");
        assert!(model["enabled"].is_boolean(), "Model should have enabled");
        assert!(model["is_default"].is_boolean(), "Model should have is_default");
        assert!(model["capabilities"].is_array(), "Model should have capabilities");
    }
}

#[then(expr = "one model should be marked as default with provider {string}")]
async fn then_default_model_provider(w: &mut AlephWorld, provider: String) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let models = ctx.get_models_array().expect("No models array in response");
    let default_model = models
        .iter()
        .find(|m| m["is_default"].as_bool().unwrap_or(false))
        .expect("No default model found");
    assert_eq!(
        default_model["provider"].as_str().unwrap(),
        provider,
        "Default model provider mismatch"
    );
}

#[then(expr = "the model provider should be {string}")]
async fn then_model_provider(w: &mut AlephWorld, provider: String) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let models = ctx.get_models_array().expect("No models array in response");
    assert_eq!(models.len(), 1, "Expected exactly one model");
    assert_eq!(
        models[0]["provider"].as_str().unwrap(),
        provider,
        "Model provider mismatch"
    );
}

// =========================================================================
// Then Steps - Single Model (from models.get)
// =========================================================================

#[then(expr = "the returned model id should be {string}")]
async fn then_returned_model_id(w: &mut AlephWorld, id: String) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let model = ctx.get_model().expect("No model in response");
    assert_eq!(model["id"].as_str().unwrap(), id, "Model id mismatch");
}

#[then(expr = "the returned model provider should be {string}")]
async fn then_returned_model_provider(w: &mut AlephWorld, provider: String) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let model = ctx.get_model().expect("No model in response");
    assert_eq!(model["provider"].as_str().unwrap(), provider, "Model provider mismatch");
}

#[then(expr = "the returned model provider_type should be {string}")]
async fn then_returned_model_provider_type(w: &mut AlephWorld, provider_type: String) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let model = ctx.get_model().expect("No model in response");
    assert_eq!(
        model["provider_type"].as_str().unwrap(),
        provider_type,
        "Model provider_type mismatch"
    );
}

#[then("the returned model should be enabled")]
async fn then_returned_model_enabled(w: &mut AlephWorld) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let model = ctx.get_model().expect("No model in response");
    assert!(model["enabled"].as_bool().unwrap(), "Model should be enabled");
}

#[then("the returned model should be marked as default")]
async fn then_returned_model_default(w: &mut AlephWorld) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let model = ctx.get_model().expect("No model in response");
    assert!(model["is_default"].as_bool().unwrap(), "Model should be marked as default");
}

#[then(expr = "the returned model capabilities should include {string} {string} and {string}")]
async fn then_returned_model_capabilities(w: &mut AlephWorld, cap1: String, cap2: String, cap3: String) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let model = ctx.get_model().expect("No model in response");
    let caps = model["capabilities"].as_array().expect("No capabilities array");
    assert!(caps.iter().any(|c| c.as_str() == Some(&cap1)), "Missing capability: {}", cap1);
    assert!(caps.iter().any(|c| c.as_str() == Some(&cap2)), "Missing capability: {}", cap2);
    assert!(caps.iter().any(|c| c.as_str() == Some(&cap3)), "Missing capability: {}", cap3);
}

// =========================================================================
// Then Steps - Capabilities
// =========================================================================

#[then(expr = "the capabilities should include {string}")]
async fn then_capabilities_include(w: &mut AlephWorld, cap: String) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let caps = ctx.get_capabilities().expect("No capabilities in response");
    assert!(
        caps.iter().any(|c| c.as_str() == Some(&cap)),
        "Missing capability: {}",
        cap
    );
}

// =========================================================================
// Then Steps - SendParams
// =========================================================================

#[then(expr = "the message should be {string}")]
async fn then_send_message(w: &mut AlephWorld, expected: String) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let params = ctx.send_params.as_ref().expect("SendParams not deserialized");
    assert_eq!(params.message, expected, "Message mismatch");
}

#[then(expr = "the session_key should be {string}")]
async fn then_send_session_key(w: &mut AlephWorld, expected: String) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let params = ctx.send_params.as_ref().expect("SendParams not deserialized");
    assert_eq!(params.session_key, Some(expected), "session_key mismatch");
}

#[then("the session_key should be none")]
async fn then_send_session_key_none(w: &mut AlephWorld) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let params = ctx.send_params.as_ref().expect("SendParams not deserialized");
    assert!(params.session_key.is_none(), "session_key should be None");
}

#[then(expr = "the channel should be {string}")]
async fn then_send_channel(w: &mut AlephWorld, expected: String) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let params = ctx.send_params.as_ref().expect("SendParams not deserialized");
    assert_eq!(params.channel, Some(expected), "channel mismatch");
}

#[then("the channel should be none")]
async fn then_send_channel_none(w: &mut AlephWorld) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let params = ctx.send_params.as_ref().expect("SendParams not deserialized");
    assert!(params.channel.is_none(), "channel should be None");
}

#[then("stream should be true")]
async fn then_stream_true(w: &mut AlephWorld) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let params = ctx.send_params.as_ref().expect("SendParams not deserialized");
    assert!(params.stream, "stream should be true");
}

#[then("stream should be false")]
async fn then_stream_false(w: &mut AlephWorld) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let params = ctx.send_params.as_ref().expect("SendParams not deserialized");
    assert!(!params.stream, "stream should be false");
}

#[then(expr = "thinking should be {string}")]
async fn then_thinking(w: &mut AlephWorld, expected: String) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let params = ctx.send_params.as_ref().expect("SendParams not deserialized");
    assert_eq!(params.thinking, Some(expected), "thinking mismatch");
}

#[then("thinking should be none")]
async fn then_thinking_none(w: &mut AlephWorld) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let params = ctx.send_params.as_ref().expect("SendParams not deserialized");
    assert!(params.thinking.is_none(), "thinking should be None");
}

// =========================================================================
// Then Steps - HistoryParams
// =========================================================================

#[then(expr = "the history session_key should be {string}")]
async fn then_history_session_key(w: &mut AlephWorld, expected: String) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let params = ctx.history_params.as_ref().expect("HistoryParams not deserialized");
    assert_eq!(params.session_key, expected, "session_key mismatch");
}

#[then(expr = "the limit should be {int}")]
async fn then_history_limit(w: &mut AlephWorld, expected: usize) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let params = ctx.history_params.as_ref().expect("HistoryParams not deserialized");
    assert_eq!(params.limit, Some(expected), "limit mismatch");
}

#[then("the limit should be none")]
async fn then_history_limit_none(w: &mut AlephWorld) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let params = ctx.history_params.as_ref().expect("HistoryParams not deserialized");
    assert!(params.limit.is_none(), "limit should be None");
}

#[then(expr = "the before should be {string}")]
async fn then_history_before(w: &mut AlephWorld, expected: String) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let params = ctx.history_params.as_ref().expect("HistoryParams not deserialized");
    assert_eq!(params.before, Some(expected), "before mismatch");
}

#[then("the before should be none")]
async fn then_history_before_none(w: &mut AlephWorld) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let params = ctx.history_params.as_ref().expect("HistoryParams not deserialized");
    assert!(params.before.is_none(), "before should be None");
}

// =========================================================================
// Then Steps - ClearParams
// =========================================================================

#[then(expr = "the clear session_key should be {string}")]
async fn then_clear_session_key(w: &mut AlephWorld, expected: String) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let params = ctx.clear_params.as_ref().expect("ClearParams not deserialized");
    assert_eq!(params.session_key, expected, "session_key mismatch");
}

#[then("keep_system should be true")]
async fn then_keep_system_true(w: &mut AlephWorld) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let params = ctx.clear_params.as_ref().expect("ClearParams not deserialized");
    assert!(params.keep_system, "keep_system should be true");
}

#[then("keep_system should be false")]
async fn then_keep_system_false(w: &mut AlephWorld) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let params = ctx.clear_params.as_ref().expect("ClearParams not deserialized");
    assert!(!params.keep_system, "keep_system should be false");
}

// =========================================================================
// Model Discovery Steps
// =========================================================================

#[then("the models array should contain models with source field")]
async fn then_models_have_source(w: &mut AlephWorld) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let models = ctx.get_models_array().expect("No models in response");
    for model in models {
        assert!(model.get("source").is_some(), "Model missing 'source' field: {:?}", model);
    }
}

#[then("the models should include preset models")]
async fn then_models_include_presets(w: &mut AlephWorld) {
    let ctx = w.models.as_ref().expect("Models context not initialized");
    let models = ctx.get_models_array().expect("No models in response");
    assert!(!models.is_empty(), "Expected preset models but got empty");
}

#[when(expr = "I call models.refresh for provider {string}")]
async fn when_call_models_refresh(w: &mut AlephWorld, provider: String) {
    let ctx = w.models.as_mut().expect("Models context not initialized");
    let config = ctx.get_config();
    let request = JsonRpcRequest::new(
        "models.refresh",
        Some(json!({"provider": provider})),
        Some(json!(1)),
    );
    let response = models::handle_refresh(request, config).await;
    ctx.response = Some(response);
}

#[given(expr = "a mutable config with providers {string} and {string} default {string}")]
async fn given_mutable_config(w: &mut AlephWorld, p1: String, p2: String, default: String) {
    let ctx = w.models.get_or_insert_with(ModelsContext::default);
    ctx.init_mutable_config_with_providers(vec![(&p1, "model-1"), (&p2, "model-2")]);
    // Set default on the mutable config
    {
        let mutable_cfg = ctx.get_mutable_config();
        let mut config = mutable_cfg.write().await;
        config.general.default_provider = Some(default.clone());
    }
    // Also init read-only config for other handlers
    ctx.init_config_with_providers(vec![(&p1, "model-1"), (&p2, "model-2")]);
    ctx.set_default_provider(&default);
}

#[when(expr = "I call models.set with model {string}")]
async fn when_call_models_set(w: &mut AlephWorld, model: String) {
    let ctx = w.models.as_mut().expect("Models context not initialized");
    let config = ctx.get_mutable_config();
    let request = JsonRpcRequest::new(
        "models.set",
        Some(json!({"model": model})),
        Some(json!(1)),
    );
    let response = models::handle_set(request, config).await;
    ctx.response = Some(response);
}
