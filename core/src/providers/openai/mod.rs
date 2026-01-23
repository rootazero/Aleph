/// OpenAI API client implementation
///
/// Implements the `AiProvider` trait for OpenAI's chat completion API.
/// Supports GPT-4o, GPT-4o-mini, and other chat models.
///
/// # Configuration
///
/// Required fields:
/// - `api_key`: OpenAI API key (from https://platform.openai.com)
/// - `model`: Model name (e.g., "gpt-4o", "gpt-4o-mini")
///
/// Optional fields:
/// - `base_url`: Custom API endpoint (defaults to "https://api.openai.com/v1")
/// - `timeout_seconds`: Request timeout (defaults to 30)
/// - `max_tokens`: Maximum response tokens
/// - `temperature`: Response randomness (0.0-2.0)
///
/// # Example
///
/// ```rust,ignore
/// use aethecore::config::ProviderConfig;
/// use aethecore::providers::openai::OpenAiProvider;
/// use aethecore::providers::AiProvider;
///
/// # async fn example() -> aethecore::error::Result<()> {
/// let config = ProviderConfig {
///     api_key: Some("sk-...".to_string()),
///     model: "gpt-4o".to_string(),
///     base_url: None,
///     color: "#10a37f".to_string(),
///     timeout_seconds: 30,
///     max_tokens: Some(4096),
///     temperature: Some(0.7),
/// };
///
/// let provider = OpenAiProvider::new(config)?;
/// let response = provider.process("Hello!", Some("You are helpful")).await?;
/// println!("Response: {}", response);
/// # Ok(())
/// # }
/// ```

mod error;
mod provider;
mod request;
mod types;

// Re-export the main provider struct
pub use provider::OpenAiProvider;

// Re-export types for external use if needed
pub use types::{
    ChatCompletionRequest, ChatCompletionResponse, Choice, ContentBlock, ErrorDetails,
    ErrorResponse, ImageUrl, Message, MessageContent, ResponseMessage,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderConfig;
    use crate::core::MediaAttachment;
    use crate::error::AetherError;

    fn create_test_config() -> ProviderConfig {
        let mut config = ProviderConfig::test_config("gpt-4o");
        config.color = "#10a37f".to_string(); // OpenAI brand color
        config.max_tokens = Some(1000);
        config.temperature = Some(0.7);
        config
    }

    #[test]
    fn test_new_provider_success() {
        let config = create_test_config();
        let provider = OpenAiProvider::new("openai".to_string(), config);
        assert!(provider.is_ok());
    }

    #[test]
    fn test_new_provider_missing_api_key() {
        let mut config = create_test_config();
        config.api_key = None;
        let result = OpenAiProvider::new("openai".to_string(), config);
        assert!(matches!(result, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_new_provider_empty_api_key() {
        let mut config = create_test_config();
        config.api_key = Some("".to_string());
        let result = OpenAiProvider::new("openai".to_string(), config);
        assert!(matches!(result, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_new_provider_empty_model() {
        let mut config = create_test_config();
        config.model = "".to_string();
        let result = OpenAiProvider::new("openai".to_string(), config);
        assert!(matches!(result, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_new_provider_zero_timeout() {
        let mut config = create_test_config();
        config.timeout_seconds = 0;
        let result = OpenAiProvider::new("openai".to_string(), config);
        assert!(matches!(result, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_build_request_without_system_prompt() {
        let config = create_test_config();
        let provider = OpenAiProvider::new("openai".to_string(), config.clone()).unwrap();

        let request = request::build_request(&config, "Hello", None);

        assert_eq!(request.model, "gpt-4o");
        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.messages[0].role, "user");
        // MessageContent is an enum, can't directly compare with string
        assert_eq!(request.max_tokens, Some(1000));
        assert_eq!(request.temperature, Some(0.7));

        // Suppress unused variable warning
        let _ = provider;
    }

    #[test]
    fn test_build_request_with_system_prompt() {
        // Use standard mode to get separate system + user messages
        let mut config = create_test_config();
        config.system_prompt_mode = Some("standard".to_string());
        let provider = OpenAiProvider::new("openai".to_string(), config.clone()).unwrap();

        let request = request::build_request(&config, "Hello", Some("You are a helpful assistant"));

        assert_eq!(request.messages.len(), 2);
        assert_eq!(request.messages[0].role, "system");
        assert_eq!(request.messages[1].role, "user");
        // MessageContent is an enum, can't directly compare with string

        // Suppress unused variable warning
        let _ = provider;
    }

    #[test]
    fn test_provider_metadata() {
        use crate::providers::AiProvider;

        let config = create_test_config();
        let provider = OpenAiProvider::new("openai".to_string(), config).unwrap();

        assert_eq!(provider.name(), "openai");
        assert_eq!(provider.color(), "#10a37f");
    }

    #[test]
    fn test_custom_base_url() {
        let mut config = create_test_config();
        config.base_url = Some("https://custom.openai.com/v1/".to_string());

        let provider = OpenAiProvider::new("openai".to_string(), config).unwrap();
        assert_eq!(
            provider.endpoint(),
            "https://custom.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn test_default_base_url() {
        let config = create_test_config();
        let provider = OpenAiProvider::new("openai".to_string(), config).unwrap();
        assert_eq!(
            provider.endpoint(),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn test_url_normalization_without_v1() {
        // User provides URL without /v1 - should still work
        let mut config = create_test_config();
        config.base_url = Some("https://ai.t8star.cn".to_string());

        let provider = OpenAiProvider::new("openai".to_string(), config).unwrap();
        assert_eq!(
            provider.endpoint(),
            "https://ai.t8star.cn/v1/chat/completions"
        );
    }

    #[test]
    fn test_url_normalization_with_v1() {
        // User provides URL with /v1 - should NOT produce duplicate /v1
        let mut config = create_test_config();
        config.base_url = Some("https://ai.t8star.cn/v1".to_string());

        let provider = OpenAiProvider::new("openai".to_string(), config).unwrap();
        assert_eq!(
            provider.endpoint(),
            "https://ai.t8star.cn/v1/chat/completions"
        );
    }

    #[test]
    fn test_url_normalization_with_trailing_slash() {
        let mut config = create_test_config();
        config.base_url = Some("https://api.example.com/".to_string());

        let provider = OpenAiProvider::new("openai".to_string(), config).unwrap();
        assert_eq!(
            provider.endpoint(),
            "https://api.example.com/v1/chat/completions"
        );
    }

    #[test]
    fn test_url_normalization_with_v1_and_trailing_slash() {
        let mut config = create_test_config();
        config.base_url = Some("https://api.example.com/v1/".to_string());

        let provider = OpenAiProvider::new("openai".to_string(), config).unwrap();
        assert_eq!(
            provider.endpoint(),
            "https://api.example.com/v1/chat/completions"
        );
    }

    #[test]
    fn test_multimodal_request_json_format() {
        let config = create_test_config();
        let provider = OpenAiProvider::new("openai".to_string(), config.clone()).unwrap();

        let attachments = vec![MediaAttachment {
            media_type: "image".to_string(),
            mime_type: "image/png".to_string(),
            data: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==".to_string(),
            encoding: "base64".to_string(),
            filename: Some("test.png".to_string()),
            size_bytes: 100,
        }];

        let request =
            request::build_multimodal_request(&config, "What's in this image?", &attachments, None);

        // Serialize to JSON and verify format
        let json = serde_json::to_string_pretty(&request).unwrap();
        println!("Multimodal request JSON:\n{}", json);

        // Parse JSON to verify structure
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        // Verify model
        assert_eq!(parsed["model"], "gpt-4o");

        // Verify messages structure
        let messages = parsed["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);

        // Verify user message has content array
        let content = messages[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);

        // Verify text block
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "What's in this image?");

        // Verify image_url block
        assert_eq!(content[1]["type"], "image_url");
        assert!(content[1]["image_url"]["url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));
        assert_eq!(content[1]["image_url"]["detail"], "auto");

        // Suppress unused variable warning
        let _ = provider;
    }

    #[test]
    fn test_v3_api_detection() {
        // Test that v3 API is correctly detected and formatted
        let mut config = create_test_config();
        config.base_url = Some("https://ark.cn-beijing.volces.com/api/v3".to_string());

        let provider = OpenAiProvider::new("volcengine".to_string(), config).unwrap();
        assert_eq!(
            provider.endpoint(),
            "https://ark.cn-beijing.volces.com/api/v3/chat/completions"
        );
    }

    #[test]
    fn test_default_to_openai() {
        // Test that missing base_url defaults to OpenAI
        let config = create_test_config();
        let provider = OpenAiProvider::new("openai".to_string(), config).unwrap();
        assert_eq!(
            provider.endpoint(),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    // Note: Integration tests with real API calls should be in tests/ directory
    // and gated behind a feature flag or environment variable
}
