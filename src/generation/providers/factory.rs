//! Factory function for creating generation providers from configuration.

use super::url_normalize::resolve_base_url;
use super::*;
use crate::config::GenerationProviderConfig;
use crate::generation::{GenerationError, GenerationProvider, GenerationResult, GenerationType};
use crate::sync_primitives::Arc;

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
/// - `"openai"` or `"openai_image"` or `"dalle"` - OpenAI DALL-E image generation
/// - `"openai_tts"` or `"tts"` - OpenAI Text-to-Speech
/// - `"openai_compat"` - Generic OpenAI-compatible API
/// - `"stability"` or `"stability_image"` or `"sdxl"` - Stability AI image generation
/// - `"google"` or `"google_imagen"` or `"imagen"` - Google Imagen image generation
/// - `"google_veo"` or `"veo"` - Google Veo video generation
/// - `"replicate"` - Replicate API for various models
/// - `"elevenlabs"` - ElevenLabs Text-to-Speech
/// - `"midjourney"` or `"mj"` - T8Star Midjourney API proxy
///
/// # Example
///
/// ```rust,ignore
/// use alephcore::config::GenerationProviderConfig;
/// use alephcore::generation::providers::create_provider;
/// use alephcore::generation::GenerationType;
///
/// // Create a DALL-E provider
/// let config = GenerationProviderConfig {
///     provider_type: "openai".to_string(),
///     api_key: Some("sk-xxx".to_string()),
///     model: Some("dall-e-3".to_string()),
///     ..Default::default()
/// };
/// let provider = create_provider("dalle", &config, GenerationType::Image)?;
///
/// // Create a TTS provider
/// let tts_config = GenerationProviderConfig {
///     provider_type: "openai_tts".to_string(),
///     api_key: Some("sk-xxx".to_string()),
///     model: Some("tts-1-hd".to_string()),
///     ..Default::default()
/// };
/// let tts_provider = create_provider("tts", &tts_config, GenerationType::Speech)?;
///
/// // Create an OpenAI-compatible provider
/// let compat_config = GenerationProviderConfig {
///     provider_type: "openai_compat".to_string(),
///     api_key: Some("api-key".to_string()),
///     base_url: Some("https://api.example.com".to_string()),
///     model: Some("custom-model".to_string()),
///     capabilities: vec![GenerationType::Image, GenerationType::Video],
///     color: "#ff0000".to_string(),
///     ..Default::default()
/// };
/// let compat_provider = create_provider("my-service", &compat_config, GenerationType::Image)?;
/// ```
pub fn create_provider(
    name: &str,
    config: &GenerationProviderConfig,
    gen_type: GenerationType,
) -> GenerationResult<Arc<dyn GenerationProvider>> {
    let resolved_url = config.base_url.as_deref().map(resolve_base_url);

    let api_key = config.api_key.clone().ok_or_else(|| {
        GenerationError::authentication(
            format!("API key is required for provider '{}'", name),
            name,
        )
    })?;

    let provider: Arc<dyn GenerationProvider> = match config.provider_type.as_str() {
        "openai" | "openai_image" | "dalle" => Arc::new(OpenAiImageProvider::new(
            api_key,
            config.base_url.clone(),
            config.default_model().map(|s| s.to_string()),
            resolved_url,
        )?),
        "openai_tts" | "tts" => Arc::new(OpenAiTtsProvider::new(
            api_key,
            config.base_url.clone(),
            config.default_model().map(|s| s.to_string()),
            config.defaults.voice.clone(),
            resolved_url,
        )?),
        "openai_compat" => {
            let base_url = config.base_url.clone().ok_or_else(|| {
                GenerationError::invalid_parameters(
                    "base_url is required for openai_compat provider",
                    Some("base_url".to_string()),
                )
            })?;

            let mut builder = OpenAiCompatProvider::builder(name, &api_key, &base_url);

            if let Some(model) = config.default_model() {
                builder = builder.model(model);
            }

            builder = builder.color(&config.color);

            if let Some(ref edit_url) = config.edit_url {
                builder = builder.edit_endpoint(edit_url);
            }

            // Use capabilities directly (already Vec<GenerationType>)
            if !config.capabilities.is_empty() {
                builder = builder.supported_types(config.capabilities.clone());
            }

            Arc::new(builder.build()?)
        }
        "stability" | "stability_image" | "sdxl" => Arc::new(StabilityImageProvider::new(
            api_key,
            config.base_url.clone(),
            config.default_model().map(|s| s.to_string()),
        )?),
        "google" | "google_imagen" | "imagen" => Arc::new(GoogleImagenProvider::new(
            api_key,
            config.base_url.clone(),
            config.default_model().map(|s| s.to_string()),
        )?),
        "google_veo" | "veo" => Arc::new(GoogleVeoProvider::new(
            api_key,
            config.base_url.clone(),
            config.default_model().map(|s| s.to_string()),
        )?),
        "replicate" => {
            let mut builder = ReplicateProvider::builder(&api_key);

            if let Some(base_url) = &config.base_url {
                builder = builder.endpoint(base_url);
            }

            // Add model as "default" alias if specified
            if let Some(model) = config.default_model() {
                builder = builder.add_model("default", model);
            }

            // Add model mappings from config
            for (alias, version) in &config.model_aliases {
                builder = builder.add_model(alias, version);
            }

            Arc::new(builder.build())
        }
        "elevenlabs" => Arc::new(ElevenLabsProvider::new(
            api_key,
            config.base_url.clone(),
            config.default_model().map(|s| s.to_string()),
            config.defaults.voice.clone(),
        )?),
        "midjourney" | "mj" => {
            let mut builder = MidjourneyProvider::builder(&api_key);

            if let Some(base_url) = &config.base_url {
                builder = builder.endpoint(base_url);
            }

            // Check for mode in extra config or model field
            if let Some(model) = config.default_model() {
                let mode = match model.to_lowercase().as_str() {
                    "fast" | "mj-fast" => MidjourneyMode::Fast,
                    "relax" | "mj-relax" => MidjourneyMode::Relax,
                    _ => MidjourneyMode::Fast,
                };
                builder = builder.mode(mode);
            }

            if !config.color.is_empty() {
                builder = builder.color(&config.color);
            }

            Arc::new(builder.build()?)
        }
        other => {
            return Err(GenerationError::invalid_parameters(
                format!(
                    "Unknown provider type: '{}'. Supported: openai, openai_image, dalle, openai_tts, tts, openai_compat, stability, stability_image, sdxl, google, google_imagen, imagen, google_veo, veo, replicate, elevenlabs, midjourney, mj",
                    other
                ),
                Some("provider_type".to_string()),
            ));
        }
    };

    if !provider.supports(gen_type) {
        return Err(GenerationError::unsupported_generation_type(
            gen_type.to_string(),
            name,
        ));
    }

    Ok(provider)
}
