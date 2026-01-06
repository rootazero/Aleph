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
/// ```rust,no_run
/// use aethecore::config::ProviderConfig;
/// use aethecore::providers::ollama::OllamaProvider;
/// use aethecore::providers::AiProvider;
///
/// # async fn example() -> aethecore::error::Result<()> {
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
use crate::error::{AetherError, Result};
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
    #[allow(dead_code)]
    done: bool,
}

/// Error response from Ollama API
#[derive(Debug, Deserialize)]
struct OllamaError {
    error: String,
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
    /// * `Err(AetherError)` - Configuration validation failed
    ///
    /// # Errors
    ///
    /// Returns `InvalidConfig` if:
    /// - Model name is empty
    /// - Timeout is zero
    pub fn new(name: String, config: ProviderConfig) -> Result<Self> {
        if config.model.is_empty() {
            return Err(AetherError::invalid_config("Model name cannot be empty"));
        }

        if config.timeout_seconds == 0 {
            return Err(AetherError::invalid_config(
                "Timeout must be greater than zero",
            ));
        }

        // Build HTTP client with timeout
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds))
            .build()
            .map_err(|e| {
                AetherError::invalid_config(format!("Failed to build HTTP client: {}", e))
            })?;

        // Build API endpoint
        let base_url = config
            .base_url
            .as_ref()
            .map(|s| s.trim_end_matches('/').to_string())
            .unwrap_or_else(|| DEFAULT_OLLAMA_URL.to_string());
        let endpoint = format!("{}/api/generate", base_url);

        info!(
            model = %config.model,
            endpoint = %endpoint,
            timeout_seconds = config.timeout_seconds,
            "Ollama provider initialized successfully (HTTP API)"
        );

        Ok(Self {
            name,
            model: config.model.clone(),
            client,
            endpoint,
            color: config.color.clone(),
            config,
        })
    }

    /// Check if the model supports vision
    ///
    /// Vision-capable models include llava, bakllava, and similar.
    fn is_vision_model(&self) -> bool {
        let model_lower = self.model.to_lowercase();
        model_lower.contains("llava")
            || model_lower.contains("bakllava")
            || model_lower.contains("vision")
            || model_lower.contains("moondream")
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

    /// Build request with images for vision generation
    fn build_multimodal_request(
        &self,
        input: &str,
        attachments: &[crate::core::MediaAttachment],
        system_prompt: Option<&str>,
    ) -> GenerateRequest {
        // Separate images and documents
        let images: Vec<String> = attachments
            .iter()
            .filter(|a| a.media_type == "image")
            .map(|a| a.data.clone()) // Already base64 encoded
            .collect();

        let documents: Vec<_> = attachments
            .iter()
            .filter(|a| a.media_type == "document")
            .collect();

        // Build document context (prepend to user input)
        let doc_context = if !documents.is_empty() {
            documents
                .iter()
                .map(|d| {
                    let name = d.filename.as_deref().unwrap_or("document");
                    format!("--- {} ---\n{}", name, d.data)
                })
                .collect::<Vec<_>>()
                .join("\n\n")
        } else {
            String::new()
        };

        // Build final input with document context
        let full_input = if doc_context.is_empty() {
            input.to_string()
        } else {
            format!("{}\n\n{}", doc_context, input)
        };

        GenerateRequest {
            model: self.model.clone(),
            prompt: if full_input.is_empty() {
                "Describe this image in detail.".to_string()
            } else {
                full_input
            },
            system: system_prompt.map(|s| s.to_string()),
            images: if images.is_empty() {
                None
            } else {
                Some(images)
            },
            stream: false,
            options: self.build_options(),
        }
    }

    /// Handle error response from Ollama
    async fn handle_error(&self, response: reqwest::Response) -> AetherError {
        let status = response.status();

        // Try to parse error response
        if let Ok(error_response) = response.json::<OllamaError>().await {
            let error_msg = error_response.error;

            // Check for specific error patterns
            if error_msg.contains("model") && error_msg.contains("not found") {
                error!(model = %self.model, "Ollama model not found");
                return AetherError::provider(format!(
                    "Ollama model '{}' not found. Run: ollama pull {}",
                    self.model, self.model
                ));
            }

            error!(status = %status, error = %error_msg, "Ollama API error");
            return AetherError::provider(format!("Ollama error: {}", error_msg));
        }

        // Fallback error
        error!(status = %status, "Ollama request failed");
        AetherError::provider(format!("Ollama request failed: {}", status))
    }
}

impl AiProvider for OllamaProvider {
    fn process(
        &self,
        input: &str,
        system_prompt: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        let input = input.to_string();
        let system_prompt = system_prompt.map(|s| s.to_string());

        Box::pin(async move {
            debug!(
                model = %self.model,
                input_length = input.len(),
                has_system_prompt = system_prompt.is_some(),
                "Sending request to Ollama"
            );

            // Build request body
            let request_body = self.build_request(&input, system_prompt.as_deref());

            // Send POST request
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
                        AetherError::Timeout {
                            suggestion: Some(
                                "The Ollama model is taking too long. Try a smaller model or increase the timeout.".to_string(),
                            ),
                        }
                    } else if e.is_connect() {
                        error!(error = %e, "Failed to connect to Ollama");
                        AetherError::network(format!(
                            "Failed to connect to Ollama at {}. Is Ollama running?",
                            self.endpoint
                        ))
                    } else {
                        error!(error = %e, "Ollama network error");
                        AetherError::network(format!("Network error: {}", e))
                    }
                })?;

            // Check status code
            if !response.status().is_success() {
                return Err(self.handle_error(response).await);
            }

            // Parse response
            let generate_response: GenerateResponse = response.json().await.map_err(|e| {
                error!(error = %e, "Failed to parse Ollama response");
                AetherError::provider(format!("Failed to parse Ollama response: {}", e))
            })?;

            let text = generate_response.response.trim().to_string();

            if text.is_empty() {
                warn!(model = %self.model, "Ollama returned empty response");
                return Err(AetherError::provider("Ollama returned empty response"));
            }

            info!(
                model = %self.model,
                response_length = text.len(),
                "Ollama request completed successfully"
            );

            Ok(text)
        })
    }

    fn process_with_attachments(
        &self,
        input: &str,
        attachments: Option<&[crate::core::MediaAttachment]>,
        system_prompt: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        let input = input.to_string();
        let attachments = attachments.map(|a| a.to_vec());
        let system_prompt = system_prompt.map(|s| s.to_string());

        Box::pin(async move {
            // Check if we have any attachments (images or documents)
            let Some(all_attachments) = attachments.as_ref() else {
                return self.process(&input, system_prompt.as_deref()).await;
            };

            let image_count = all_attachments
                .iter()
                .filter(|a| a.media_type == "image")
                .count();
            let document_count = all_attachments
                .iter()
                .filter(|a| a.media_type == "document")
                .count();

            // If no useful attachments, fall back to text-only
            if image_count == 0 && document_count == 0 {
                return self.process(&input, system_prompt.as_deref()).await;
            }

            // Check if model supports vision (only needed for images)
            if image_count > 0 && !self.is_vision_model() {
                warn!(
                    model = %self.model,
                    "Model does not support vision, falling back to text-only with documents"
                );
                // Still process documents even if vision is not supported
                if document_count == 0 {
                    return self.process(&input, system_prompt.as_deref()).await;
                }
            }

            debug!(
                model = %self.model,
                input_length = input.len(),
                image_count = image_count,
                document_count = document_count,
                has_system_prompt = system_prompt.is_some(),
                "Sending multimodal request to Ollama"
            );

            // Build multimodal request body
            let request_body =
                self.build_multimodal_request(&input, all_attachments, system_prompt.as_deref());

            // Send POST request
            let response = self
                .client
                .post(&self.endpoint)
                .header("Content-Type", "application/json")
                .json(&request_body)
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        error!("Ollama multimodal request timed out");
                        AetherError::Timeout {
                            suggestion: Some(
                                "Vision processing is slow. Try increasing the timeout.".to_string(),
                            ),
                        }
                    } else if e.is_connect() {
                        error!(error = %e, "Failed to connect to Ollama");
                        AetherError::network(format!(
                            "Failed to connect to Ollama at {}. Is Ollama running?",
                            self.endpoint
                        ))
                    } else {
                        error!(error = %e, "Ollama network error");
                        AetherError::network(format!("Network error: {}", e))
                    }
                })?;

            // Check status code
            if !response.status().is_success() {
                return Err(self.handle_error(response).await);
            }

            // Parse response
            let generate_response: GenerateResponse = response.json().await.map_err(|e| {
                error!(error = %e, "Failed to parse Ollama multimodal response");
                AetherError::provider(format!("Failed to parse Ollama response: {}", e))
            })?;

            let text = generate_response.response.trim().to_string();

            if text.is_empty() {
                warn!(model = %self.model, "Ollama returned empty response");
                return Err(AetherError::provider("Ollama returned empty response"));
            }

            info!(
                model = %self.model,
                response_length = text.len(),
                "Ollama multimodal request completed successfully"
            );

            Ok(text)
        })
    }

    fn supports_vision(&self) -> bool {
        self.is_vision_model()
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn color(&self) -> &str {
        &self.color
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
        config.model = "".to_string();
        let result = OllamaProvider::new("ollama".to_string(), config);
        assert!(matches!(result, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_new_provider_zero_timeout() {
        let mut config = create_test_config();
        config.timeout_seconds = 0;
        let result = OllamaProvider::new("ollama".to_string(), config);
        assert!(matches!(result, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_default_endpoint() {
        let config = create_test_config();
        let provider = OllamaProvider::new("ollama".to_string(), config).unwrap();
        assert_eq!(
            provider.endpoint,
            "http://localhost:11434/api/generate"
        );
    }

    #[test]
    fn test_custom_endpoint() {
        let mut config = create_test_config();
        config.base_url = Some("http://192.168.1.100:11434".to_string());
        let provider = OllamaProvider::new("ollama".to_string(), config).unwrap();
        assert_eq!(
            provider.endpoint,
            "http://192.168.1.100:11434/api/generate"
        );
    }

    #[test]
    fn test_is_vision_model() {
        let config = create_test_config();
        let provider = OllamaProvider::new("ollama".to_string(), config).unwrap();
        assert!(!provider.is_vision_model());

        let mut vision_config = create_test_config();
        vision_config.model = "llava".to_string();
        let vision_provider = OllamaProvider::new("ollama".to_string(), vision_config).unwrap();
        assert!(vision_provider.is_vision_model());

        let mut bak_config = create_test_config();
        bak_config.model = "bakllava:latest".to_string();
        let bak_provider = OllamaProvider::new("ollama".to_string(), bak_config).unwrap();
        assert!(bak_provider.is_vision_model());
    }

    #[test]
    fn test_supports_vision() {
        let config = create_test_config();
        let provider = OllamaProvider::new("ollama".to_string(), config).unwrap();
        assert!(!provider.supports_vision());

        let mut vision_config = create_test_config();
        vision_config.model = "llava".to_string();
        let vision_provider = OllamaProvider::new("ollama".to_string(), vision_config).unwrap();
        assert!(vision_provider.supports_vision());
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
}
