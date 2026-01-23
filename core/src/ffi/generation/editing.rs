//! Image editing operations for AetherCore
//!
//! This module contains image editing methods (inpainting, image-to-image).

use super::types::{GenerationOutputFFI, GenerationParamsFFI};
use crate::ffi::{AetherCore, AetherFfiError};
use crate::generation::{GenerationParams, GenerationRequest, GenerationType};
use tracing::{info, warn};

impl AetherCore {
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
}
