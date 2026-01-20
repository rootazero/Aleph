//! FFI module for generation operations
//!
//! This module provides FFI-safe interfaces for media generation operations
//! including image generation, speech synthesis, and audio generation.

use super::{AetherCore, AetherFfiError};
use crate::generation::{
    GenerationData, GenerationMetadata, GenerationOutput, GenerationParams, GenerationProgress,
    GenerationProviderRegistry, GenerationRequest, GenerationType,
};
use std::sync::Arc;
use tracing::{info, warn};

// ============================================================================
// FFI-Safe Type Definitions
// ============================================================================

/// FFI-safe generation type enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationTypeFFI {
    /// Image generation (DALL-E, Stable Diffusion, etc.)
    Image,
    /// Video generation (Runway, Pika, etc.)
    Video,
    /// Audio/music generation (Suno, MusicGen, etc.)
    Audio,
    /// Text-to-speech synthesis (ElevenLabs, OpenAI TTS, etc.)
    Speech,
}

impl From<GenerationType> for GenerationTypeFFI {
    fn from(t: GenerationType) -> Self {
        match t {
            GenerationType::Image => GenerationTypeFFI::Image,
            GenerationType::Video => GenerationTypeFFI::Video,
            GenerationType::Audio => GenerationTypeFFI::Audio,
            GenerationType::Speech => GenerationTypeFFI::Speech,
        }
    }
}

impl From<GenerationTypeFFI> for GenerationType {
    fn from(t: GenerationTypeFFI) -> Self {
        match t {
            GenerationTypeFFI::Image => GenerationType::Image,
            GenerationTypeFFI::Video => GenerationType::Video,
            GenerationTypeFFI::Audio => GenerationType::Audio,
            GenerationTypeFFI::Speech => GenerationType::Speech,
        }
    }
}

/// FFI-safe generation parameters
#[derive(Debug, Clone, Default)]
pub struct GenerationParamsFFI {
    // Image/Video parameters
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub aspect_ratio: Option<String>,
    pub quality: Option<String>,
    pub style: Option<String>,
    pub n: Option<u32>,
    pub seed: Option<i64>,
    pub format: Option<String>,

    // Video-specific
    pub duration_seconds: Option<f32>,
    pub fps: Option<u32>,

    // Audio/Speech parameters
    pub voice: Option<String>,
    pub speed: Option<f32>,
    pub language: Option<String>,

    // Common parameters
    pub model: Option<String>,
    pub negative_prompt: Option<String>,
    pub guidance_scale: Option<f32>,
    pub steps: Option<u32>,

    // Reference inputs
    pub reference_image: Option<String>,
    pub reference_audio: Option<String>,

    // Image editing - mask for inpainting (transparent areas = edit regions)
    pub mask: Option<String>,
}

impl From<GenerationParamsFFI> for GenerationParams {
    fn from(p: GenerationParamsFFI) -> Self {
        let mut extra = std::collections::HashMap::new();

        // Add mask to extra params if provided
        if let Some(mask) = &p.mask {
            extra.insert("mask".to_string(), serde_json::json!(mask));
        }

        GenerationParams {
            width: p.width,
            height: p.height,
            aspect_ratio: p.aspect_ratio,
            quality: p.quality,
            style: p.style,
            n: p.n,
            seed: p.seed,
            format: p.format,
            duration_seconds: p.duration_seconds,
            fps: p.fps,
            voice: p.voice,
            speed: p.speed,
            language: p.language,
            model: p.model,
            negative_prompt: p.negative_prompt,
            guidance_scale: p.guidance_scale,
            steps: p.steps,
            reference_image: p.reference_image,
            reference_audio: p.reference_audio,
            extra,
        }
    }
}

impl From<GenerationParams> for GenerationParamsFFI {
    fn from(p: GenerationParams) -> Self {
        // Extract mask from extra params
        let mask = p
            .extra
            .get("mask")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        GenerationParamsFFI {
            width: p.width,
            height: p.height,
            aspect_ratio: p.aspect_ratio,
            quality: p.quality,
            style: p.style,
            n: p.n,
            seed: p.seed,
            format: p.format,
            duration_seconds: p.duration_seconds,
            fps: p.fps,
            voice: p.voice,
            speed: p.speed,
            language: p.language,
            model: p.model,
            negative_prompt: p.negative_prompt,
            guidance_scale: p.guidance_scale,
            steps: p.steps,
            reference_image: p.reference_image,
            reference_audio: p.reference_audio,
            mask,
        }
    }
}

/// FFI-safe generation data representation
#[derive(Debug, Clone)]
pub enum GenerationDataTypeFFI {
    /// Raw binary data
    Bytes,
    /// URL to the generated content
    Url,
    /// Path to a local file
    LocalPath,
}

/// FFI-safe generation data
#[derive(Debug, Clone)]
pub struct GenerationDataFFI {
    /// Type of data
    pub data_type: GenerationDataTypeFFI,
    /// Raw bytes (if data_type is Bytes)
    pub bytes: Option<Vec<u8>>,
    /// URL string (if data_type is Url)
    pub url: Option<String>,
    /// Local file path (if data_type is LocalPath)
    pub local_path: Option<String>,
}

impl From<GenerationData> for GenerationDataFFI {
    fn from(data: GenerationData) -> Self {
        match data {
            GenerationData::Bytes(bytes) => GenerationDataFFI {
                data_type: GenerationDataTypeFFI::Bytes,
                bytes: Some(bytes),
                url: None,
                local_path: None,
            },
            GenerationData::Url(url) => GenerationDataFFI {
                data_type: GenerationDataTypeFFI::Url,
                bytes: None,
                url: Some(url),
                local_path: None,
            },
            GenerationData::LocalPath(path) => GenerationDataFFI {
                data_type: GenerationDataTypeFFI::LocalPath,
                bytes: None,
                url: None,
                local_path: Some(path),
            },
        }
    }
}

/// FFI-safe generation metadata
#[derive(Debug, Clone, Default)]
pub struct GenerationMetadataFFI {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub duration_ms: Option<u64>,
    pub seed: Option<i64>,
    pub revised_prompt: Option<String>,
    pub content_type: Option<String>,
    pub size_bytes: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_seconds: Option<f32>,
}

impl From<GenerationMetadata> for GenerationMetadataFFI {
    fn from(m: GenerationMetadata) -> Self {
        GenerationMetadataFFI {
            provider: m.provider,
            model: m.model,
            duration_ms: m.duration.map(|d| d.as_millis() as u64),
            seed: m.seed,
            revised_prompt: m.revised_prompt,
            content_type: m.content_type,
            size_bytes: m.size_bytes,
            width: m.width,
            height: m.height,
            duration_seconds: m.duration_seconds,
        }
    }
}

/// FFI-safe generation output
#[derive(Debug, Clone)]
pub struct GenerationOutputFFI {
    pub generation_type: GenerationTypeFFI,
    pub data: GenerationDataFFI,
    pub additional_outputs: Vec<GenerationDataFFI>,
    pub metadata: GenerationMetadataFFI,
    pub request_id: Option<String>,
}

impl From<GenerationOutput> for GenerationOutputFFI {
    fn from(output: GenerationOutput) -> Self {
        GenerationOutputFFI {
            generation_type: output.generation_type.into(),
            data: output.data.into(),
            additional_outputs: output
                .additional_outputs
                .into_iter()
                .map(|d| d.into())
                .collect(),
            metadata: output.metadata.into(),
            request_id: output.request_id,
        }
    }
}

/// FFI-safe generation progress
#[derive(Debug, Clone)]
pub struct GenerationProgressFFI {
    pub percentage: f32,
    pub step: String,
    pub eta_ms: Option<u64>,
    pub is_complete: bool,
    pub preview_url: Option<String>,
}

impl From<GenerationProgress> for GenerationProgressFFI {
    fn from(p: GenerationProgress) -> Self {
        GenerationProgressFFI {
            percentage: p.percentage,
            step: p.step,
            eta_ms: p.eta.map(|d| d.as_millis() as u64),
            is_complete: p.is_complete,
            preview_url: p.preview_url,
        }
    }
}

/// FFI-safe provider info for listing
#[derive(Debug, Clone)]
pub struct GenerationProviderInfoFFI {
    pub name: String,
    pub color: String,
    pub supported_types: Vec<GenerationTypeFFI>,
    pub default_model: Option<String>,
}

/// FFI-safe generation provider configuration
///
/// Used for adding/updating generation providers from the UI.
#[derive(Debug, Clone)]
pub struct GenerationProviderConfigFFI {
    /// Provider type identifier (openai, openai_compat, stability, elevenlabs, etc.)
    pub provider_type: String,
    /// API key (optional, can use keychain)
    pub api_key: Option<String>,
    /// Base URL for API (optional, for self-hosted or proxy)
    pub base_url: Option<String>,
    /// Default model to use
    pub model: Option<String>,
    /// Whether this provider is enabled
    pub enabled: bool,
    /// Brand color for UI theming (hex format)
    pub color: String,
    /// Supported generation types
    pub capabilities: Vec<GenerationTypeFFI>,
    /// Request timeout in seconds
    pub timeout_seconds: u64,
}

impl From<GenerationProviderConfigFFI> for crate::config::GenerationProviderConfig {
    fn from(ffi: GenerationProviderConfigFFI) -> Self {
        crate::config::GenerationProviderConfig {
            provider_type: ffi.provider_type,
            api_key: ffi.api_key,
            base_url: ffi.base_url,
            model: ffi.model,
            enabled: ffi.enabled,
            color: ffi.color,
            capabilities: ffi.capabilities.into_iter().map(|t| t.into()).collect(),
            timeout_seconds: ffi.timeout_seconds,
            defaults: Default::default(),
            models: Default::default(),
        }
    }
}

impl From<crate::config::GenerationProviderConfig> for GenerationProviderConfigFFI {
    fn from(config: crate::config::GenerationProviderConfig) -> Self {
        GenerationProviderConfigFFI {
            provider_type: config.provider_type,
            api_key: config.api_key,
            base_url: config.base_url,
            model: config.model,
            enabled: config.enabled,
            color: config.color,
            capabilities: config.capabilities.into_iter().map(|t| t.into()).collect(),
            timeout_seconds: config.timeout_seconds,
        }
    }
}

// ============================================================================
// Response Parsing Types
// ============================================================================

/// FFI-safe parsed generation request from AI response
///
/// When AI recognizes a generation model mention in conversation,
/// it outputs a `[GENERATE:type:provider:model:prompt]` tag that
/// gets parsed into this structure.
#[derive(Debug, Clone)]
pub struct ParsedGenerationRequestFFI {
    /// Generation type (image, video, audio, speech)
    pub gen_type: String,
    /// Provider name (e.g., "midjourney", "dalle")
    pub provider: String,
    /// Model name or alias (e.g., "nanobanana" -> "nano-banana-2")
    pub model: String,
    /// Generation prompt
    pub prompt: String,
    /// Original matched text (for replacement in response)
    pub original_text: String,
}

/// FFI-safe parse result containing requests and cleaned response
#[derive(Debug, Clone)]
pub struct ParseResultFFI {
    /// Extracted generation requests
    pub requests: Vec<ParsedGenerationRequestFFI>,
    /// Response text with generation tags replaced by user-friendly messages
    pub cleaned_response: String,
}

// ============================================================================
// AetherCore Generation Methods
// ============================================================================

impl AetherCore {
    /// List all registered generation providers
    ///
    /// Returns information about each provider including name, supported types,
    /// and default model.
    pub fn list_generation_providers(&self) -> Vec<GenerationProviderInfoFFI> {
        let registry = self.generation_registry.read().unwrap_or_else(|e| {
            warn!("Generation registry lock poisoned, recovering");
            e.into_inner()
        });

        registry
            .names()
            .iter()
            .filter_map(|name| {
                registry.get(name).map(|provider| GenerationProviderInfoFFI {
                    name: provider.name().to_string(),
                    color: provider.color().to_string(),
                    supported_types: provider
                        .supported_types()
                        .into_iter()
                        .map(|t| t.into())
                        .collect(),
                    default_model: provider.default_model().map(|s| s.to_string()),
                })
            })
            .collect()
    }

    /// Generate an image
    ///
    /// # Arguments
    ///
    /// * `provider_name` - Name of the provider to use (or empty for default)
    /// * `prompt` - Text prompt describing the image to generate
    /// * `params` - Optional generation parameters
    ///
    /// # Returns
    ///
    /// Generation output containing the image data or URL
    pub fn generate_image(
        &self,
        provider_name: String,
        prompt: String,
        params: Option<GenerationParamsFFI>,
    ) -> Result<GenerationOutputFFI, AetherFfiError> {
        self.generate(provider_name, GenerationTypeFFI::Image, prompt, params)
    }

    /// Generate speech from text
    ///
    /// # Arguments
    ///
    /// * `provider_name` - Name of the provider to use (or empty for default)
    /// * `text` - Text to convert to speech
    /// * `params` - Optional generation parameters (voice, speed, etc.)
    ///
    /// # Returns
    ///
    /// Generation output containing the audio data
    pub fn generate_speech(
        &self,
        provider_name: String,
        text: String,
        params: Option<GenerationParamsFFI>,
    ) -> Result<GenerationOutputFFI, AetherFfiError> {
        self.generate(provider_name, GenerationTypeFFI::Speech, text, params)
    }

    /// Generate audio/music
    ///
    /// # Arguments
    ///
    /// * `provider_name` - Name of the provider to use (or empty for default)
    /// * `prompt` - Text prompt describing the audio to generate
    /// * `params` - Optional generation parameters
    ///
    /// # Returns
    ///
    /// Generation output containing the audio data
    pub fn generate_audio(
        &self,
        provider_name: String,
        prompt: String,
        params: Option<GenerationParamsFFI>,
    ) -> Result<GenerationOutputFFI, AetherFfiError> {
        self.generate(provider_name, GenerationTypeFFI::Audio, prompt, params)
    }

    /// Generate video
    ///
    /// # Arguments
    ///
    /// * `provider_name` - Name of the provider to use (or empty for default)
    /// * `prompt` - Text prompt describing the video to generate
    /// * `params` - Optional generation parameters
    ///
    /// # Returns
    ///
    /// Generation output containing the video data or URL
    pub fn generate_video(
        &self,
        provider_name: String,
        prompt: String,
        params: Option<GenerationParamsFFI>,
    ) -> Result<GenerationOutputFFI, AetherFfiError> {
        self.generate(provider_name, GenerationTypeFFI::Video, prompt, params)
    }

    /// Generic generate method
    ///
    /// # Arguments
    ///
    /// * `provider_name` - Name of the provider to use (empty for auto-select)
    /// * `generation_type` - Type of content to generate
    /// * `prompt` - Input prompt/text
    /// * `params` - Optional generation parameters
    pub fn generate(
        &self,
        provider_name: String,
        generation_type: GenerationTypeFFI,
        prompt: String,
        params: Option<GenerationParamsFFI>,
    ) -> Result<GenerationOutputFFI, AetherFfiError> {
        let gen_type: GenerationType = generation_type.into();

        info!(
            provider = %provider_name,
            generation_type = %gen_type,
            prompt_len = prompt.len(),
            "Starting generation"
        );

        // Get provider
        let provider = {
            let registry = self.generation_registry.read().unwrap_or_else(|e| {
                warn!("Generation registry lock poisoned, recovering");
                e.into_inner()
            });

            if provider_name.is_empty() {
                // Auto-select first provider that supports this type
                registry
                    .providers_for_type(gen_type)
                    .first()
                    .cloned()
                    .ok_or_else(|| {
                        AetherFfiError::Provider(format!(
                            "No provider available for {:?} generation",
                            gen_type
                        ))
                    })?
            } else {
                registry.get(&provider_name).ok_or_else(|| {
                    AetherFfiError::Provider(format!(
                        "Generation provider '{}' not found",
                        provider_name
                    ))
                })?
            }
        };

        // Check provider supports this type
        if !provider.supports(gen_type) {
            return Err(AetherFfiError::Provider(format!(
                "Provider '{}' does not support {:?} generation",
                provider.name(),
                gen_type
            )));
        }

        // Build request
        let generation_params: GenerationParams = params.unwrap_or_default().into();
        let request = GenerationRequest::new(gen_type, prompt).with_params(generation_params);

        // Execute generation
        let output = self.runtime.block_on(async { provider.generate(request).await });

        match output {
            Ok(output) => {
                info!(
                    provider = %provider.name(),
                    generation_type = %gen_type,
                    "Generation completed successfully"
                );
                Ok(output.into())
            }
            Err(e) => {
                warn!(
                    provider = %provider.name(),
                    error = %e,
                    "Generation failed"
                );
                Err(AetherFfiError::Provider(e.to_string()))
            }
        }
    }

    /// Check the progress of a long-running generation
    ///
    /// # Arguments
    ///
    /// * `provider_name` - Name of the provider
    /// * `job_id` - Job ID returned from a previous generation request
    ///
    /// # Returns
    ///
    /// Progress information including percentage and status
    pub fn check_generation_progress(
        &self,
        provider_name: String,
        job_id: String,
    ) -> Result<GenerationProgressFFI, AetherFfiError> {
        let provider = {
            let registry = self.generation_registry.read().unwrap_or_else(|e| {
                warn!("Generation registry lock poisoned, recovering");
                e.into_inner()
            });

            registry.get(&provider_name).ok_or_else(|| {
                AetherFfiError::Provider(format!(
                    "Generation provider '{}' not found",
                    provider_name
                ))
            })?
        };

        let progress = self
            .runtime
            .block_on(async { provider.check_progress(&job_id).await });

        match progress {
            Ok(progress) => Ok(progress.into()),
            Err(e) => Err(AetherFfiError::Provider(e.to_string())),
        }
    }

    /// Cancel a running generation
    ///
    /// # Arguments
    ///
    /// * `provider_name` - Name of the provider
    /// * `job_id` - Job ID to cancel
    pub fn cancel_generation(
        &self,
        provider_name: String,
        job_id: String,
    ) -> Result<(), AetherFfiError> {
        let provider = {
            let registry = self.generation_registry.read().unwrap_or_else(|e| {
                warn!("Generation registry lock poisoned, recovering");
                e.into_inner()
            });

            registry.get(&provider_name).ok_or_else(|| {
                AetherFfiError::Provider(format!(
                    "Generation provider '{}' not found",
                    provider_name
                ))
            })?
        };

        let result = self
            .runtime
            .block_on(async { provider.cancel(&job_id).await });

        match result {
            Ok(()) => {
                info!(provider = %provider_name, job_id = %job_id, "Generation cancelled");
                Ok(())
            }
            Err(e) => Err(AetherFfiError::Provider(e.to_string())),
        }
    }

    /// Edit an existing image using a prompt
    ///
    /// This method supports image-to-image generation where an input image
    /// is modified based on a text prompt. Some providers call this "inpainting"
    /// or "image editing".
    ///
    /// # Arguments
    ///
    /// * `provider_name` - Name of the provider to use (or empty for default)
    /// * `prompt` - Text prompt describing the desired edit
    /// * `params` - Generation parameters (must include reference_image)
    ///   - `reference_image`: Required - base64-encoded input image or URL
    ///   - `mask`: Optional - base64-encoded mask image (via extra params)
    ///
    /// # Returns
    ///
    /// Generation output containing the edited image data or URL
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Provider not found
    /// - Provider doesn't support image editing
    /// - reference_image not provided
    pub fn edit_image(
        &self,
        provider_name: String,
        prompt: String,
        params: GenerationParamsFFI,
    ) -> Result<GenerationOutputFFI, AetherFfiError> {
        info!(
            provider = %provider_name,
            prompt_len = prompt.len(),
            has_reference = params.reference_image.is_some(),
            "Starting image edit"
        );

        // Validate reference_image is provided
        if params.reference_image.is_none() {
            return Err(AetherFfiError::Config(
                "reference_image is required for image editing".to_string(),
            ));
        }

        // Get provider
        let provider = {
            let registry = self.generation_registry.read().unwrap_or_else(|e| {
                warn!("Generation registry lock poisoned, recovering");
                e.into_inner()
            });

            if provider_name.is_empty() {
                // Auto-select first provider that supports image editing
                registry
                    .providers_for_type(GenerationType::Image)
                    .into_iter()
                    .find(|p| p.supports_image_editing())
                    .ok_or_else(|| {
                        AetherFfiError::Provider(
                            "No provider available that supports image editing".to_string(),
                        )
                    })?
            } else {
                registry.get(&provider_name).ok_or_else(|| {
                    AetherFfiError::Provider(format!(
                        "Generation provider '{}' not found",
                        provider_name
                    ))
                })?
            }
        };

        // Check provider supports image editing
        if !provider.supports_image_editing() {
            return Err(AetherFfiError::Provider(format!(
                "Provider '{}' does not support image editing",
                provider.name()
            )));
        }

        // Build request
        let generation_params: GenerationParams = params.into();
        let request =
            GenerationRequest::new(GenerationType::Image, prompt).with_params(generation_params);

        // Execute image editing
        let output = self
            .runtime
            .block_on(async { provider.edit_image(request).await });

        match output {
            Ok(output) => {
                info!(
                    provider = %provider.name(),
                    "Image editing completed successfully"
                );
                Ok(output.into())
            }
            Err(e) => {
                warn!(
                    provider = %provider.name(),
                    error = %e,
                    "Image editing failed"
                );
                Err(AetherFfiError::Provider(e.to_string()))
            }
        }
    }

    /// Check if a provider supports image editing
    ///
    /// # Arguments
    ///
    /// * `provider_name` - Name of the provider to check
    ///
    /// # Returns
    ///
    /// `true` if the provider supports image editing, `false` otherwise
    pub fn provider_supports_image_editing(&self, provider_name: String) -> bool {
        let registry = self.generation_registry.read().unwrap_or_else(|e| {
            warn!("Generation registry lock poisoned, recovering");
            e.into_inner()
        });

        registry
            .get(&provider_name)
            .map(|p| p.supports_image_editing())
            .unwrap_or(false)
    }

    /// Get providers that support a specific generation type
    ///
    /// # Arguments
    ///
    /// * `generation_type` - Type of generation to filter by
    ///
    /// # Returns
    ///
    /// List of provider names that support the specified type
    pub fn get_providers_for_type(
        &self,
        generation_type: GenerationTypeFFI,
    ) -> Vec<GenerationProviderInfoFFI> {
        let gen_type: GenerationType = generation_type.into();

        let registry = self.generation_registry.read().unwrap_or_else(|e| {
            warn!("Generation registry lock poisoned, recovering");
            e.into_inner()
        });

        registry
            .providers_for_type(gen_type)
            .into_iter()
            .map(|provider| GenerationProviderInfoFFI {
                name: provider.name().to_string(),
                color: provider.color().to_string(),
                supported_types: provider
                    .supported_types()
                    .into_iter()
                    .map(|t| t.into())
                    .collect(),
                default_model: provider.default_model().map(|s| s.to_string()),
            })
            .collect()
    }

    /// Test a generation provider connection with temporary configuration
    ///
    /// This method tests a generation provider without persisting the configuration.
    /// It sends a minimal test request to verify the API is reachable and responsive.
    ///
    /// # Arguments
    ///
    /// * `provider_type` - Provider type: "openai_compat", "openai", "stability", etc.
    /// * `api_key` - API key for authentication
    /// * `base_url` - Base URL for the API (optional, e.g., "https://api.example.com/v1")
    /// * `model` - Model name (optional, e.g., "dall-e-3")
    ///
    /// # Returns
    ///
    /// Test result with success status and message
    pub fn test_generation_provider_connection(
        &self,
        provider_type: String,
        api_key: String,
        base_url: Option<String>,
        model: Option<String>,
    ) -> crate::config::TestConnectionResult {
        use crate::config::GenerationProviderConfig;
        use crate::generation::providers::create_provider;

        info!(
            provider_type = %provider_type,
            base_url = ?base_url,
            model = ?model,
            "Testing generation provider connection"
        );

        // Build provider config
        let provider_config = GenerationProviderConfig {
            provider_type: provider_type.clone(),
            api_key: Some(api_key),
            base_url,
            model,
            enabled: true,
            color: "#808080".to_string(),
            capabilities: vec![GenerationType::Image],
            timeout_seconds: 120,
            defaults: Default::default(),
            models: Default::default(),
        };

        // Create provider instance
        let provider = match create_provider("test-connection", &provider_config) {
            Ok(p) => p,
            Err(e) => {
                return crate::config::TestConnectionResult {
                    success: false,
                    message: format!("Failed to create provider: {}", e),
                };
            }
        };

        // Test by generating a simple image with minimal prompt
        // Don't specify size - let the API use its default (more compatible with different models)
        let test_request = GenerationRequest::new(GenerationType::Image, "a white dot")
            .with_params(
                crate::generation::GenerationParams::builder()
                    .n(1)
                    .build(),
            );

        let result = self.runtime.block_on(async {
            // Use tokio timeout for safety (120 seconds for image generation)
            match tokio::time::timeout(
                std::time::Duration::from_secs(120),
                provider.generate(test_request),
            )
            .await
            {
                Ok(Ok(output)) => {
                    let data_type = if output.data.is_url() {
                        "URL"
                    } else if output.data.is_bytes() {
                        "bytes"
                    } else {
                        "file"
                    };
                    Ok(format!(
                        "Image generated successfully ({} returned)",
                        data_type
                    ))
                }
                Ok(Err(e)) => Err(format!("{}", e)),
                Err(_) => Err("Connection timed out after 120 seconds".to_string()),
            }
        });

        match result {
            Ok(msg) => crate::config::TestConnectionResult {
                success: true,
                message: format!("✓ {}", msg),
            },
            Err(err_msg) => crate::config::TestConnectionResult {
                success: false,
                message: err_msg,
            },
        }
    }

    // ========================================================================
    // Response Parsing Methods
    // ========================================================================

    /// Parse AI response for generation requests
    ///
    /// Looks for `[GENERATE:type:provider:model:prompt]` patterns in AI responses
    /// and extracts them into structured requests that can be executed.
    ///
    /// # Arguments
    ///
    /// * `response` - The AI response text to parse
    ///
    /// # Returns
    ///
    /// ParseResultFFI containing:
    /// - `requests`: List of generation requests to execute
    /// - `cleaned_response`: Response with generation tags replaced by user-friendly messages
    ///
    /// # Example
    ///
    /// ```ignore
    /// let result = core.parse_response_for_generation(ai_response);
    /// for request in result.requests {
    ///     // Execute generation with resolved model alias
    ///     core.generate_image(request.provider, request.prompt, Some(params));
    /// }
    /// // Display cleaned_response to user
    /// ```
    pub fn parse_response_for_generation(&self, response: String) -> ParseResultFFI {
        use crate::generation::response_parser::parse_generation_requests;

        let result = parse_generation_requests(&response);

        ParseResultFFI {
            requests: result
                .requests
                .into_iter()
                .map(|r| ParsedGenerationRequestFFI {
                    gen_type: r.gen_type,
                    provider: r.provider,
                    model: r.model,
                    prompt: r.prompt,
                    original_text: r.original_text,
                })
                .collect(),
            cleaned_response: result.cleaned_response,
        }
    }

    /// Check if response contains any generation requests
    ///
    /// Quick check without full parsing - useful for early detection.
    pub fn has_generation_requests(&self, response: &str) -> bool {
        crate::generation::response_parser::has_generation_requests(response)
    }

    /// Resolve model alias to actual model ID
    ///
    /// Given a provider name and model alias (like "nanobanana"),
    /// returns the actual model ID (like "nano-banana-2") if found in config.
    ///
    /// # Arguments
    ///
    /// * `provider_name` - The generation provider name
    /// * `model_alias` - The model alias or name to resolve
    ///
    /// # Returns
    ///
    /// The resolved model ID, or the original alias if no mapping found
    pub fn resolve_model_alias(&self, provider_name: &str, model_alias: &str) -> String {
        let full_config = self.full_config.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(provider_config) = full_config.generation.providers.get(provider_name) {
            // Check if model_alias matches any configured alias
            for (alias, actual_model) in &provider_config.models {
                if alias.eq_ignore_ascii_case(model_alias) {
                    return actual_model.clone();
                }
            }

            // Check if it's already an actual model name
            for (_, actual_model) in &provider_config.models {
                if actual_model.eq_ignore_ascii_case(model_alias) {
                    return actual_model.clone();
                }
            }

            // Fallback to default model if available
            if let Some(ref default_model) = provider_config.model {
                return default_model.clone();
            }
        }

        // Return original if no mapping found
        model_alias.to_string()
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Initialize generation providers from configuration
pub(crate) fn init_generation_providers(
    config: &crate::config::Config,
) -> Arc<std::sync::RwLock<GenerationProviderRegistry>> {
    use crate::generation::providers::create_provider;

    let mut registry = GenerationProviderRegistry::new();

    // Iterate over configured generation providers
    for (name, provider_config) in &config.generation.providers {
        if !provider_config.enabled {
            info!(provider = %name, "Generation provider disabled, skipping");
            continue;
        }

        match create_provider(name, provider_config) {
            Ok(provider) => {
                if let Err(e) = registry.register(name.clone(), provider) {
                    warn!(provider = %name, error = %e, "Failed to register generation provider");
                } else {
                    info!(provider = %name, "Registered generation provider");
                }
            }
            Err(e) => {
                warn!(provider = %name, error = %e, "Failed to create generation provider");
            }
        }
    }

    info!(
        provider_count = registry.len(),
        "Generation provider registry initialized"
    );

    Arc::new(std::sync::RwLock::new(registry))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generation_type_conversion() {
        // Test FFI -> Core
        assert_eq!(
            GenerationType::from(GenerationTypeFFI::Image),
            GenerationType::Image
        );
        assert_eq!(
            GenerationType::from(GenerationTypeFFI::Video),
            GenerationType::Video
        );
        assert_eq!(
            GenerationType::from(GenerationTypeFFI::Audio),
            GenerationType::Audio
        );
        assert_eq!(
            GenerationType::from(GenerationTypeFFI::Speech),
            GenerationType::Speech
        );

        // Test Core -> FFI
        assert_eq!(
            GenerationTypeFFI::from(GenerationType::Image),
            GenerationTypeFFI::Image
        );
        assert_eq!(
            GenerationTypeFFI::from(GenerationType::Video),
            GenerationTypeFFI::Video
        );
        assert_eq!(
            GenerationTypeFFI::from(GenerationType::Audio),
            GenerationTypeFFI::Audio
        );
        assert_eq!(
            GenerationTypeFFI::from(GenerationType::Speech),
            GenerationTypeFFI::Speech
        );
    }

    #[test]
    fn test_generation_params_conversion() {
        let ffi_params = GenerationParamsFFI {
            width: Some(1024),
            height: Some(1024),
            quality: Some("hd".to_string()),
            style: Some("vivid".to_string()),
            voice: Some("alloy".to_string()),
            ..Default::default()
        };

        let core_params: GenerationParams = ffi_params.into();

        assert_eq!(core_params.width, Some(1024));
        assert_eq!(core_params.height, Some(1024));
        assert_eq!(core_params.quality, Some("hd".to_string()));
        assert_eq!(core_params.style, Some("vivid".to_string()));
        assert_eq!(core_params.voice, Some("alloy".to_string()));
    }

    #[test]
    fn test_generation_data_conversion() {
        // Test URL conversion
        let url_data = GenerationData::Url("https://example.com/image.png".to_string());
        let ffi_url: GenerationDataFFI = url_data.into();
        assert!(matches!(ffi_url.data_type, GenerationDataTypeFFI::Url));
        assert_eq!(
            ffi_url.url,
            Some("https://example.com/image.png".to_string())
        );
        assert!(ffi_url.bytes.is_none());
        assert!(ffi_url.local_path.is_none());

        // Test Bytes conversion
        let bytes_data = GenerationData::Bytes(vec![1, 2, 3, 4]);
        let ffi_bytes: GenerationDataFFI = bytes_data.into();
        assert!(matches!(ffi_bytes.data_type, GenerationDataTypeFFI::Bytes));
        assert_eq!(ffi_bytes.bytes, Some(vec![1, 2, 3, 4]));
        assert!(ffi_bytes.url.is_none());

        // Test LocalPath conversion
        let path_data = GenerationData::LocalPath("/tmp/image.png".to_string());
        let ffi_path: GenerationDataFFI = path_data.into();
        assert!(matches!(
            ffi_path.data_type,
            GenerationDataTypeFFI::LocalPath
        ));
        assert_eq!(ffi_path.local_path, Some("/tmp/image.png".to_string()));
    }

    #[test]
    fn test_generation_metadata_conversion() {
        use std::time::Duration;

        let metadata = GenerationMetadata {
            provider: Some("openai".to_string()),
            model: Some("dall-e-3".to_string()),
            duration: Some(Duration::from_millis(1500)),
            seed: Some(12345),
            revised_prompt: Some("A beautiful sunset".to_string()),
            content_type: Some("image/png".to_string()),
            size_bytes: Some(102400),
            width: Some(1024),
            height: Some(1024),
            duration_seconds: None,
            extra: Default::default(),
        };

        let ffi_metadata: GenerationMetadataFFI = metadata.into();

        assert_eq!(ffi_metadata.provider, Some("openai".to_string()));
        assert_eq!(ffi_metadata.model, Some("dall-e-3".to_string()));
        assert_eq!(ffi_metadata.duration_ms, Some(1500));
        assert_eq!(ffi_metadata.seed, Some(12345));
        assert_eq!(ffi_metadata.width, Some(1024));
        assert_eq!(ffi_metadata.height, Some(1024));
    }

    #[test]
    fn test_generation_progress_conversion() {
        use std::time::Duration;

        let progress = GenerationProgress {
            percentage: 75.0,
            step: "Rendering".to_string(),
            eta: Some(Duration::from_secs(10)),
            is_complete: false,
            preview_url: Some("https://example.com/preview.jpg".to_string()),
        };

        let ffi_progress: GenerationProgressFFI = progress.into();

        assert_eq!(ffi_progress.percentage, 75.0);
        assert_eq!(ffi_progress.step, "Rendering");
        assert_eq!(ffi_progress.eta_ms, Some(10000));
        assert!(!ffi_progress.is_complete);
        assert_eq!(
            ffi_progress.preview_url,
            Some("https://example.com/preview.jpg".to_string())
        );
    }
}
