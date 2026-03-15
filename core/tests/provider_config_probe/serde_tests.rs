//! Serialization and backward compatibility tests for provider config types.
//!
//! Validates:
//! - TOML backward compat: `model = "x"` → `models: ["x"]`
//! - Multi-model round-trip
//! - Empty list rejection for required-models types
//! - Empty string filtering
//! - default_model() returns first element
//! - Serialization outputs `models = [...]`, never `model = ...`

use alephcore::ProviderConfig;
use alephcore::EmbeddingProviderConfig;
use alephcore::memory::rerank::RerankConfig;
use alephcore::GenerationProviderConfig;

// =============================================================================
// ProviderConfig serde tests
// =============================================================================

#[test]
fn provider_backward_compat_single_model() {
    // `model` is an alias for `models` — when only `model` is present it should work
    let toml_str = r#"
        model = "gpt-4o"
    "#;
    let config: ProviderConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.models, vec!["gpt-4o"]);
    assert_eq!(config.default_model(), "gpt-4o");
}

#[test]
fn provider_multi_model_roundtrip() {
    let toml_str = r#"
        models = ["gpt-4o", "gpt-4o-mini", "gpt-3.5-turbo"]
    "#;
    let config: ProviderConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.models, vec!["gpt-4o", "gpt-4o-mini", "gpt-3.5-turbo"]);
    assert_eq!(config.default_model(), "gpt-4o");

    // Round-trip: serialize and deserialize
    let serialized = toml::to_string_pretty(&config).unwrap();
    let config2: ProviderConfig = toml::from_str(&serialized).unwrap();
    assert_eq!(config2.models, config.models);
}

#[test]
fn provider_empty_list_rejected() {
    let toml_str = r#"
        models = []
    "#;
    let result = toml::from_str::<ProviderConfig>(toml_str);
    assert!(result.is_err(), "empty models list should be rejected");
}

#[test]
fn provider_empty_string_rejected() {
    let toml_str = r#"
        model = ""
    "#;
    let result = toml::from_str::<ProviderConfig>(toml_str);
    assert!(result.is_err(), "empty model string should be rejected");
}

#[test]
fn provider_whitespace_only_strings_filtered() {
    let toml_str = r#"
        models = ["gpt-4o", "  ", "gpt-4o-mini"]
    "#;
    let config: ProviderConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.models, vec!["gpt-4o", "gpt-4o-mini"]);
}

#[test]
fn provider_all_whitespace_rejected() {
    let toml_str = r#"
        models = ["  ", "  "]
    "#;
    let result = toml::from_str::<ProviderConfig>(toml_str);
    assert!(result.is_err(), "all-whitespace models should be rejected");
}

#[test]
fn provider_serialization_uses_models_array() {
    let config = ProviderConfig::test_config("gpt-4o");
    let serialized = toml::to_string_pretty(&config).unwrap();
    // Should contain `models = [` not `model = `
    assert!(serialized.contains("models = ["), "serialized output: {}", serialized);
    assert!(!serialized.contains("model = \""), "should not have singular model key");
}

#[test]
fn provider_default_model_is_first() {
    let mut config = ProviderConfig::test_config("first");
    config.models.push("second".to_string());
    config.models.push("third".to_string());
    assert_eq!(config.default_model(), "first");
    assert_eq!(config.all_models(), &["first", "second", "third"]);
}

#[test]
fn provider_test_config_helper() {
    let config = ProviderConfig::test_config("claude-3-5-sonnet");
    assert_eq!(config.models, vec!["claude-3-5-sonnet"]);
    assert!(config.enabled);
    assert!(config.api_key.is_some());
}

#[test]
fn provider_optional_fields_default() {
    let toml_str = r#"
        models = ["gpt-4o"]
    "#;
    let config: ProviderConfig = toml::from_str(toml_str).unwrap();
    assert!(config.protocol.is_none());
    assert!(config.base_url.is_none());
    assert!(config.max_tokens.is_none());
    assert!(config.temperature.is_none());
    assert!(!config.verified);
}

// =============================================================================
// EmbeddingProviderConfig serde tests
// =============================================================================

#[test]
fn embedding_backward_compat_single_model() {
    let toml_str = r#"
        id = "siliconflow"
        name = "SiliconFlow"
        preset = "silicon_flow"
        api_base = "https://api.siliconflow.cn/v1"
        model = "BAAI/bge-m3"
        dimensions = 1024
    "#;
    let config: EmbeddingProviderConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.models, vec!["BAAI/bge-m3"]);
    assert_eq!(config.default_model(), "BAAI/bge-m3");
}

#[test]
fn embedding_multi_model_roundtrip() {
    let toml_str = r#"
        id = "openai"
        name = "OpenAI"
        preset = "open_ai"
        api_base = "https://api.openai.com/v1"
        models = ["text-embedding-3-small", "text-embedding-3-large"]
        dimensions = 1536
    "#;
    let config: EmbeddingProviderConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.models.len(), 2);
    assert_eq!(config.default_model(), "text-embedding-3-small");

    let serialized = toml::to_string_pretty(&config).unwrap();
    let config2: EmbeddingProviderConfig = toml::from_str(&serialized).unwrap();
    assert_eq!(config2.models, config.models);
}

#[test]
fn embedding_empty_list_rejected() {
    let toml_str = r#"
        id = "test"
        name = "Test"
        preset = "custom"
        api_base = "http://localhost"
        models = []
        dimensions = 768
    "#;
    let result = toml::from_str::<EmbeddingProviderConfig>(toml_str);
    assert!(result.is_err(), "empty models list should be rejected for embedding");
}

#[test]
fn embedding_preset_factories() {
    let sf = EmbeddingProviderConfig::siliconflow();
    assert_eq!(sf.id, "siliconflow");
    assert_eq!(sf.default_model(), "BAAI/bge-m3");
    assert_eq!(sf.dimensions, 1024);

    let oai = EmbeddingProviderConfig::openai();
    assert_eq!(oai.id, "openai");
    assert_eq!(oai.default_model(), "text-embedding-3-small");

    let ollama = EmbeddingProviderConfig::ollama();
    assert_eq!(ollama.id, "ollama");
    assert_eq!(ollama.default_model(), "nomic-embed-text");
}

#[test]
fn embedding_serialization_uses_models_array() {
    let config = EmbeddingProviderConfig::siliconflow();
    let serialized = toml::to_string_pretty(&config).unwrap();
    assert!(serialized.contains("models = ["), "serialized: {}", serialized);
}

// =============================================================================
// RerankConfig serde tests
// =============================================================================

#[test]
fn rerank_backward_compat_single_model() {
    let toml_str = r#"
        enabled = true
        provider = "jina"
        api_base = "https://api.jina.ai/v1"
        api_key = "test-key"
        model = "jina-reranker-v2-base-multilingual"
    "#;
    let config: RerankConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.models, vec!["jina-reranker-v2-base-multilingual"]);
    assert_eq!(config.default_model(), "jina-reranker-v2-base-multilingual");
}

#[test]
fn rerank_multi_model_roundtrip() {
    let toml_str = r#"
        enabled = true
        provider = "siliconflow"
        api_base = "https://api.siliconflow.cn/v1"
        api_key = "sf-key"
        models = ["BAAI/bge-reranker-v2-m3", "BAAI/bge-reranker-v2-gemma"]
    "#;
    let config: RerankConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.models.len(), 2);
    assert_eq!(config.default_model(), "BAAI/bge-reranker-v2-m3");

    let serialized = toml::to_string_pretty(&config).unwrap();
    let config2: RerankConfig = toml::from_str(&serialized).unwrap();
    assert_eq!(config2.models, config.models);
}

#[test]
fn rerank_default_has_model() {
    let config = RerankConfig::default();
    assert!(!config.models.is_empty());
    assert_eq!(config.default_model(), "BAAI/bge-reranker-v2-m3");
    assert!(!config.enabled);
}

#[test]
fn rerank_serialization_uses_models_array() {
    let config = RerankConfig::default();
    let serialized = toml::to_string_pretty(&config).unwrap();
    assert!(serialized.contains("models = ["), "serialized: {}", serialized);
}

// =============================================================================
// GenerationProviderConfig serde tests
// =============================================================================

#[test]
fn generation_backward_compat_single_model() {
    let toml_str = r#"
        provider_type = "openai"
        model = "dall-e-3"
    "#;
    let config: GenerationProviderConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.models, vec!["dall-e-3"]);
    assert_eq!(config.default_model(), Some("dall-e-3"));
}

#[test]
fn generation_multi_model() {
    let toml_str = r#"
        provider_type = "openai"
        models = ["dall-e-3", "dall-e-2", "gpt-image-1"]
    "#;
    let config: GenerationProviderConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.models.len(), 3);
    assert_eq!(config.default_model(), Some("dall-e-3"));
}

#[test]
fn generation_empty_models_allowed() {
    // GenerationProviderConfig uses deserialize_optional_models: empty is OK
    let toml_str = r#"
        provider_type = "openai"
        models = []
    "#;
    let config: GenerationProviderConfig = toml::from_str(toml_str).unwrap();
    assert!(config.models.is_empty());
    assert_eq!(config.default_model(), None);
}

#[test]
fn generation_no_models_field() {
    let toml_str = r#"
        provider_type = "openai"
    "#;
    let config: GenerationProviderConfig = toml::from_str(toml_str).unwrap();
    assert!(config.models.is_empty());
    assert_eq!(config.default_model(), None);
}

#[test]
fn generation_model_aliases() {
    let toml_str = r#"
        provider_type = "openai"
        models = ["dall-e-3"]

        [model_aliases]
        fast = "dall-e-2"
        hd = "dall-e-3"
    "#;
    let config: GenerationProviderConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.model_aliases.len(), 2);
    assert_eq!(config.resolve_model(Some("fast")), Some("dall-e-2"));
    assert_eq!(config.resolve_model(Some("hd")), Some("dall-e-3"));
    assert_eq!(config.resolve_model(None), Some("dall-e-3"));
}

#[test]
fn generation_serialization_uses_models_array() {
    let mut config = GenerationProviderConfig::new("openai");
    config.models = vec!["dall-e-3".to_string()];
    let serialized = toml::to_string_pretty(&config).unwrap();
    assert!(serialized.contains("models = ["), "serialized: {}", serialized);
}
