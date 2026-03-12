//! L2 Integration tests: wiremock + ModelRegistry

use alephcore::ProviderConfig;
use alephcore::providers::model_registry::{ModelRegistry, ModelSource};
use alephcore::providers::protocols::openai::OpenAiProtocol;
use alephcore::providers::protocols::gemini::GeminiProtocol;
use alephcore::providers::ollama::OllamaProvider;
use serde_json::json;
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
