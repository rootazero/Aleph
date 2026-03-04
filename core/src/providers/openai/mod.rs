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
/// - `timeout_seconds`: Request timeout (defaults to 300)
/// - `max_tokens`: Maximum response tokens
/// - `temperature`: Response randomness (0.0-2.0)
///
/// # Example
///
/// ```rust,ignore
/// use alephcore::config::ProviderConfig;
/// use alephcore::providers::openai::OpenAiProvider;
/// use alephcore::providers::AiProvider;
///
/// # async fn example() -> alephcore::error::Result<()> {
/// let config = ProviderConfig {
///     api_key: Some("sk-...".to_string()),
///     model: "gpt-4o".to_string(),
///     base_url: None,
///     color: "#10a37f".to_string(),
///     timeout_seconds: 300,
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

pub mod request;
pub mod types;

// Re-export types for external use (used by protocols/openai.rs)
pub use types::{
    ChatCompletionRequest, ChatCompletionResponse, Choice, ContentBlock, ErrorDetails,
    ErrorResponse, ImageUrl, Message, MessageContent, OpenAiFunction, OpenAiFunctionCall,
    OpenAiTool, OpenAiToolCall, OpenAiUsage, ResponseMessage,
};

// Note: Provider tests removed - use protocols/openai tests instead
