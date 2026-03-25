/// Ollama local LLM client implementation
///
/// Implements the `AiProvider` trait for locally-hosted Ollama models.
/// Uses Ollama's HTTP API for text and vision capabilities.
///
/// # Configuration
///
/// Required fields:
/// - `model`: Model name (e.g., "llama3.2", "llava", "bakllava")
///
/// Optional fields:
/// - `base_url`: Ollama server URL (defaults to "http://localhost:11434")
/// - `timeout_seconds`: Request timeout (defaults to 60)
///
/// # Prerequisites
///
/// Ollama must be installed and running:
/// - macOS/Linux: `curl -fsSL https://ollama.ai/install.sh | sh`
/// - Manual: https://ollama.ai/download
///
/// Models must be pulled before use:
/// ```bash
/// ollama pull llama3.2
/// ollama pull llava  # For vision support
/// ```
///
/// # Vision Support
///
/// Vision-capable models (llava, bakllava, etc.) can process images
/// when using `process_with_attachments()`.
///
/// # Example
///
/// ```rust,ignore
/// use alephcore::config::ProviderConfig;
/// use alephcore::providers::ollama::OllamaProvider;
/// use alephcore::providers::AiProvider;
///
/// # async fn example() -> alephcore::error::Result<()> {
/// let config = ProviderConfig {
///     api_key: None, // Not needed for Ollama
///     model: "llama3.2".to_string(),
///     base_url: Some("http://localhost:11434".to_string()),
///     color: "#0000ff".to_string(),
///     timeout_seconds: 60,
///     max_tokens: None,
///     temperature: None,
/// };
///
/// let provider = OllamaProvider::new("ollama".to_string(), config)?;
/// let response = provider.process("Hello!", Some("You are helpful")).await?;
/// println!("Response: {}", response);
/// # Ok(())
/// # }
/// ```
use crate::config::ProviderConfig;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::message::UnifiedMessage;
use crate::providers::AiProvider;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// Default Ollama server URL
const DEFAULT_OLLAMA_URL: &str = "http://localhost:11434";

/// Ollama local provider
pub struct OllamaProvider {
    /// Provider name (e.g., "ollama", "ollama-llama3")
    name: String,
    /// Model name (e.g., "llama3.2", "llava")
    model: String,
    /// HTTP client
    client: Client,
    /// API endpoint
    endpoint: String,
    /// Provider brand color
    color: String,
    /// Provider config for advanced options
    config: ProviderConfig,
}

/// Request body for Ollama /api/generate endpoint
#[derive(Debug, Serialize)]
struct GenerateRequest {
    model: String,
    prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    images: Option<Vec<String>>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<GenerateOptions>,
}

/// Optional generation parameters
#[derive(Debug, Serialize)]
struct GenerateOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_predict: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repeat_penalty: Option<f32>,
}

/// Response from Ollama /api/generate endpoint
#[derive(Debug, Deserialize)]
struct GenerateResponse {
    response: String,
    #[allow(dead_code)] // Deserialized from API response
    done: bool,
}

/// Error response from Ollama API
#[derive(Debug, Deserialize)]
struct OllamaError {
    error: String,
}

/// Response from Ollama /api/tags endpoint
#[derive(Debug, Deserialize)]
pub(crate) struct TagsResponse {
    pub(crate) models: Vec<OllamaModelInfo>,
}

/// Model info from Ollama tags
#[derive(Debug, Deserialize)]
pub(crate) struct OllamaModelInfo {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) size: Option<u64>,
    #[serde(default)]
    pub(crate) modified_at: Option<String>,
}

impl OllamaProvider {
    /// Create new Ollama provider
    ///
    /// # Arguments
    ///
    /// * `name` - Provider name
    /// * `config` - Provider configuration with model name
    ///
    /// # Returns
    ///
    /// * `Ok(OllamaProvider)` - Successfully initialized provider
    /// * `Err(AlephError)` - Configuration validation failed
    ///
    /// # Errors
    ///
    /// Returns `InvalidConfig` if:
    /// - Model name is empty
    /// - Timeout is zero
    pub fn new(name: String, config: ProviderConfig) -> Result<Self> {
        if config.default_model().is_empty() {
            return Err(AlephError::invalid_config("Model name cannot be empty"));
        }

        if config.timeout_seconds == 0 {
            return Err(AlephError::invalid_config(
                "Timeout must be greater than zero",
            ));
        }

        // Build HTTP client with timeout
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds))
            .build()
            .map_err(|e| {
                AlephError::invalid_config(format!("Failed to build HTTP client: {}", e))
            })?;

        // Build API endpoint
        let base_url = config
            .base_url
            .as_ref()
            .map(|s| s.trim_end_matches('/').to_string())
            .unwrap_or_else(|| DEFAULT_OLLAMA_URL.to_string());
        let endpoint = format!("{}/api/generate", base_url);

        info!(
            model = %config.default_model(),
            endpoint = %endpoint,
            timeout_seconds = config.timeout_seconds,
            "Ollama provider initialized successfully (HTTP API)"
        );

        Ok(Self {
            name,
            model: config.default_model().to_string(),
            client,
            endpoint,
            color: config.color.clone(),
            config,
        })
    }

    /// Build generate options from config
    fn build_options(&self) -> Option<GenerateOptions> {
        if self.config.temperature.is_some()
            || self.config.max_tokens.is_some()
            || self.config.repeat_penalty.is_some()
        {
            Some(GenerateOptions {
                temperature: self.config.temperature,
                num_predict: self.config.max_tokens,
                repeat_penalty: self.config.repeat_penalty,
            })
        } else {
            None
        }
    }

    /// Build request for text-only generation
    fn build_request(&self, input: &str, system_prompt: Option<&str>) -> GenerateRequest {
        GenerateRequest {
            model: self.model.clone(),
            prompt: input.to_string(),
            system: system_prompt.map(|s| s.to_string()),
            images: None,
            stream: false,
            options: self.build_options(),
        }
    }


    /// Handle error response from Ollama
    async fn handle_error(&self, response: reqwest::Response) -> AlephError {
        let status = response.status();

        // Try to parse error response
        if let Ok(error_response) = response.json::<OllamaError>().await {
            let error_msg = error_response.error;

            // Check for specific error patterns
            if error_msg.contains("model") && error_msg.contains("not found") {
                error!(model = %self.model, "Ollama model not found");
                return AlephError::provider(format!(
                    "Ollama model '{}' not found. Run: ollama pull {}",
                    self.model, self.model
                ));
            }

            error!(status = %status, error = %error_msg, "Ollama API error");
            return AlephError::provider(format!("Ollama error: {}", error_msg));
        }

        // Fallback error
        error!(status = %status, "Ollama request failed");
        AlephError::provider(format!("Ollama request failed: {}", status))
    }
}

impl AiProvider for OllamaProvider {
    fn process(
        &self,
        payload: RequestPayload<'_>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + '_>> {
        // Extract text from last user message (Ollama is text-only)
        let input = payload
            .messages
            .iter()
            .rev()
            .find_map(|m| match m {
                UnifiedMessage::User { content } => content.iter().find_map(|b| b.as_text()),
                _ => None,
            })
            .unwrap_or("")
            .to_string();
        let system_prompt = payload.system_prompt.map(|s| s.to_string());

        Box::pin(async move {
            debug!(
                model = %self.model,
                input_length = input.len(),
                has_system_prompt = system_prompt.is_some(),
                "Sending request to Ollama"
            );

            let request_body = self.build_request(&input, system_prompt.as_deref());

            let response = self
                .client
                .post(&self.endpoint)
                .header("Content-Type", "application/json")
                .json(&request_body)
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        error!("Ollama request timed out");
                        AlephError::Timeout {
                            suggestion: Some(
                                "The Ollama model is taking too long. Try a smaller model or increase the timeout.".to_string(),
                            ),
                        }
                    } else if e.is_connect() {
                        error!(error = %e, "Failed to connect to Ollama");
                        AlephError::network(format!(
                            "Failed to connect to Ollama at {}. Is Ollama running?",
                            self.endpoint
                        ))
                    } else {
                        error!(error = %e, "Ollama network error");
                        AlephError::network(format!("Network error: {}", e))
                    }
                })?;

            if !response.status().is_success() {
                return Err(self.handle_error(response).await);
            }

            let generate_response: GenerateResponse = response.json().await.map_err(|e| {
                error!(error = %e, "Failed to parse Ollama response");
                AlephError::provider(format!("Failed to parse Ollama response: {}", e))
            })?;

            let text = generate_response.response.trim().to_string();

            if text.is_empty() {
                warn!(model = %self.model, "Ollama returned empty response");
                return Err(AlephError::provider("Ollama returned empty response"));
            }

            info!(
                model = %self.model,
                response_length = text.len(),
                "Ollama request completed successfully"
            );

            Ok(ProviderResponse::text_only(text))
        })
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn color(&self) -> &str {
        &self.color
    }
}

#[async_trait::async_trait]
impl crate::providers::model_discovery::ModelDiscovery for OllamaProvider {
    fn provider_name(&self) -> &str {
        &self.name
    }

    async fn discover_models(&self) -> anyhow::Result<Vec<crate::providers::model_discovery::DiscoveredModel>> {
        use crate::providers::model_discovery::DiscoveredModel;

        // Derive base URL from the /api/generate endpoint
        let base_url = self.endpoint.trim_end_matches("/api/generate");
        let url = format!("{}/api/tags", base_url);

        let resp: TagsResponse = self
            .client
            .get(&url)
            .send()
            .await?
            .json()
            .await?;

        Ok(resp
            .models
            .into_iter()
            .map(|m| DiscoveredModel {
                id: m.name.clone(),
                display_name: Some(m.name),
                size_bytes: m.size,
                modified_at: m.modified_at,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> ProviderConfig {
        let mut config = ProviderConfig::test_config("llama3.2");
        config.api_key = None; // Not needed for Ollama
        config.color = "#0000ff".to_string(); // Ollama brand color
        config.timeout_seconds = 60;
        config
    }

    #[test]
    fn test_new_provider_success() {
        let config = create_test_config();
        let provider = OllamaProvider::new("ollama".to_string(), config);
        assert!(provider.is_ok());
    }

    #[test]
    fn test_new_provider_empty_model() {
        let mut config = create_test_config();
        config.models = vec!["".to_string()];
        let result = OllamaProvider::new("ollama".to_string(), config);
        assert!(matches!(result, Err(AlephError::InvalidConfig { .. })));
    }

    #[test]
    fn test_new_provider_zero_timeout() {
        let mut config = create_test_config();
        config.timeout_seconds = 0;
        let result = OllamaProvider::new("ollama".to_string(), config);
        assert!(matches!(result, Err(AlephError::InvalidConfig { .. })));
    }

    #[test]
    fn test_default_endpoint() {
        let config = create_test_config();
        let provider = OllamaProvider::new("ollama".to_string(), config).unwrap();
        assert_eq!(provider.endpoint, "http://localhost:11434/api/generate");
    }

    #[test]
    fn test_custom_endpoint() {
        let mut config = create_test_config();
        config.base_url = Some("http://192.168.1.100:11434".to_string());
        let provider = OllamaProvider::new("ollama".to_string(), config).unwrap();
        assert_eq!(provider.endpoint, "http://192.168.1.100:11434/api/generate");
    }

    #[test]
    fn test_provider_metadata() {
        let config = create_test_config();
        let provider = OllamaProvider::new("ollama".to_string(), config).unwrap();

        assert_eq!(provider.name(), "ollama");
        assert_eq!(provider.color(), "#0000ff");
    }

    #[test]
    fn test_build_request() {
        let config = create_test_config();
        let provider = OllamaProvider::new("ollama".to_string(), config).unwrap();

        let request = provider.build_request("Hello", None);
        assert_eq!(request.model, "llama3.2");
        assert_eq!(request.prompt, "Hello");
        assert!(request.system.is_none());
        assert!(request.images.is_none());
        assert!(!request.stream);

        let request_with_system = provider.build_request("Hello", Some("Be helpful"));
        assert_eq!(request_with_system.system, Some("Be helpful".to_string()));
    }

    #[test]
    fn test_build_options() {
        let mut config = create_test_config();
        config.temperature = Some(0.8);
        config.max_tokens = Some(1000);
        let provider = OllamaProvider::new("ollama".to_string(), config).unwrap();

        let options = provider.build_options();
        assert!(options.is_some());
        let opts = options.unwrap();
        assert_eq!(opts.temperature, Some(0.8));
        assert_eq!(opts.num_predict, Some(1000));
    }

    #[test]
    fn parse_tags_response_success() {
        let json = r#"{"models": [{"name": "llama3:latest"}, {"name": "codellama:7b"}]}"#;
        let tags: TagsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(tags.models.len(), 2);
        assert_eq!(tags.models[0].name, "llama3:latest");
        assert_eq!(tags.models[1].name, "codellama:7b");
    }

    #[test]
    fn parse_tags_response_empty() {
        let json = r#"{"models": []}"#;
        let tags: TagsResponse = serde_json::from_str(json).unwrap();
        assert!(tags.models.is_empty());
    }

    #[test]
    fn parse_tags_response_vision_model_detection() {
        let json = r#"{"models": [{"name": "llava:latest"}, {"name": "bakllava:7b"}]}"#;
        let tags: TagsResponse = serde_json::from_str(json).unwrap();
        assert!(tags.models[0].name.contains("llava"));
        assert!(tags.models[1].name.contains("llava"));
    }
}
