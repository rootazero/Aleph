# Phase 2: OpenAI 生成供应商实现计划

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 实现 OpenAI 图像生成 (DALL·E 3) + TTS + OpenAI 兼容 API 供应商

**Architecture:** 基于 Phase 1 的 `GenerationProvider` trait，创建 `generation/providers/` 目录并实现具体供应商。使用与现有 `providers/openai.rs` 相似的模式：HTTP client + async/await + 错误处理。

**Tech Stack:** Rust, reqwest, serde, tokio, base64

---

## Task 1: 创建 providers 目录结构

**Files:**
- Create: `Aether/core/src/generation/providers/mod.rs`

**Step 1: Write the failing test**

在 `Aether/core/src/generation/mod.rs` 添加测试：

```rust
#[test]
fn test_providers_module_exists() {
    // This test verifies that the providers module is properly exported
    use crate::generation::providers;
    // Module should compile and be accessible
}
```

**Step 2: Run test to verify it fails**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test generation::tests::test_providers_module_exists`
Expected: FAIL with "failed to resolve: use of undeclared crate or module `providers`"

**Step 3: Create the providers module**

Create `Aether/core/src/generation/providers/mod.rs`:

```rust
//! Generation provider implementations
//!
//! This module contains concrete implementations of the `GenerationProvider` trait
//! for various AI service providers.
//!
//! # Available Providers
//!
//! - `OpenAiImageProvider` - DALL·E 3 image generation
//! - `OpenAiTtsProvider` - OpenAI Text-to-Speech
//! - `OpenAiCompatProvider` - Generic OpenAI-compatible API

pub mod openai_image;
pub mod openai_tts;
pub mod openai_compat;

pub use openai_image::OpenAiImageProvider;
pub use openai_tts::OpenAiTtsProvider;
pub use openai_compat::OpenAiCompatProvider;
```

Update `Aether/core/src/generation/mod.rs` to add:
```rust
pub mod providers;
```

**Step 4: Run test to verify it passes**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test generation::tests::test_providers_module_exists`
Expected: PASS

**Step 5: Commit**

```bash
git add Aleph/core/src/generation/providers/mod.rs Aleph/core/src/generation/mod.rs
git commit -m "feat(generation): add providers module structure

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 2: 实现 OpenAI 图像生成供应商 (DALL·E 3)

**Files:**
- Create: `Aether/core/src/generation/providers/openai_image.rs`
- Test: in same file

**Step 1: Create the provider file with tests first**

Create `Aether/core/src/generation/providers/openai_image.rs`:

```rust
//! OpenAI DALL·E 3 Image Generation Provider
//!
//! Implements the `GenerationProvider` trait for OpenAI's image generation API.
//! Supports DALL-E 3 with various sizes and quality options.
//!
//! # API Endpoint
//!
//! POST `{base_url}/v1/images/generations`
//!
//! # Supported Parameters
//!
//! - `model`: "dall-e-3" (default)
//! - `size`: "1024x1024", "1792x1024", "1024x1792"
//! - `quality`: "standard", "hd"
//! - `style`: "vivid", "natural"
//! - `n`: 1 (DALL-E 3 only supports 1)
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::generation::providers::OpenAiImageProvider;
//! use alephcore::generation::{GenerationProvider, GenerationRequest};
//!
//! let provider = OpenAiImageProvider::new(
//!     "sk-xxx".to_string(),
//!     None, // Use default base_url
//!     None, // Use default model
//! )?;
//!
//! let request = GenerationRequest::image("A sunset over mountains");
//! let output = provider.generate(request).await?;
//! ```

use crate::generation::{
    GenerationData, GenerationError, GenerationMetadata, GenerationOutput,
    GenerationProgress, GenerationProvider, GenerationRequest, GenerationResult,
    GenerationType,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tracing::{debug, error, info};

/// Default OpenAI API base URL
const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
/// Default model for image generation
const DEFAULT_MODEL: &str = "dall-e-3";
/// Default timeout in seconds
const DEFAULT_TIMEOUT_SECS: u64 = 120;
/// OpenAI brand color
const OPENAI_COLOR: &str = "#10a37f";

/// OpenAI DALL·E 3 Image Generation Provider
pub struct OpenAiImageProvider {
    /// HTTP client with configured timeout
    client: Client,
    /// API key for authentication
    api_key: String,
    /// API endpoint for image generation
    endpoint: String,
    /// Model to use (default: dall-e-3)
    model: String,
}

/// Request body for OpenAI image generation API
#[derive(Debug, Serialize)]
struct ImageGenerationRequest {
    model: String,
    prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    quality: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    style: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    n: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<String>,
}

/// Response from OpenAI image generation API
#[derive(Debug, Deserialize)]
struct ImageGenerationResponse {
    data: Vec<ImageData>,
    #[allow(dead_code)]
    created: u64,
}

/// Individual image data in response
#[derive(Debug, Deserialize)]
struct ImageData {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    b64_json: Option<String>,
    #[serde(default)]
    revised_prompt: Option<String>,
}

/// Error response from OpenAI API
#[derive(Debug, Deserialize)]
struct ErrorResponse {
    error: ErrorDetails,
}

#[derive(Debug, Deserialize)]
struct ErrorDetails {
    message: String,
    #[serde(rename = "type")]
    #[allow(dead_code)]
    error_type: Option<String>,
    #[allow(dead_code)]
    code: Option<String>,
}

impl OpenAiImageProvider {
    /// Create a new OpenAI image generation provider
    ///
    /// # Arguments
    ///
    /// * `api_key` - OpenAI API key
    /// * `base_url` - Optional custom API endpoint (defaults to OpenAI's API)
    /// * `model` - Optional model override (defaults to dall-e-3)
    ///
    /// # Returns
    ///
    /// * `Ok(OpenAiImageProvider)` - Successfully initialized provider
    /// * `Err(GenerationError)` - Configuration validation failed
    pub fn new(
        api_key: String,
        base_url: Option<String>,
        model: Option<String>,
    ) -> GenerationResult<Self> {
        if api_key.is_empty() {
            return Err(GenerationError::authentication(
                "API key cannot be empty",
                "openai-image",
            ));
        }

        let client = Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .build()
            .map_err(|e| GenerationError::internal(format!("Failed to build HTTP client: {}", e)))?;

        let base = base_url
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string();
        let endpoint = format!("{}/images/generations", base);

        Ok(Self {
            client,
            api_key,
            endpoint,
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        })
    }

    /// Build the API request body from a generation request
    fn build_request_body(&self, request: &GenerationRequest) -> ImageGenerationRequest {
        let params = &request.params;

        // Determine size from width/height or use default
        let size = if let (Some(w), Some(h)) = (params.width, params.height) {
            Some(format!("{}x{}", w, h))
        } else {
            None
        };

        ImageGenerationRequest {
            model: params.model.clone().unwrap_or_else(|| self.model.clone()),
            prompt: request.prompt.clone(),
            size,
            quality: params.quality.clone(),
            style: params.style.clone(),
            n: params.n,
            response_format: Some("url".to_string()),
        }
    }

    /// Handle error response from API
    async fn handle_error(&self, response: reqwest::Response) -> GenerationError {
        let status = response.status();
        let body_text = response.text().await.unwrap_or_default();

        error!(
            status = %status,
            body_preview = %body_text.chars().take(500).collect::<String>(),
            "OpenAI image API error"
        );

        // Try to parse error response
        if let Ok(error_response) = serde_json::from_str::<ErrorResponse>(&body_text) {
            let msg = error_response.error.message;

            return match status.as_u16() {
                400 => {
                    // Check for content policy violation
                    if msg.contains("content policy") || msg.contains("safety") {
                        GenerationError::content_filtered(msg, Some("policy".to_string()))
                    } else if msg.contains("size") {
                        GenerationError::unsupported_dimension(msg, Some("1024x1024, 1792x1024, 1024x1792".to_string()))
                    } else {
                        GenerationError::invalid_parameters(msg, None)
                    }
                }
                401 => GenerationError::authentication(msg, "openai-image"),
                429 => {
                    // Parse retry-after if available
                    GenerationError::rate_limit(msg, None)
                }
                500..=599 => GenerationError::provider(msg, Some(status.as_u16()), "openai-image"),
                _ => GenerationError::provider(msg, Some(status.as_u16()), "openai-image"),
            };
        }

        // Fallback error
        match status.as_u16() {
            401 => GenerationError::authentication("Invalid API key", "openai-image"),
            429 => GenerationError::rate_limit("Rate limit exceeded", None),
            500..=599 => GenerationError::provider(
                format!("Server error: {}", status),
                Some(status.as_u16()),
                "openai-image",
            ),
            _ => GenerationError::provider(
                format!("API error ({}): {}", status, body_text.chars().take(200).collect::<String>()),
                Some(status.as_u16()),
                "openai-image",
            ),
        }
    }
}

impl GenerationProvider for OpenAiImageProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            // Verify generation type
            if request.generation_type != GenerationType::Image {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    "openai-image",
                ));
            }

            debug!(
                model = %self.model,
                prompt_length = request.prompt.len(),
                "Sending image generation request to OpenAI"
            );

            let start = std::time::Instant::now();
            let request_body = self.build_request_body(&request);

            // Send request
            let response = self
                .client
                .post(&self.endpoint)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("Content-Type", "application/json")
                .json(&request_body)
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        GenerationError::timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
                    } else if e.is_connect() {
                        GenerationError::network(format!("Failed to connect: {}", e))
                    } else {
                        GenerationError::network(format!("Network error: {}", e))
                    }
                })?;

            // Check status
            if !response.status().is_success() {
                return Err(self.handle_error(response).await);
            }

            // Parse response
            let api_response: ImageGenerationResponse = response.json().await.map_err(|e| {
                error!(error = %e, "Failed to parse OpenAI image response");
                GenerationError::serialization(format!("Failed to parse response: {}", e))
            })?;

            // Extract image data
            let image_data = api_response
                .data
                .first()
                .ok_or_else(|| GenerationError::provider("No image in response", None, "openai-image"))?;

            let data = if let Some(url) = &image_data.url {
                GenerationData::url(url.clone())
            } else if let Some(b64) = &image_data.b64_json {
                GenerationData::bytes(
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64)
                        .map_err(|e| GenerationError::serialization(format!("Invalid base64: {}", e)))?
                )
            } else {
                return Err(GenerationError::provider("No image data in response", None, "openai-image"));
            };

            let duration = start.elapsed();
            let metadata = GenerationMetadata::new()
                .with_provider("openai-image")
                .with_model(&self.model)
                .with_duration(duration);

            let metadata = if let Some(revised) = &image_data.revised_prompt {
                metadata.with_revised_prompt(revised.clone())
            } else {
                metadata
            };

            let mut output = GenerationOutput::new(GenerationType::Image, data)
                .with_metadata(metadata);

            if let Some(id) = request.request_id {
                output = output.with_request_id(id);
            }

            info!(
                duration_ms = duration.as_millis(),
                "OpenAI image generation completed"
            );

            Ok(output)
        })
    }

    fn name(&self) -> &str {
        "openai-image"
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        vec![GenerationType::Image]
    }

    fn color(&self) -> &str {
        OPENAI_COLOR
    }

    fn default_model(&self) -> Option<&str> {
        Some(&self.model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_provider_success() {
        let provider = OpenAiImageProvider::new(
            "sk-test".to_string(),
            None,
            None,
        );
        assert!(provider.is_ok());
    }

    #[test]
    fn test_new_provider_empty_api_key() {
        let result = OpenAiImageProvider::new("".to_string(), None, None);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), GenerationError::AuthenticationError { .. }));
    }

    #[test]
    fn test_provider_metadata() {
        let provider = OpenAiImageProvider::new("sk-test".to_string(), None, None).unwrap();

        assert_eq!(provider.name(), "openai-image");
        assert_eq!(provider.color(), "#10a37f");
        assert_eq!(provider.default_model(), Some("dall-e-3"));
    }

    #[test]
    fn test_supported_types() {
        let provider = OpenAiImageProvider::new("sk-test".to_string(), None, None).unwrap();

        let types = provider.supported_types();
        assert_eq!(types.len(), 1);
        assert!(types.contains(&GenerationType::Image));
        assert!(provider.supports(GenerationType::Image));
        assert!(!provider.supports(GenerationType::Video));
    }

    #[test]
    fn test_custom_base_url() {
        let provider = OpenAiImageProvider::new(
            "sk-test".to_string(),
            Some("https://custom.api.com".to_string()),
            None,
        ).unwrap();

        assert_eq!(provider.endpoint, "https://custom.api.com/images/generations");
    }

    #[test]
    fn test_custom_model() {
        let provider = OpenAiImageProvider::new(
            "sk-test".to_string(),
            None,
            Some("dall-e-2".to_string()),
        ).unwrap();

        assert_eq!(provider.default_model(), Some("dall-e-2"));
    }

    #[test]
    fn test_build_request_body_basic() {
        let provider = OpenAiImageProvider::new("sk-test".to_string(), None, None).unwrap();
        let request = GenerationRequest::image("A beautiful sunset");

        let body = provider.build_request_body(&request);

        assert_eq!(body.model, "dall-e-3");
        assert_eq!(body.prompt, "A beautiful sunset");
        assert_eq!(body.response_format, Some("url".to_string()));
    }

    #[test]
    fn test_build_request_body_with_params() {
        use crate::generation::GenerationParams;

        let provider = OpenAiImageProvider::new("sk-test".to_string(), None, None).unwrap();
        let params = GenerationParams::builder()
            .width(1792)
            .height(1024)
            .quality("hd")
            .style("vivid")
            .build();
        let request = GenerationRequest::image("A mountain").with_params(params);

        let body = provider.build_request_body(&request);

        assert_eq!(body.size, Some("1792x1024".to_string()));
        assert_eq!(body.quality, Some("hd".to_string()));
        assert_eq!(body.style, Some("vivid".to_string()));
    }

    #[tokio::test]
    async fn test_generate_wrong_type_error() {
        let provider = OpenAiImageProvider::new("sk-test".to_string(), None, None).unwrap();
        let request = GenerationRequest::video("A video");

        let result = provider.generate(request).await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            GenerationError::UnsupportedGenerationTypeError { .. }
        ));
    }
}
```

**Step 2: Run test to verify compilation and tests**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test generation::providers::openai_image::tests`
Expected: All tests PASS

**Step 3: Commit**

```bash
git add Aleph/core/src/generation/providers/openai_image.rs
git commit -m "feat(generation): add OpenAI DALL·E 3 image provider

Implements GenerationProvider trait for OpenAI's image generation API.
- Supports DALL-E 3 with size, quality, style parameters
- Error handling for content policy, rate limits, auth errors
- Custom base_url support for OpenAI-compatible endpoints

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 3: 实现 OpenAI TTS 供应商

**Files:**
- Create: `Aether/core/src/generation/providers/openai_tts.rs`

**Step 1: Create the TTS provider with tests**

Create `Aether/core/src/generation/providers/openai_tts.rs`:

```rust
//! OpenAI Text-to-Speech Provider
//!
//! Implements the `GenerationProvider` trait for OpenAI's TTS API.
//! Supports tts-1 and tts-1-hd models with various voices.
//!
//! # API Endpoint
//!
//! POST `{base_url}/v1/audio/speech`
//!
//! # Supported Parameters
//!
//! - `model`: "tts-1" (default), "tts-1-hd"
//! - `voice`: "alloy", "echo", "fable", "onyx", "nova", "shimmer"
//! - `speed`: 0.25 to 4.0 (default: 1.0)
//! - `format`: "mp3" (default), "opus", "aac", "flac"

use crate::generation::{
    GenerationData, GenerationError, GenerationMetadata, GenerationOutput,
    GenerationProgress, GenerationProvider, GenerationRequest, GenerationResult,
    GenerationType,
};
use reqwest::Client;
use serde::Serialize;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tracing::{debug, error, info};

/// Default OpenAI API base URL
const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
/// Default model for TTS
const DEFAULT_MODEL: &str = "tts-1";
/// Default voice
const DEFAULT_VOICE: &str = "alloy";
/// Default timeout in seconds (TTS is usually fast)
const DEFAULT_TIMEOUT_SECS: u64 = 60;
/// OpenAI brand color
const OPENAI_COLOR: &str = "#10a37f";

/// Available TTS voices
const AVAILABLE_VOICES: [&str; 6] = ["alloy", "echo", "fable", "onyx", "nova", "shimmer"];

/// OpenAI Text-to-Speech Provider
pub struct OpenAiTtsProvider {
    /// HTTP client
    client: Client,
    /// API key
    api_key: String,
    /// API endpoint
    endpoint: String,
    /// Model to use
    model: String,
    /// Default voice
    default_voice: String,
}

/// Request body for OpenAI TTS API
#[derive(Debug, Serialize)]
struct TtsRequest {
    model: String,
    input: String,
    voice: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    speed: Option<f32>,
}

impl OpenAiTtsProvider {
    /// Create a new OpenAI TTS provider
    ///
    /// # Arguments
    ///
    /// * `api_key` - OpenAI API key
    /// * `base_url` - Optional custom API endpoint
    /// * `model` - Optional model override (tts-1 or tts-1-hd)
    /// * `default_voice` - Optional default voice
    pub fn new(
        api_key: String,
        base_url: Option<String>,
        model: Option<String>,
        default_voice: Option<String>,
    ) -> GenerationResult<Self> {
        if api_key.is_empty() {
            return Err(GenerationError::authentication(
                "API key cannot be empty",
                "openai-tts",
            ));
        }

        let client = Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .build()
            .map_err(|e| GenerationError::internal(format!("Failed to build HTTP client: {}", e)))?;

        let base = base_url
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string();
        let endpoint = format!("{}/audio/speech", base);

        let voice = default_voice.unwrap_or_else(|| DEFAULT_VOICE.to_string());
        if !AVAILABLE_VOICES.contains(&voice.as_str()) {
            return Err(GenerationError::invalid_parameters(
                format!("Invalid voice '{}'. Available: {}", voice, AVAILABLE_VOICES.join(", ")),
                Some("voice".to_string()),
            ));
        }

        Ok(Self {
            client,
            api_key,
            endpoint,
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            default_voice: voice,
        })
    }

    /// Build the TTS request body
    fn build_request_body(&self, request: &GenerationRequest) -> TtsRequest {
        let params = &request.params;

        let voice = params.voice.clone()
            .unwrap_or_else(|| self.default_voice.clone());

        let format = params.format.clone();

        TtsRequest {
            model: params.model.clone().unwrap_or_else(|| self.model.clone()),
            input: request.prompt.clone(),
            voice,
            response_format: format,
            speed: params.speed,
        }
    }
}

impl GenerationProvider for OpenAiTtsProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            // Verify generation type
            if request.generation_type != GenerationType::Speech {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    "openai-tts",
                ));
            }

            if request.prompt.is_empty() {
                return Err(GenerationError::invalid_parameters(
                    "Input text cannot be empty",
                    Some("prompt".to_string()),
                ));
            }

            debug!(
                model = %self.model,
                text_length = request.prompt.len(),
                "Sending TTS request to OpenAI"
            );

            let start = std::time::Instant::now();
            let request_body = self.build_request_body(&request);
            let format = request_body.response_format.clone().unwrap_or_else(|| "mp3".to_string());

            // Send request
            let response = self
                .client
                .post(&self.endpoint)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("Content-Type", "application/json")
                .json(&request_body)
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        GenerationError::timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
                    } else if e.is_connect() {
                        GenerationError::network(format!("Failed to connect: {}", e))
                    } else {
                        GenerationError::network(format!("Network error: {}", e))
                    }
                })?;

            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                error!(status = %status, body = %body, "OpenAI TTS request failed");

                return Err(match status.as_u16() {
                    401 => GenerationError::authentication("Invalid API key", "openai-tts"),
                    429 => GenerationError::rate_limit("Rate limit exceeded", None),
                    400 => GenerationError::invalid_parameters(body, None),
                    _ => GenerationError::provider(body, Some(status.as_u16()), "openai-tts"),
                });
            }

            // Get audio bytes
            let bytes = response.bytes().await.map_err(|e| {
                GenerationError::download(format!("Failed to read response: {}", e), None)
            })?;

            let duration = start.elapsed();
            let content_type = match format.as_str() {
                "mp3" => "audio/mpeg",
                "opus" => "audio/opus",
                "aac" => "audio/aac",
                "flac" => "audio/flac",
                _ => "audio/mpeg",
            };

            let metadata = GenerationMetadata::new()
                .with_provider("openai-tts")
                .with_model(&self.model)
                .with_duration(duration)
                .with_content_type(content_type)
                .with_size_bytes(bytes.len() as u64);

            let mut output = GenerationOutput::new(
                GenerationType::Speech,
                GenerationData::bytes(bytes.to_vec()),
            )
            .with_metadata(metadata);

            if let Some(id) = request.request_id {
                output = output.with_request_id(id);
            }

            info!(
                duration_ms = duration.as_millis(),
                size_bytes = bytes.len(),
                "OpenAI TTS generation completed"
            );

            Ok(output)
        })
    }

    fn name(&self) -> &str {
        "openai-tts"
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        vec![GenerationType::Speech]
    }

    fn color(&self) -> &str {
        OPENAI_COLOR
    }

    fn default_model(&self) -> Option<&str> {
        Some(&self.model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_provider_success() {
        let provider = OpenAiTtsProvider::new(
            "sk-test".to_string(),
            None,
            None,
            None,
        );
        assert!(provider.is_ok());
    }

    #[test]
    fn test_new_provider_empty_api_key() {
        let result = OpenAiTtsProvider::new("".to_string(), None, None, None);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), GenerationError::AuthenticationError { .. }));
    }

    #[test]
    fn test_new_provider_invalid_voice() {
        let result = OpenAiTtsProvider::new(
            "sk-test".to_string(),
            None,
            None,
            Some("invalid-voice".to_string()),
        );
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), GenerationError::InvalidParametersError { .. }));
    }

    #[test]
    fn test_provider_metadata() {
        let provider = OpenAiTtsProvider::new("sk-test".to_string(), None, None, None).unwrap();

        assert_eq!(provider.name(), "openai-tts");
        assert_eq!(provider.color(), "#10a37f");
        assert_eq!(provider.default_model(), Some("tts-1"));
    }

    #[test]
    fn test_supported_types() {
        let provider = OpenAiTtsProvider::new("sk-test".to_string(), None, None, None).unwrap();

        assert!(provider.supports(GenerationType::Speech));
        assert!(!provider.supports(GenerationType::Image));
        assert!(!provider.supports(GenerationType::Video));
    }

    #[test]
    fn test_custom_voice() {
        let provider = OpenAiTtsProvider::new(
            "sk-test".to_string(),
            None,
            None,
            Some("nova".to_string()),
        ).unwrap();

        assert_eq!(provider.default_voice, "nova");
    }

    #[test]
    fn test_build_request_body_basic() {
        let provider = OpenAiTtsProvider::new("sk-test".to_string(), None, None, None).unwrap();
        let request = GenerationRequest::speech("Hello world");

        let body = provider.build_request_body(&request);

        assert_eq!(body.model, "tts-1");
        assert_eq!(body.input, "Hello world");
        assert_eq!(body.voice, "alloy");
    }

    #[test]
    fn test_build_request_body_with_params() {
        use crate::generation::GenerationParams;

        let provider = OpenAiTtsProvider::new("sk-test".to_string(), None, None, None).unwrap();
        let params = GenerationParams::builder()
            .voice("nova")
            .speed(1.5)
            .format("opus")
            .build();
        let request = GenerationRequest::speech("Test").with_params(params);

        let body = provider.build_request_body(&request);

        assert_eq!(body.voice, "nova");
        assert_eq!(body.speed, Some(1.5));
        assert_eq!(body.response_format, Some("opus".to_string()));
    }

    #[tokio::test]
    async fn test_generate_wrong_type_error() {
        let provider = OpenAiTtsProvider::new("sk-test".to_string(), None, None, None).unwrap();
        let request = GenerationRequest::image("An image");

        let result = provider.generate(request).await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            GenerationError::UnsupportedGenerationTypeError { .. }
        ));
    }

    #[tokio::test]
    async fn test_generate_empty_input_error() {
        let provider = OpenAiTtsProvider::new("sk-test".to_string(), None, None, None).unwrap();
        let request = GenerationRequest::speech("");

        let result = provider.generate(request).await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            GenerationError::InvalidParametersError { .. }
        ));
    }
}
```

**Step 2: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test generation::providers::openai_tts::tests`
Expected: All tests PASS

**Step 3: Commit**

```bash
git add Aleph/core/src/generation/providers/openai_tts.rs
git commit -m "feat(generation): add OpenAI TTS provider

Implements GenerationProvider trait for OpenAI's text-to-speech API.
- Supports tts-1 and tts-1-hd models
- 6 voice options: alloy, echo, fable, onyx, nova, shimmer
- Multiple output formats: mp3, opus, aac, flac
- Speed control from 0.25x to 4.0x

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 4: 实现 OpenAI 兼容 API 供应商

**Files:**
- Create: `Aether/core/src/generation/providers/openai_compat.rs`

**Step 1: Create the OpenAI-compatible provider**

Create `Aether/core/src/generation/providers/openai_compat.rs`:

```rust
//! Generic OpenAI-Compatible Image Generation Provider
//!
//! Supports any API that implements the OpenAI image generation format.
//! This includes third-party proxies, custom endpoints, and alternative providers.
//!
//! # Use Cases
//!
//! - Third-party OpenAI proxies (e.g., API relay services)
//! - Self-hosted models with OpenAI-compatible APIs
//! - Alternative providers that follow OpenAI's API format

use crate::generation::{
    GenerationData, GenerationError, GenerationMetadata, GenerationOutput,
    GenerationProgress, GenerationProvider, GenerationRequest, GenerationResult,
    GenerationType,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tracing::{debug, error, info};

const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Generic OpenAI-Compatible Generation Provider
///
/// Can be used for any service that implements the OpenAI image generation API format.
pub struct OpenAiCompatProvider {
    /// Provider name (for logging and identification)
    name: String,
    /// HTTP client
    client: Client,
    /// API key
    api_key: String,
    /// Full endpoint URL for image generation
    endpoint: String,
    /// Model to use
    model: String,
    /// Provider brand color
    color: String,
    /// Supported generation types
    supported_types: Vec<GenerationType>,
}

/// Request body (OpenAI format)
#[derive(Debug, Serialize)]
struct ImageRequest {
    model: String,
    prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    quality: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    style: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    n: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<String>,
}

/// Response format (OpenAI format)
#[derive(Debug, Deserialize)]
struct ImageResponse {
    data: Vec<ImageData>,
}

#[derive(Debug, Deserialize)]
struct ImageData {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    b64_json: Option<String>,
    #[serde(default)]
    revised_prompt: Option<String>,
}

/// Builder for OpenAiCompatProvider
pub struct OpenAiCompatProviderBuilder {
    name: String,
    api_key: String,
    base_url: String,
    model: String,
    color: String,
    supported_types: Vec<GenerationType>,
    timeout_secs: u64,
}

impl OpenAiCompatProviderBuilder {
    /// Create a new builder
    pub fn new(name: &str, api_key: &str, base_url: &str) -> Self {
        Self {
            name: name.to_string(),
            api_key: api_key.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            model: "dall-e-3".to_string(),
            color: "#808080".to_string(),
            supported_types: vec![GenerationType::Image],
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
    }

    /// Set the model
    pub fn model(mut self, model: &str) -> Self {
        self.model = model.to_string();
        self
    }

    /// Set the brand color
    pub fn color(mut self, color: &str) -> Self {
        self.color = color.to_string();
        self
    }

    /// Set supported generation types
    pub fn supported_types(mut self, types: Vec<GenerationType>) -> Self {
        self.supported_types = types;
        self
    }

    /// Set timeout in seconds
    pub fn timeout_secs(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Build the provider
    pub fn build(self) -> GenerationResult<OpenAiCompatProvider> {
        if self.api_key.is_empty() {
            return Err(GenerationError::authentication(
                "API key cannot be empty",
                &self.name,
            ));
        }

        if self.base_url.is_empty() {
            return Err(GenerationError::invalid_parameters(
                "Base URL cannot be empty",
                Some("base_url".to_string()),
            ));
        }

        let client = Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .build()
            .map_err(|e| GenerationError::internal(format!("Failed to build HTTP client: {}", e)))?;

        let endpoint = format!("{}/images/generations", self.base_url);

        Ok(OpenAiCompatProvider {
            name: self.name,
            client,
            api_key: self.api_key,
            endpoint,
            model: self.model,
            color: self.color,
            supported_types: self.supported_types,
        })
    }
}

impl OpenAiCompatProvider {
    /// Create a new provider with builder pattern
    pub fn builder(name: &str, api_key: &str, base_url: &str) -> OpenAiCompatProviderBuilder {
        OpenAiCompatProviderBuilder::new(name, api_key, base_url)
    }

    /// Create a simple provider with defaults
    pub fn new(
        name: String,
        api_key: String,
        base_url: String,
        model: Option<String>,
    ) -> GenerationResult<Self> {
        Self::builder(&name, &api_key, &base_url)
            .model(&model.unwrap_or_else(|| "dall-e-3".to_string()))
            .build()
    }

    fn build_request_body(&self, request: &GenerationRequest) -> ImageRequest {
        let params = &request.params;

        let size = if let (Some(w), Some(h)) = (params.width, params.height) {
            Some(format!("{}x{}", w, h))
        } else {
            None
        };

        ImageRequest {
            model: params.model.clone().unwrap_or_else(|| self.model.clone()),
            prompt: request.prompt.clone(),
            size,
            quality: params.quality.clone(),
            style: params.style.clone(),
            n: params.n,
            response_format: Some("url".to_string()),
        }
    }
}

impl GenerationProvider for OpenAiCompatProvider {
    fn generate(
        &self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + '_>> {
        Box::pin(async move {
            if !self.supports(request.generation_type) {
                return Err(GenerationError::unsupported_generation_type(
                    request.generation_type.to_string(),
                    &self.name,
                ));
            }

            debug!(
                provider = %self.name,
                model = %self.model,
                prompt_length = request.prompt.len(),
                "Sending request to OpenAI-compatible API"
            );

            let start = std::time::Instant::now();
            let request_body = self.build_request_body(&request);

            let response = self
                .client
                .post(&self.endpoint)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("Content-Type", "application/json")
                .json(&request_body)
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        GenerationError::timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
                    } else if e.is_connect() {
                        GenerationError::network(format!("Failed to connect to {}: {}", self.name, e))
                    } else {
                        GenerationError::network(format!("Network error: {}", e))
                    }
                })?;

            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                error!(
                    provider = %self.name,
                    status = %status,
                    body = %body,
                    "OpenAI-compat API error"
                );

                return Err(match status.as_u16() {
                    401 => GenerationError::authentication("Invalid API key", &self.name),
                    429 => GenerationError::rate_limit("Rate limit exceeded", None),
                    400 => {
                        if body.contains("content policy") || body.contains("safety") {
                            GenerationError::content_filtered(body, None)
                        } else {
                            GenerationError::invalid_parameters(body, None)
                        }
                    }
                    _ => GenerationError::provider(body, Some(status.as_u16()), &self.name),
                });
            }

            // Handle empty response
            if status == reqwest::StatusCode::NO_CONTENT {
                return Err(GenerationError::provider(
                    "API returned empty response",
                    Some(204),
                    &self.name,
                ));
            }

            let api_response: ImageResponse = response.json().await.map_err(|e| {
                error!(error = %e, "Failed to parse response");
                GenerationError::serialization(format!("Failed to parse response: {}", e))
            })?;

            let image_data = api_response
                .data
                .first()
                .ok_or_else(|| GenerationError::provider("No image in response", None, &self.name))?;

            let data = if let Some(url) = &image_data.url {
                GenerationData::url(url.clone())
            } else if let Some(b64) = &image_data.b64_json {
                GenerationData::bytes(
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64)
                        .map_err(|e| GenerationError::serialization(format!("Invalid base64: {}", e)))?
                )
            } else {
                return Err(GenerationError::provider("No image data in response", None, &self.name));
            };

            let duration = start.elapsed();
            let metadata = GenerationMetadata::new()
                .with_provider(&self.name)
                .with_model(&self.model)
                .with_duration(duration);

            let metadata = if let Some(revised) = &image_data.revised_prompt {
                metadata.with_revised_prompt(revised.clone())
            } else {
                metadata
            };

            let mut output = GenerationOutput::new(request.generation_type, data)
                .with_metadata(metadata);

            if let Some(id) = request.request_id {
                output = output.with_request_id(id);
            }

            info!(
                provider = %self.name,
                duration_ms = duration.as_millis(),
                "OpenAI-compat generation completed"
            );

            Ok(output)
        })
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        self.supported_types.clone()
    }

    fn color(&self) -> &str {
        &self.color
    }

    fn default_model(&self) -> Option<&str> {
        Some(&self.model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_success() {
        let provider = OpenAiCompatProvider::builder("my-proxy", "sk-test", "https://api.proxy.com/v1")
            .model("custom-model")
            .color("#ff0000")
            .build();

        assert!(provider.is_ok());
        let p = provider.unwrap();
        assert_eq!(p.name(), "my-proxy");
        assert_eq!(p.color(), "#ff0000");
        assert_eq!(p.default_model(), Some("custom-model"));
    }

    #[test]
    fn test_builder_empty_api_key() {
        let result = OpenAiCompatProvider::builder("test", "", "https://api.test.com")
            .build();

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), GenerationError::AuthenticationError { .. }));
    }

    #[test]
    fn test_builder_empty_base_url() {
        let result = OpenAiCompatProvider::builder("test", "sk-test", "")
            .build();

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), GenerationError::InvalidParametersError { .. }));
    }

    #[test]
    fn test_new_simple() {
        let provider = OpenAiCompatProvider::new(
            "simple".to_string(),
            "sk-test".to_string(),
            "https://api.example.com/v1".to_string(),
            None,
        );

        assert!(provider.is_ok());
        let p = provider.unwrap();
        assert_eq!(p.endpoint, "https://api.example.com/v1/images/generations");
    }

    #[test]
    fn test_supported_types() {
        let provider = OpenAiCompatProvider::builder("test", "sk-test", "https://api.test.com")
            .supported_types(vec![GenerationType::Image, GenerationType::Video])
            .build()
            .unwrap();

        assert!(provider.supports(GenerationType::Image));
        assert!(provider.supports(GenerationType::Video));
        assert!(!provider.supports(GenerationType::Speech));
    }

    #[test]
    fn test_build_request_body() {
        use crate::generation::GenerationParams;

        let provider = OpenAiCompatProvider::new(
            "test".to_string(),
            "sk-test".to_string(),
            "https://api.test.com".to_string(),
            Some("custom-model".to_string()),
        ).unwrap();

        let params = GenerationParams::builder()
            .width(1024)
            .height(768)
            .quality("hd")
            .build();
        let request = GenerationRequest::image("A test").with_params(params);

        let body = provider.build_request_body(&request);

        assert_eq!(body.model, "custom-model");
        assert_eq!(body.size, Some("1024x768".to_string()));
        assert_eq!(body.quality, Some("hd".to_string()));
    }

    #[tokio::test]
    async fn test_generate_unsupported_type() {
        let provider = OpenAiCompatProvider::builder("test", "sk-test", "https://api.test.com")
            .supported_types(vec![GenerationType::Image])
            .build()
            .unwrap();

        let request = GenerationRequest::speech("Test");
        let result = provider.generate(request).await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            GenerationError::UnsupportedGenerationTypeError { .. }
        ));
    }
}
```

**Step 2: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test generation::providers::openai_compat::tests`
Expected: All tests PASS

**Step 3: Commit**

```bash
git add Aleph/core/src/generation/providers/openai_compat.rs
git commit -m "feat(generation): add OpenAI-compatible provider

Generic provider for any OpenAI-compatible image generation API.
- Builder pattern for flexible configuration
- Supports custom base_url for third-party proxies
- Configurable supported types, color, and model

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 5: 更新 providers/mod.rs 导出并添加工厂函数

**Files:**
- Modify: `Aether/core/src/generation/providers/mod.rs`

**Step 1: Update mod.rs with proper exports and factory function**

Update `Aether/core/src/generation/providers/mod.rs`:

```rust
//! Generation provider implementations
//!
//! This module contains concrete implementations of the `GenerationProvider` trait
//! for various AI service providers.
//!
//! # Available Providers
//!
//! - `OpenAiImageProvider` - DALL·E 3 image generation
//! - `OpenAiTtsProvider` - OpenAI Text-to-Speech
//! - `OpenAiCompatProvider` - Generic OpenAI-compatible API
//!
//! # Factory Function
//!
//! Use `create_provider` to instantiate providers from configuration:
//!
//! ```rust,ignore
//! use alephcore::generation::providers::create_provider;
//! use alephcore::config::GenerationProviderConfig;
//!
//! let config = GenerationProviderConfig { ... };
//! let provider = create_provider("dalle", &config)?;
//! ```

pub mod openai_compat;
pub mod openai_image;
pub mod openai_tts;

pub use openai_compat::{OpenAiCompatProvider, OpenAiCompatProviderBuilder};
pub use openai_image::OpenAiImageProvider;
pub use openai_tts::OpenAiTtsProvider;

use crate::config::GenerationProviderConfig;
use crate::generation::{GenerationError, GenerationProvider, GenerationResult, GenerationType};
use std::sync::Arc;

/// Create a generation provider from configuration
///
/// # Arguments
///
/// * `name` - Provider name (used for logging and identification)
/// * `config` - Provider configuration from config.toml
///
/// # Returns
///
/// * `Ok(Arc<dyn GenerationProvider>)` - Successfully created provider
/// * `Err(GenerationError)` - Configuration or initialization error
///
/// # Supported Provider Types
///
/// - `"openai"` or `"openai_image"` - OpenAI DALL·E image generation
/// - `"openai_tts"` - OpenAI Text-to-Speech
/// - `"openai_compat"` - Generic OpenAI-compatible API
///
/// # Example
///
/// ```rust,ignore
/// use alephcore::generation::providers::create_provider;
///
/// let config = GenerationProviderConfig {
///     provider_type: "openai".to_string(),
///     api_key: Some("sk-xxx".to_string()),
///     ..Default::default()
/// };
///
/// let provider = create_provider("dalle", &config)?;
/// ```
pub fn create_provider(
    name: &str,
    config: &GenerationProviderConfig,
) -> GenerationResult<Arc<dyn GenerationProvider>> {
    let api_key = config.api_key.clone().ok_or_else(|| {
        GenerationError::authentication(
            format!("API key is required for provider '{}'", name),
            name,
        )
    })?;

    let provider: Arc<dyn GenerationProvider> = match config.provider_type.as_str() {
        "openai" | "openai_image" | "dalle" => {
            Arc::new(OpenAiImageProvider::new(
                api_key,
                config.base_url.clone(),
                config.model.clone(),
            )?)
        }
        "openai_tts" | "tts" => {
            Arc::new(OpenAiTtsProvider::new(
                api_key,
                config.base_url.clone(),
                config.model.clone(),
                config.defaults.voice.clone(),
            )?)
        }
        "openai_compat" => {
            let base_url = config.base_url.clone().ok_or_else(|| {
                GenerationError::invalid_parameters(
                    "base_url is required for openai_compat provider",
                    Some("base_url".to_string()),
                )
            })?;

            let mut builder = OpenAiCompatProvider::builder(name, &api_key, &base_url);

            if let Some(model) = &config.model {
                builder = builder.model(model);
            }

            builder = builder.color(&config.color);

            // Convert capabilities to GenerationType
            if !config.capabilities.is_empty() {
                let types: Vec<GenerationType> = config
                    .capabilities
                    .iter()
                    .filter_map(|c| match c.to_lowercase().as_str() {
                        "image" => Some(GenerationType::Image),
                        "video" => Some(GenerationType::Video),
                        "audio" => Some(GenerationType::Audio),
                        "speech" => Some(GenerationType::Speech),
                        _ => None,
                    })
                    .collect();

                if !types.is_empty() {
                    builder = builder.supported_types(types);
                }
            }

            Arc::new(builder.build()?)
        }
        other => {
            return Err(GenerationError::invalid_parameters(
                format!("Unknown provider type: '{}'. Supported: openai, openai_tts, openai_compat", other),
                Some("provider_type".to_string()),
            ));
        }
    };

    Ok(provider)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GenerationDefaults;

    fn test_config(provider_type: &str) -> GenerationProviderConfig {
        GenerationProviderConfig {
            provider_type: provider_type.to_string(),
            api_key: Some("sk-test".to_string()),
            base_url: None,
            model: None,
            enabled: true,
            color: "#808080".to_string(),
            capabilities: vec![],
            defaults: GenerationDefaults::default(),
            models: None,
        }
    }

    #[test]
    fn test_create_openai_image_provider() {
        let config = test_config("openai");
        let provider = create_provider("dalle", &config);

        assert!(provider.is_ok());
        let p = provider.unwrap();
        assert!(p.supports(GenerationType::Image));
    }

    #[test]
    fn test_create_openai_tts_provider() {
        let config = test_config("openai_tts");
        let provider = create_provider("tts", &config);

        assert!(provider.is_ok());
        let p = provider.unwrap();
        assert!(p.supports(GenerationType::Speech));
    }

    #[test]
    fn test_create_openai_compat_provider() {
        let mut config = test_config("openai_compat");
        config.base_url = Some("https://api.proxy.com/v1".to_string());
        config.capabilities = vec!["image".to_string()];

        let provider = create_provider("my-proxy", &config);

        assert!(provider.is_ok());
        let p = provider.unwrap();
        assert!(p.supports(GenerationType::Image));
    }

    #[test]
    fn test_create_provider_missing_api_key() {
        let mut config = test_config("openai");
        config.api_key = None;

        let result = create_provider("test", &config);

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), GenerationError::AuthenticationError { .. }));
    }

    #[test]
    fn test_create_provider_unknown_type() {
        let config = test_config("unknown_type");
        let result = create_provider("test", &config);

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), GenerationError::InvalidParametersError { .. }));
    }

    #[test]
    fn test_create_compat_missing_base_url() {
        let config = test_config("openai_compat");
        let result = create_provider("test", &config);

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), GenerationError::InvalidParametersError { .. }));
    }
}
```

**Step 2: Run all tests**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test generation::providers`
Expected: All tests PASS

**Step 3: Commit**

```bash
git add Aleph/core/src/generation/providers/mod.rs
git commit -m "feat(generation): add provider factory function

Add create_provider() to instantiate providers from config.
Supports: openai (DALL·E), openai_tts, openai_compat.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 6: 在 lib.rs 中导出 providers 模块

**Files:**
- Modify: `Aether/core/src/lib.rs`

**Step 1: Add providers to public API**

In `lib.rs`, ensure the generation module with providers is properly exported:

```rust
// In the generation module exports section, add:
pub use generation::providers;
```

**Step 2: Run full test suite**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test`
Expected: All tests PASS

**Step 3: Commit**

```bash
git add Aleph/core/src/lib.rs
git commit -m "feat(generation): export providers module in public API

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 7: 运行完整构建验证

**Step 1: Run cargo build**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo build`
Expected: Build succeeds with no errors

**Step 2: Run cargo clippy**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo clippy -- -D warnings`
Expected: No warnings

**Step 3: Run full test suite**

Run: `cd /Users/zouguojun/Workspace/Aether/Aether/core && cargo test`
Expected: All tests pass

---

## 验收标准

完成后应有：

1. **新文件**:
   - `generation/providers/mod.rs` - 模块导出 + `create_provider()` 工厂函数
   - `generation/providers/openai_image.rs` - DALL·E 3 图像生成
   - `generation/providers/openai_tts.rs` - OpenAI TTS
   - `generation/providers/openai_compat.rs` - OpenAI 兼容 API

2. **测试覆盖**:
   - 每个 provider 的单元测试
   - 工厂函数测试
   - 错误处理测试

3. **功能**:
   - OpenAI DALL·E 3 图像生成
   - OpenAI TTS 语音合成
   - 任意 OpenAI 兼容 API 的通用支持
   - 完整的错误分类和处理
