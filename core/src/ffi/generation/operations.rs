//! Generation operations for AetherCore
//!
//! This module contains the main generation methods implemented on AetherCore.

use super::provider_info::GenerationProviderInfoFFI;
use super::response_parsing::{ParseResultFFI, ParsedGenerationRequestFFI};
use super::types::{
    GenerationOutputFFI, GenerationParamsFFI, GenerationProgressFFI, GenerationTypeFFI,
};
use crate::ffi::{AetherCore, AetherFfiError};
use crate::generation::{GenerationParams, GenerationRequest, GenerationType};
use tracing::{info, warn};

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
                registry
                    .get(name)
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
        let output = self
            .runtime
            .block_on(async { provider.generate(request).await });

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
            for actual_model in provider_config.models.values() {
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
