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
}

impl From<GenerationParamsFFI> for GenerationParams {
    fn from(p: GenerationParamsFFI) -> Self {
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
            extra: Default::default(),
        }
    }
}

impl From<GenerationParams> for GenerationParamsFFI {
    fn from(p: GenerationParams) -> Self {
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
