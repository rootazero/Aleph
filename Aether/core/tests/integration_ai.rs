use aethecore::providers::mock::MockProvider;
/// Integration tests for AI pipeline
///
/// Tests end-to-end AI processing with MockProvider to avoid real API calls.
/// Covers:
/// - Routing with multiple rules
/// - Memory augmentation
/// - Error recovery and fallback
/// - Timeout handling
use aethecore::{
    AetherError, AetherEventHandler, AiProvider, Config, ProcessingState, ProviderConfig, Router,
    RoutingRuleConfig,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;

/// Mock event handler for testing
#[derive(Clone)]
struct TestEventHandler {
    states: Arc<Mutex<Vec<ProcessingState>>>,
    errors: Arc<Mutex<Vec<String>>>,
}

impl TestEventHandler {
    fn new() -> Self {
        Self {
            states: Arc::new(Mutex::new(Vec::new())),
            errors: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn get_states(&self) -> Vec<ProcessingState> {
        self.states.lock().unwrap().clone()
    }

    fn get_errors(&self) -> Vec<String> {
        self.errors.lock().unwrap().clone()
    }

    fn clear(&self) {
        self.states.lock().unwrap().clear();
        self.errors.lock().unwrap().clear();
    }
}

impl AetherEventHandler for TestEventHandler {
    fn on_state_changed(&self, state: ProcessingState) {
        self.states.lock().unwrap().push(state);
    }

    fn on_hotkey_detected(&self, _clipboard_content: String) {}

    fn on_error(&self, message: String) {
        self.errors.lock().unwrap().push(message);
    }

    fn on_response_chunk(&self, _text: String) {}

    fn on_error_typed(&self, _error_type: aethecore::ErrorType, message: String) {
        self.errors.lock().unwrap().push(message);
    }

    fn on_progress(&self, _percent: f32) {}

    fn on_ai_processing_started(&self, _provider_name: String, _provider_color: String) {}

    fn on_ai_response_received(&self, _response_preview: String) {}
}

/// Create a test config with mock providers
fn create_test_config() -> Config {
    let mut config = Config::default();

    // Add mock OpenAI provider
    config.providers.insert(
        "openai".to_string(),
        ProviderConfig {
            provider_type: Some("mock".to_string()),
            api_key: Some("test-key".to_string()),
            model: "gpt-4o".to_string(),
            base_url: None,
            color: "#10a37f".to_string(),
            timeout_seconds: 30,
            max_tokens: Some(4096),
            temperature: Some(0.7),
        },
    );

    // Add mock Claude provider
    config.providers.insert(
        "claude".to_string(),
        ProviderConfig {
            provider_type: Some("mock".to_string()),
            api_key: Some("test-key".to_string()),
            model: "claude-3-5-sonnet".to_string(),
            base_url: None,
            color: "#d97757".to_string(),
            timeout_seconds: 30,
            max_tokens: Some(4096),
            temperature: Some(0.7),
        },
    );

    // Add routing rules
    config.rules.push(RoutingRuleConfig {
        regex: "^/code".to_string(),
        provider: "claude".to_string(),
        system_prompt: Some("You are a coding assistant.".to_string()),
    });

    config.rules.push(RoutingRuleConfig {
        regex: ".*".to_string(),
        provider: "openai".to_string(),
        system_prompt: None,
    });

    config.general.default_provider = Some("openai".to_string());

    config
}

#[test]
fn test_router_with_multiple_rules() {
    let config = create_test_config();

    // Router::new will create mock providers internally based on config
    let router = Router::new(&config).unwrap();

    // Test routing to Claude
    let (provider, system_prompt) = router.route("/code write a function").unwrap();
    assert_eq!(provider.name(), "mock"); // MockProvider returns "mock" as its name
    assert_eq!(system_prompt, Some("You are a coding assistant."));

    // Test routing to OpenAI (default)
    let (provider, system_prompt) = router.route("tell me a joke").unwrap();
    assert_eq!(provider.name(), "mock");
    assert_eq!(system_prompt, None);
}

#[test]
fn test_routing_priority() {
    let mut config = Config::default();

    // Add provider
    config.providers.insert(
        "provider1".to_string(),
        ProviderConfig {
            provider_type: Some("mock".to_string()),
            api_key: Some("test".to_string()),
            model: "test".to_string(),
            base_url: None,
            color: "#000000".to_string(),
            timeout_seconds: 30,
            max_tokens: None,
            temperature: None,
        },
    );

    config.providers.insert(
        "provider2".to_string(),
        ProviderConfig {
            provider_type: Some("mock".to_string()),
            api_key: Some("test".to_string()),
            model: "test".to_string(),
            base_url: None,
            color: "#000000".to_string(),
            timeout_seconds: 30,
            max_tokens: None,
            temperature: None,
        },
    );

    // First rule should match first
    config.rules.push(RoutingRuleConfig {
        regex: "^test".to_string(),
        provider: "provider1".to_string(),
        system_prompt: Some("prompt1".to_string()),
    });

    // This rule also matches but should not be used
    config.rules.push(RoutingRuleConfig {
        regex: "test".to_string(),
        provider: "provider2".to_string(),
        system_prompt: Some("prompt2".to_string()),
    });

    let router = Router::new(&config).unwrap();

    // First rule should match
    let (provider, system_prompt) = router.route("test input").unwrap();
    assert_eq!(system_prompt, Some("prompt1"));
}

#[test]
fn test_default_provider_fallback() {
    let mut config = Config::default();

    config.providers.insert(
        "default".to_string(),
        ProviderConfig {
            provider_type: Some("mock".to_string()),
            api_key: Some("test".to_string()),
            model: "test".to_string(),
            base_url: None,
            color: "#000000".to_string(),
            timeout_seconds: 30,
            max_tokens: None,
            temperature: None,
        },
    );

    config.general.default_provider = Some("default".to_string());

    // No routing rules - should use default
    let router = Router::new(&config).unwrap();

    // Should use default provider
    let (provider, system_prompt) = router.route("anything").unwrap();
    assert_eq!(provider.name(), "mock");
    assert_eq!(system_prompt, None);
}

#[tokio::test]
async fn test_mock_provider_processing() {
    let provider = MockProvider::new("Test response".to_string());

    let result = provider.process("Test input", None).await.unwrap();
    assert_eq!(result, "Test response");

    // Test with system prompt
    let result = provider
        .process("Test input", Some("You are a helpful assistant"))
        .await
        .unwrap();
    assert_eq!(result, "Test response");
}

#[tokio::test]
async fn test_mock_provider_with_delay() {
    let provider = MockProvider::new("Response".to_string()).with_delay(Duration::from_millis(100));

    let start = std::time::Instant::now();
    let result = provider.process("Input", None).await.unwrap();
    let elapsed = start.elapsed();

    assert_eq!(result, "Response");
    assert!(elapsed >= Duration::from_millis(100));
}

#[tokio::test]
async fn test_mock_provider_with_error() {
    use aethecore::providers::mock::MockError;

    let provider = MockProvider::new("".to_string())
        .with_error(MockError::Network("Network error".to_string()));

    let result = provider.process("Input", None).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AetherError::NetworkError(_)));
}

#[tokio::test]
async fn test_timeout_handling() {
    use aethecore::providers::mock::MockError;

    let provider = MockProvider::new("".to_string()).with_error(MockError::Timeout);

    let result = provider.process("Input", None).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), AetherError::Timeout));
}

#[test]
fn test_config_validation_comprehensive() {
    let mut config = create_test_config();

    // Valid config should pass
    assert!(config.validate().is_ok());

    // Missing provider in rule
    config.rules.push(RoutingRuleConfig {
        regex: "test".to_string(),
        provider: "nonexistent".to_string(),
        system_prompt: None,
    });
    assert!(config.validate().is_err());

    // Reset rules
    config.rules.pop();

    // Invalid regex
    config.rules.push(RoutingRuleConfig {
        regex: "[invalid(".to_string(),
        provider: "openai".to_string(),
        system_prompt: None,
    });
    assert!(config.validate().is_err());
}

#[test]
fn test_memory_config_in_router() {
    let mut config = create_test_config();

    // Memory enabled
    config.memory.enabled = true;
    assert!(config.validate().is_ok());

    // Memory disabled
    config.memory.enabled = false;
    assert!(config.validate().is_ok());
}

#[test]
fn test_config_file_load_and_validate() {
    use std::fs;

    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");

    // Write valid config
    let toml_content = r##"
default_hotkey = "Command+Grave"

[general]
default_provider = "openai"

[providers.openai]
api_key = "sk-test"
model = "gpt-4o"
color = "#10a37f"
timeout_seconds = 30

[[rules]]
regex = ".*"
provider = "openai"

[memory]
enabled = true
"##;

    fs::write(&config_path, toml_content).unwrap();

    // Load and validate
    let config = Config::load_from_file(&config_path).unwrap();
    assert_eq!(config.general.default_provider, Some("openai".to_string()));
    assert!(config.memory.enabled);
}

#[test]
fn test_config_file_not_found() {
    let result = Config::load_from_file("/nonexistent/config.toml");
    assert!(result.is_err());
}

#[test]
fn test_config_file_invalid_toml() {
    use std::fs;

    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");

    // Write invalid TOML
    fs::write(&config_path, "invalid [[ toml").unwrap();

    let result = Config::load_from_file(&config_path);
    assert!(result.is_err());
}

#[test]
fn test_multiple_providers_same_type() {
    let mut config = Config::default();

    // Add multiple OpenAI-compatible providers
    config.providers.insert(
        "openai".to_string(),
        ProviderConfig {
            provider_type: Some("openai".to_string()),
            api_key: Some("key1".to_string()),
            model: "gpt-4o".to_string(),
            base_url: None,
            color: "#10a37f".to_string(),
            timeout_seconds: 30,
            max_tokens: None,
            temperature: None,
        },
    );

    config.providers.insert(
        "deepseek".to_string(),
        ProviderConfig {
            provider_type: Some("openai".to_string()),
            api_key: Some("key2".to_string()),
            model: "deepseek-chat".to_string(),
            base_url: Some("https://api.deepseek.com".to_string()),
            color: "#0066cc".to_string(),
            timeout_seconds: 30,
            max_tokens: None,
            temperature: None,
        },
    );

    config.rules.push(RoutingRuleConfig {
        regex: "^/deep".to_string(),
        provider: "deepseek".to_string(),
        system_prompt: None,
    });

    config.rules.push(RoutingRuleConfig {
        regex: ".*".to_string(),
        provider: "openai".to_string(),
        system_prompt: None,
    });

    // Should validate successfully
    assert!(config.validate().is_ok());
}

#[test]
fn test_provider_type_inference() {
    let config = ProviderConfig {
        provider_type: None,
        api_key: Some("key".to_string()),
        model: "test".to_string(),
        base_url: None,
        color: "#000000".to_string(),
        timeout_seconds: 30,
        max_tokens: None,
        temperature: None,
    };

    // Should infer "openai" from provider name "openai"
    assert_eq!(config.infer_provider_type("openai"), "openai");

    // Should infer "claude" from provider name containing "claude"
    assert_eq!(config.infer_provider_type("claude"), "claude");
    assert_eq!(config.infer_provider_type("my-claude-api"), "claude");

    // Should infer "ollama" from provider name containing "ollama"
    assert_eq!(config.infer_provider_type("ollama"), "ollama");
    assert_eq!(config.infer_provider_type("local-ollama"), "ollama");

    // Should default to "openai" for unknown names
    assert_eq!(config.infer_provider_type("deepseek"), "openai");
    assert_eq!(config.infer_provider_type("moonshot"), "openai");
}
