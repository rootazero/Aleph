/// Google Gemini API client implementation
///
/// Implements the `AiProvider` trait for Google's Gemini API.
/// Supports Gemini Pro, Gemini 1.5 Pro, and Gemini 1.5 Flash models.
///
/// # Configuration
///
/// Required fields:
/// - `api_key`: Google AI API key (from https://aistudio.google.com)
/// - `model`: Model name (e.g., "gemini-1.5-pro", "gemini-1.5-flash")
///
/// Optional fields:
/// - `base_url`: Custom API endpoint (defaults to "https://generativelanguage.googleapis.com")
/// - `timeout_seconds`: Request timeout (defaults to 30)
/// - `max_tokens`: Maximum response tokens
/// - `temperature`: Response randomness (0.0-2.0)
use crate::config::ProviderConfig;
use crate::error::{AetherError, Result};
use crate::providers::shared::{
    build_document_context, combine_with_document_context, separate_attachments,
};
use crate::providers::AiProvider;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tracing::{debug, error, info};

/// Google Gemini API provider
pub struct GeminiProvider {
    /// Provider name
    name: String,
    /// HTTP client with configured timeout and TLS
    client: Client,
    /// Provider configuration
    config: ProviderConfig,
    /// API base URL
    base_url: String,
}

/// Request body for Gemini generateContent API
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerateContentRequest {
    contents: Vec<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GenerationConfig>,
}

/// Content structure for Gemini API
#[derive(Debug, Serialize)]
struct Content {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    parts: Vec<Part>,
}

/// Part can be text or inline image data
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum Part {
    /// Text content part
    Text { text: String },
    /// Inline image data part
    InlineData { inline_data: InlineData },
}

/// Inline data for images
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InlineData {
    /// MIME type (e.g., "image/png", "image/jpeg")
    mime_type: String,
    /// Base64-encoded image data (without data URI prefix)
    data: String,
}

/// Generation configuration
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_k: Option<u32>,
}

/// Response from Gemini generateContent API
#[derive(Debug, Deserialize)]
struct GenerateContentResponse {
    candidates: Option<Vec<Candidate>>,
    error: Option<GeminiError>,
}

#[derive(Debug, Deserialize)]
struct Candidate {
    content: CandidateContent,
}

#[derive(Debug, Deserialize)]
struct CandidateContent {
    parts: Vec<ResponsePart>,
}

#[derive(Debug, Deserialize)]
struct ResponsePart {
    text: String,
}

/// Error response from Gemini API
#[derive(Debug, Deserialize)]
struct GeminiError {
    code: i32,
    message: String,
    status: String,
}

impl GeminiProvider {
    /// Create new Gemini provider
    ///
    /// # Arguments
    ///
    /// * `name` - Provider name (e.g., "gemini")
    /// * `config` - Provider configuration with API key and model
    ///
    /// # Returns
    ///
    /// * `Ok(GeminiProvider)` - Successfully initialized provider
    /// * `Err(AetherError)` - Configuration validation failed
    pub fn new(name: String, config: ProviderConfig) -> Result<Self> {
        // Validate configuration
        let api_key = config
            .api_key
            .as_ref()
            .ok_or_else(|| AetherError::invalid_config("Gemini API key is required"))?;

        if api_key.is_empty() {
            return Err(AetherError::invalid_config(
                "Gemini API key cannot be empty",
            ));
        }

        if config.model.is_empty() {
            return Err(AetherError::invalid_config("Model name cannot be empty"));
        }

        if config.timeout_seconds == 0 {
            return Err(AetherError::invalid_config(
                "Timeout must be greater than zero",
            ));
        }

        // Build HTTP client with timeout and TLS
        // Use native TLS to trust system CA certificates (required for HTTPS interception like Kaspersky)
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds))
            .build()
            .map_err(|e| {
                AetherError::invalid_config(format!("Failed to build HTTP client: {}", e))
            })?;

        // Build API base URL
        let base_url = config
            .base_url
            .as_ref()
            .map(|s| s.trim_end_matches('/').to_string())
            .unwrap_or_else(|| "https://generativelanguage.googleapis.com".to_string());

        Ok(Self {
            name,
            client,
            config,
            base_url,
        })
    }

    /// Build API endpoint URL with model and API key
    fn build_endpoint(&self) -> String {
        let empty_key = String::new();
        let api_key = self.config.api_key.as_ref().unwrap_or(&empty_key);
        format!(
            "{}/v1beta/models/{}:generateContent?key={}",
            self.base_url, self.config.model, api_key
        )
    }

    /// Build request body for text-only generation
    fn build_request(&self, input: &str, system_prompt: Option<&str>) -> GenerateContentRequest {
        // Add user content
        let contents = vec![Content {
            role: Some("user".to_string()),
            parts: vec![Part::Text {
                text: input.to_string(),
            }],
        }];

        // Build system instruction if provided
        let system_instruction = system_prompt.map(|prompt| Content {
            role: None, // System instruction doesn't have a role
            parts: vec![Part::Text {
                text: prompt.to_string(),
            }],
        });

        // Build generation config
        let generation_config = Some(GenerationConfig {
            max_output_tokens: self.config.max_tokens,
            temperature: self.config.temperature,
            top_p: self.config.top_p,
            top_k: self.config.top_k,
        });

        GenerateContentRequest {
            contents,
            system_instruction,
            generation_config,
        }
    }

    /// Build request body with MediaAttachment for vision API
    fn build_multimodal_request(
        &self,
        input: &str,
        attachments: &[crate::core::MediaAttachment],
        system_prompt: Option<&str>,
    ) -> GenerateContentRequest {
        // Separate images and documents
        let (images, documents) = separate_attachments(attachments);

        // Build document context and combine with user input
        let doc_context = build_document_context(&documents);
        let full_input = combine_with_document_context(&doc_context, input);

        let mut parts = Vec::new();

        // Add text if not empty
        if !full_input.is_empty() {
            parts.push(Part::Text {
                text: full_input,
            });
        } else {
            // Default prompt for image-only requests
            parts.push(Part::Text {
                text: "Describe this image in detail.".to_string(),
            });
        }

        // Add images from MediaAttachment
        for attachment in images {
            // Gemini expects raw base64 data without data URI prefix
            // The data from MediaAttachment should already be clean base64
            let clean_data = if attachment.data.starts_with("data:") {
                // Strip data URI prefix if present
                attachment
                    .data
                    .split(',')
                    .nth(1)
                    .unwrap_or(&attachment.data)
                    .to_string()
            } else {
                attachment.data.clone()
            };

            parts.push(Part::InlineData {
                inline_data: InlineData {
                    mime_type: attachment.mime_type.clone(),
                    data: clean_data,
                },
            });
        }

        // Build system instruction if provided
        let system_instruction = system_prompt.map(|prompt| Content {
            role: None,
            parts: vec![Part::Text {
                text: prompt.to_string(),
            }],
        });

        // Build generation config
        let generation_config = Some(GenerationConfig {
            max_output_tokens: self.config.max_tokens.or(Some(4096)), // Higher for vision
            temperature: self.config.temperature,
            top_p: self.config.top_p,
            top_k: self.config.top_k,
        });

        GenerateContentRequest {
            contents: vec![Content {
                role: Some("user".to_string()),
                parts,
            }],
            system_instruction,
            generation_config,
        }
    }

    /// Parse error response and convert to AetherError
    fn handle_error(&self, error: GeminiError) -> AetherError {
        match error.code {
            400 => AetherError::provider(format!(
                "Gemini bad request: {}",
                error.message
            )),
            401 | 403 => AetherError::authentication(
                &self.name,
                &format!("Invalid Gemini API key: {}", error.message),
            ),
            429 => AetherError::rate_limit(format!("Gemini rate limit: {}", error.message)),
            500..=599 => AetherError::provider(format!(
                "Gemini server error ({}): {}",
                error.status, error.message
            )),
            _ => AetherError::provider(format!(
                "Gemini API error ({}): {}",
                error.status, error.message
            )),
        }
    }

    /// Parse HTTP error response
    async fn handle_http_error(&self, response: reqwest::Response) -> AetherError {
        let status = response.status();

        // Try to parse error response body
        if let Ok(error_response) = response.json::<GenerateContentResponse>().await {
            if let Some(error) = error_response.error {
                return self.handle_error(error);
            }
        }

        // Fallback if we can't parse the error response
        match status.as_u16() {
            401 | 403 => AetherError::authentication(
                self.name.clone(),
                "Invalid Gemini API key".to_string(),
            ),
            429 => AetherError::rate_limit("Gemini rate limit exceeded".to_string()),
            500..=599 => AetherError::provider(format!("Gemini server error: {}", status)),
            _ => AetherError::provider(format!("Gemini API error: {}", status)),
        }
    }
}

impl AiProvider for GeminiProvider {
    fn process(
        &self,
        input: &str,
        system_prompt: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        let input = input.to_string();
        let system_prompt = system_prompt.map(|s| s.to_string());

        Box::pin(async move {
            debug!(
                model = %self.config.model,
                input_length = input.len(),
                has_system_prompt = system_prompt.is_some(),
                "Sending request to Gemini"
            );

            // Build request body
            let request_body = self.build_request(&input, system_prompt.as_deref());

            // Build endpoint URL
            let endpoint = self.build_endpoint();

            // Send POST request
            let response = self
                .client
                .post(&endpoint)
                .header("Content-Type", "application/json")
                .json(&request_body)
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        error!("Gemini request timed out");
                        AetherError::Timeout {
                            suggestion: Some("The Gemini service is taking too long. Try again or switch providers.".to_string()),
                        }
                    } else if e.is_connect() {
                        error!(error = %e, "Failed to connect to Gemini");
                        AetherError::network(format!("Failed to connect to Gemini: {}", e))
                    } else {
                        error!(error = %e, "Gemini network error");
                        AetherError::network(format!("Network error: {}", e))
                    }
                })?;

            // Check status code
            if !response.status().is_success() {
                let status = response.status();
                debug!(status = %status, "Gemini request failed");
                return Err(self.handle_http_error(response).await);
            }

            // Parse response
            let gemini_response: GenerateContentResponse = response.json().await.map_err(|e| {
                error!(error = %e, "Failed to parse Gemini response");
                AetherError::provider(format!("Failed to parse Gemini response: {}", e))
            })?;

            // Check for error in response
            if let Some(error) = gemini_response.error {
                return Err(self.handle_error(error));
            }

            // Extract text from response
            let content = gemini_response
                .candidates
                .and_then(|candidates| candidates.into_iter().next())
                .and_then(|candidate| candidate.content.parts.into_iter().next())
                .map(|part| part.text)
                .ok_or_else(|| {
                    error!("Gemini returned no content");
                    AetherError::provider("No response from Gemini")
                })?;

            info!(
                response_length = content.len(),
                "Gemini request completed successfully"
            );

            Ok(content)
        })
    }

    fn supports_vision(&self) -> bool {
        // Gemini 1.5 models and gemini-pro-vision support vision
        let model = self.config.model.to_lowercase();
        model.contains("1.5") || model.contains("vision") || model.contains("2.0")
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

            // If only documents (no images), inject document content into text and use text-only request
            if image_count == 0 && document_count > 0 {
                let (_, documents) = separate_attachments(all_attachments);
                let doc_context = build_document_context(&documents);
                let full_input = combine_with_document_context(&doc_context, &input);

                debug!(
                    model = %self.config.model,
                    document_count = document_count,
                    full_input_length = full_input.len(),
                    "Sending document-only request as text to Gemini"
                );

                return self.process(&full_input, system_prompt.as_deref()).await;
            }

            debug!(
                model = %self.config.model,
                input_length = input.len(),
                image_count = image_count,
                document_count = document_count,
                has_system_prompt = system_prompt.is_some(),
                "Sending multimodal request to Gemini"
            );

            // Build multimodal request body (only when we have images)
            let request_body =
                self.build_multimodal_request(&input, all_attachments, system_prompt.as_deref());

            // Build endpoint URL
            let endpoint = self.build_endpoint();

            // Send POST request
            let response = self
                .client
                .post(&endpoint)
                .header("Content-Type", "application/json")
                .json(&request_body)
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        error!("Gemini multimodal request timed out");
                        AetherError::Timeout {
                            suggestion: Some(
                                "The Gemini service is taking too long. Try again or switch providers."
                                    .to_string(),
                            ),
                        }
                    } else if e.is_connect() {
                        error!(error = %e, "Failed to connect to Gemini");
                        AetherError::network(format!("Failed to connect to Gemini: {}", e))
                    } else {
                        error!(error = %e, "Gemini network error");
                        AetherError::network(format!("Network error: {}", e))
                    }
                })?;

            // Check status code
            if !response.status().is_success() {
                let status = response.status();
                debug!(status = %status, "Gemini multimodal request failed");
                return Err(self.handle_http_error(response).await);
            }

            // Parse response
            let gemini_response: GenerateContentResponse = response.json().await.map_err(|e| {
                error!(error = %e, "Failed to parse Gemini multimodal response");
                AetherError::provider(format!("Failed to parse Gemini response: {}", e))
            })?;

            // Check for error in response
            if let Some(error) = gemini_response.error {
                return Err(self.handle_error(error));
            }

            // Extract text from response
            let content = gemini_response
                .candidates
                .and_then(|candidates| candidates.into_iter().next())
                .and_then(|candidate| candidate.content.parts.into_iter().next())
                .map(|part| part.text)
                .ok_or_else(|| {
                    error!("Gemini returned no content");
                    AetherError::provider("No response from Gemini")
                })?;

            info!(
                response_length = content.len(),
                "Gemini multimodal request completed successfully"
            );

            Ok(content)
        })
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn color(&self) -> &str {
        &self.config.color
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> ProviderConfig {
        let mut config = ProviderConfig::test_config("gemini-1.5-flash");
        config.color = "#4285F4".to_string(); // Google blue
        config.max_tokens = Some(1000);
        config.temperature = Some(0.7);
        config
    }

    #[test]
    fn test_new_provider_success() {
        let config = create_test_config();
        let provider = GeminiProvider::new("gemini".to_string(), config);
        assert!(provider.is_ok());
    }

    #[test]
    fn test_new_provider_missing_api_key() {
        let mut config = create_test_config();
        config.api_key = None;
        let result = GeminiProvider::new("gemini".to_string(), config);
        assert!(matches!(result, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_new_provider_empty_api_key() {
        let mut config = create_test_config();
        config.api_key = Some("".to_string());
        let result = GeminiProvider::new("gemini".to_string(), config);
        assert!(matches!(result, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_new_provider_empty_model() {
        let mut config = create_test_config();
        config.model = "".to_string();
        let result = GeminiProvider::new("gemini".to_string(), config);
        assert!(matches!(result, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_provider_metadata() {
        let config = create_test_config();
        let provider = GeminiProvider::new("gemini".to_string(), config).unwrap();

        assert_eq!(provider.name(), "gemini");
        assert_eq!(provider.color(), "#4285F4");
    }

    #[test]
    fn test_build_endpoint() {
        let config = create_test_config();
        let provider = GeminiProvider::new("gemini".to_string(), config).unwrap();

        let endpoint = provider.build_endpoint();
        assert!(endpoint.contains("generativelanguage.googleapis.com"));
        assert!(endpoint.contains("gemini-1.5-flash"));
        assert!(endpoint.contains("generateContent"));
    }

    #[test]
    fn test_custom_base_url() {
        let mut config = create_test_config();
        config.base_url = Some("https://custom.google.com/".to_string());

        let provider = GeminiProvider::new("gemini".to_string(), config).unwrap();
        let endpoint = provider.build_endpoint();
        assert!(endpoint.starts_with("https://custom.google.com/"));
    }

    #[test]
    fn test_supports_vision() {
        // Test gemini-1.5-flash (should support vision)
        let config = create_test_config();
        let provider = GeminiProvider::new("gemini".to_string(), config).unwrap();
        assert!(provider.supports_vision());

        // Test gemini-1.5-pro (should support vision)
        let mut config2 = create_test_config();
        config2.model = "gemini-1.5-pro".to_string();
        let provider2 = GeminiProvider::new("gemini".to_string(), config2).unwrap();
        assert!(provider2.supports_vision());

        // Test gemini-pro (text-only, should not support vision)
        let mut config3 = create_test_config();
        config3.model = "gemini-pro".to_string();
        let provider3 = GeminiProvider::new("gemini".to_string(), config3).unwrap();
        assert!(!provider3.supports_vision());
    }
}
