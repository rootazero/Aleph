//! L4 Real API probe tests
//!
//! These tests call real provider APIs and require credentials.
//! Run with: cargo test -p alephcore --test real_api_probe -- --ignored
//!
//! Required env vars:
//! - OPENAI_API_KEY: for OpenAI tests
//! - GEMINI_API_KEY: for Gemini tests
//! - Ollama running on localhost:11434: for Ollama tests

use alephcore::providers::model_registry::ModelRegistry;
use alephcore::providers::protocols::openai::OpenAiProtocol;
use alephcore::providers::protocols::gemini::GeminiProtocol;
use alephcore::ProviderConfig;
use std::env;

#[tokio::test]
#[ignore]
async fn real_openai_list_models() {
    let api_key = env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");

    let adapter = OpenAiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gpt-4o");
    config.api_key = Some(api_key);

    let registry = ModelRegistry::new(None);
    let models = registry
        .list_models("real-openai", "openai", &adapter, &config)
        .await;

    assert!(!models.is_empty(), "OpenAI should return models");

    let has_gpt = models.iter().any(|m| m.id.contains("gpt"));
    assert!(has_gpt, "Should contain at least one GPT model");

    println!("OpenAI returned {} models", models.len());
    for m in models.iter().take(5) {
        println!("  - {} (owned_by: {:?})", m.id, m.owned_by);
    }
}

#[tokio::test]
#[ignore]
async fn real_gemini_list_models() {
    let api_key = env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY must be set");

    let adapter = GeminiProtocol::new(reqwest::Client::new());
    let mut config = ProviderConfig::test_config("gemini-2.5-pro");
    config.api_key = Some(api_key);

    let registry = ModelRegistry::new(None);
    let models = registry
        .list_models("real-gemini", "gemini", &adapter, &config)
        .await;

    assert!(!models.is_empty(), "Gemini should return models");

    let has_gemini = models.iter().any(|m| m.id.contains("gemini"));
    assert!(has_gemini, "Should contain at least one Gemini model");

    println!("Gemini returned {} models", models.len());
    for m in models.iter().take(5) {
        println!("  - {} (name: {:?})", m.id, m.name);
    }
}

#[tokio::test]
#[ignore]
async fn real_ollama_list_models() {
    use alephcore::providers::ollama::OllamaProvider;

    let config = ProviderConfig::test_config("llama3");

    let provider = OllamaProvider::new("ollama".to_string(), config)
        .expect("Should create OllamaProvider");
    let models = provider.list_models().await;

    match models {
        Ok(models) => {
            println!("Ollama returned {} models", models.len());
            for m in &models {
                println!("  - {}", m.id);
            }
        }
        Err(e) => {
            println!("Ollama not available or errored: {}", e);
        }
    }
}

#[tokio::test]
#[ignore]
async fn real_full_discovery_flow() {
    use alephcore::providers::model_registry::ModelSource;

    let registry = ModelRegistry::new(Some(include_str!(
        "../../shared/config/model-presets.toml"
    )));

    if let Ok(api_key) = env::var("OPENAI_API_KEY") {
        let adapter = OpenAiProtocol::new(reqwest::Client::new());
        let mut config = ProviderConfig::test_config("gpt-4o");
        config.api_key = Some(api_key);

        // Probe
        let models = registry
            .list_models("flow-openai", "openai", &adapter, &config)
            .await;
        assert!(!models.is_empty());
        assert_eq!(
            registry.get_source("flow-openai").await,
            Some(ModelSource::Api)
        );

        // Cache hit
        let models2 = registry
            .list_models("flow-openai", "openai", &adapter, &config)
            .await;
        assert_eq!(models.len(), models2.len());

        // Force refresh
        let models3 = registry
            .refresh("flow-openai", "openai", &adapter, &config)
            .await;
        assert!(!models3.is_empty());

        println!(
            "Full flow OK: {} models, cache + refresh working",
            models.len()
        );
    } else {
        println!("Skipping OpenAI flow test — OPENAI_API_KEY not set");
    }
}
