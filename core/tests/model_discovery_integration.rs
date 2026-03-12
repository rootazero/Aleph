//! L2 Integration tests: wiremock + ModelRegistry

use alephcore::ProviderConfig;
use alephcore::providers::model_registry::{ModelRegistry, ModelSource};
use alephcore::providers::protocols::openai::OpenAiProtocol;
use alephcore::providers::protocols::gemini::GeminiProtocol;
use alephcore::providers::ollama::OllamaProvider;
use serde_json::json;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn openai_models_response() -> serde_json::Value {
    json!({
        "data": [
            {"id": "gpt-4o", "owned_by": "openai"},
            {"id": "gpt-4o-mini", "owned_by": "openai"},
            {"id": "o3", "owned_by": "openai"}
        ]
    })
}

fn gemini_models_response() -> serde_json::Value {
    json!({
        "models": [
            {"name": "models/gemini-2.5-pro", "displayName": "Gemini 2.5 Pro"},
            {"name": "models/gemini-2.5-flash", "displayName": "Gemini 2.5 Flash"}
        ]
    })
}

#[tokio::test]
async fn probe_openai_models_via_api() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_models_response()))
        .mount(&server)
        .await;

    let adapter = OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some(server.uri());

    let registry = ModelRegistry::new(None);
    let models = registry
        .list_models("test-openai", "openai", &adapter, &config)
        .await;

    assert_eq!(models.len(), 3);
    assert_eq!(models[0].id, "gpt-4o");
    assert_eq!(
        registry.get_source("test-openai").await,
        Some(ModelSource::Api)
    );
}

#[tokio::test]
async fn probe_gemini_models_via_api() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1beta/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(gemini_models_response()))
        .mount(&server)
        .await;

    let adapter = GeminiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gemini-2.5-pro");
    config.base_url = Some(server.uri());

    let registry = ModelRegistry::new(None);
    let models = registry
        .list_models("test-gemini", "gemini", &adapter, &config)
        .await;

    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "gemini-2.5-pro");
    assert_eq!(models[0].name, Some("Gemini 2.5 Pro".to_string()));
    assert_eq!(
        registry.get_source("test-gemini").await,
        Some(ModelSource::Api)
    );
}

#[tokio::test]
async fn probe_ollama_tags_via_api() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({
                "models": [
                    {"name": "llama3:latest"},
                    {"name": "llava:7b"}
                ]
            })),
        )
        .mount(&server)
        .await;

    let mut config = ProviderConfig::test_config("llama3");
    config.base_url = Some(server.uri());

    let provider = OllamaProvider::new("test-ollama".to_string(), config)
        .expect("Should create OllamaProvider");
    let models = provider.list_models().await.expect("Should list models");

    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "llama3:latest");
    assert_eq!(models[1].id, "llava:7b");
}

// ── Task 7: Fallback degradation tests ───────────────────────────────────────

#[tokio::test]
async fn fallback_to_preset_on_api_failure() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let adapter = OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some(server.uri());

    let preset_toml = r#"
[openai]
models = [
    { id = "gpt-4o", name = "GPT-4o", capabilities = ["chat", "vision"] },
]
"#;
    let registry = ModelRegistry::new(Some(preset_toml));
    let models = registry.list_models("test-openai", "openai", &adapter, &config).await;

    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "gpt-4o");
    assert_eq!(registry.get_source("test-openai").await, Some(ModelSource::Preset));
}

#[tokio::test]
async fn fallback_to_preset_on_timeout() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(openai_models_response())
                .set_delay(Duration::from_secs(10)),
        )
        .mount(&server)
        .await;

    let adapter = OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some(server.uri());

    let preset_toml = r#"
[openai]
models = [
    { id = "gpt-4o-fallback", name = "Fallback", capabilities = ["chat"] },
]
"#;
    let registry = ModelRegistry::new(Some(preset_toml));
    let models = registry.list_models("test-openai", "openai", &adapter, &config).await;

    assert!(!models.is_empty());
    assert_eq!(registry.get_source("test-openai").await, Some(ModelSource::Preset));
}

#[tokio::test]
async fn fallback_to_preset_on_401_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"error": "invalid_api_key"})))
        .mount(&server)
        .await;

    let adapter = OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some(server.uri());

    let preset_toml = r#"
[openai]
models = [
    { id = "gpt-4o", name = "GPT-4o", capabilities = ["chat"] },
]
"#;
    let registry = ModelRegistry::new(Some(preset_toml));
    let models = registry.list_models("test-openai", "openai", &adapter, &config).await;

    assert_eq!(models.len(), 1);
    assert_eq!(registry.get_source("test-openai").await, Some(ModelSource::Preset));
}

#[tokio::test]
async fn fallback_to_empty_when_no_preset() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let adapter = OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some(server.uri());

    let registry = ModelRegistry::new(None);
    let models = registry.list_models("test-openai", "openai", &adapter, &config).await;

    assert!(models.is_empty());
}

#[tokio::test]
async fn fallback_to_preset_on_empty_api_response() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": []})))
        .mount(&server)
        .await;

    let adapter = OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some(server.uri());

    let preset_toml = r#"
[openai]
models = [
    { id = "gpt-4o", name = "GPT-4o", capabilities = ["chat"] },
]
"#;
    let registry = ModelRegistry::new(Some(preset_toml));
    let models = registry.list_models("test-openai", "openai", &adapter, &config).await;

    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "gpt-4o");
    assert_eq!(registry.get_source("test-openai").await, Some(ModelSource::Preset));
}

// ── Task 8: Cache and concurrency tests ──────────────────────────────────────

#[tokio::test]
async fn cache_hit_avoids_api_call() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_models_response()))
        .expect(1)
        .mount(&server)
        .await;

    let adapter = OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some(server.uri());

    let registry = ModelRegistry::new(None);
    let models1 = registry.list_models("test-openai", "openai", &adapter, &config).await;
    assert_eq!(models1.len(), 3);

    let models2 = registry.list_models("test-openai", "openai", &adapter, &config).await;
    assert_eq!(models2.len(), 3);
    // wiremock verifies exactly 1 request on drop
}

#[tokio::test]
async fn cache_expires_after_ttl() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_models_response()))
        .expect(2)
        .mount(&server)
        .await;

    let adapter = OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some(server.uri());

    let registry = ModelRegistry::new(None).with_ttl(Duration::from_millis(100));
    let _ = registry.list_models("test-openai", "openai", &adapter, &config).await;

    tokio::time::sleep(Duration::from_millis(150)).await;

    let models = registry.list_models("test-openai", "openai", &adapter, &config).await;
    assert_eq!(models.len(), 3);
    // wiremock verifies exactly 2 requests on drop
}

#[tokio::test]
async fn force_refresh_bypasses_cache() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_models_response()))
        .expect(2)
        .mount(&server)
        .await;

    let adapter = OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some(server.uri());

    let registry = ModelRegistry::new(None);
    let _ = registry.list_models("test-openai", "openai", &adapter, &config).await;

    let models = registry.refresh("test-openai", "openai", &adapter, &config).await;
    assert_eq!(models.len(), 3);
    // wiremock verifies 2 requests
}

#[tokio::test]
async fn concurrent_list_models_no_panic() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(openai_models_response()))
        .mount(&server)
        .await;

    let adapter = std::sync::Arc::new(OpenAiProtocol::new(reqwest::Client::new()));
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.base_url = Some(server.uri());
    let config = std::sync::Arc::new(config);
    let registry = std::sync::Arc::new(ModelRegistry::new(None));

    let mut handles = vec![];
    for i in 0..10 {
        let registry = std::sync::Arc::clone(&registry);
        let adapter = std::sync::Arc::clone(&adapter);
        let config = std::sync::Arc::clone(&config);
        handles.push(tokio::spawn(async move {
            registry.list_models(&format!("provider-{}", i), "openai", adapter.as_ref(), &config).await
        }));
    }
    for _ in 0..2 {
        let registry = std::sync::Arc::clone(&registry);
        let adapter = std::sync::Arc::clone(&adapter);
        let config = std::sync::Arc::clone(&config);
        handles.push(tokio::spawn(async move {
            registry.refresh("concurrent-refresh", "openai", adapter.as_ref(), &config).await
        }));
    }

    for handle in handles {
        let models = handle.await.expect("task should not panic");
        assert!(!models.is_empty());
    }
}
