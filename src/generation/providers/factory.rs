//! Factory function for creating generation providers from configuration.

use super::url_normalize::resolve_base_url;
use super::{
    AzureSpeechProvider, BflProvider, CartesiaProvider, DeepgramSttProvider, DeepgramTtsProvider,
    ElevenLabsProvider, FalProvider, GoogleImagenProvider, GoogleVeoProvider, MidjourneyMode,
    MidjourneyProvider, MinimaxTtsProvider, OpenAiCompatProvider, OpenAiImageProvider,
    OpenAiTtsProvider, OpenAiWhisperProvider, ReplicateProvider, StabilityImageProvider,
    SunoProvider, VolcengineTtsProvider,
};
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
/// - `"openai"` or `"openai_image"` or `"dalle"` - `OpenAI` DALL-E image generation
/// - `"openai_tts"` or `"tts"` - `OpenAI` Text-to-Speech
/// - `"openai_compat"` - Generic OpenAI-compatible API
/// - `"stability"` or `"stability_image"` or `"sdxl"` - Stability AI image generation
/// - `"google"` or `"google_imagen"` or `"imagen"` - Google Imagen image generation
/// - `"google_veo"` or `"veo"` - Google Veo video generation
/// - `"replicate"` - Replicate API for various models
/// - `"elevenlabs"` - `ElevenLabs` Text-to-Speech
/// - `"midjourney"` or `"mj"` - `T8Star` Midjourney API proxy
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

    // BYO local voice endpoint (OpenAI-compatible server the user runs).
    // Handled before the api_key gate: BYO endpoints are commonly
    // unauthenticated, so no key is required for this provider type.
    if config.provider_type == "local" {
        let provider: Arc<dyn GenerationProvider> = Arc::new(
            crate::gateway::voice::local_provider::LocalVoiceProvider::from_config(
                gen_type, config,
            ),
        );
        if !provider.supports(gen_type) {
            return Err(GenerationError::unsupported_generation_type(
                gen_type.to_string(),
                name,
            ));
        }
        return Ok(provider);
    }

    let api_key = config.api_key.clone().ok_or_else(|| {
        GenerationError::authentication(format!("API key is required for provider '{name}'"), name)
    })?;

    let provider: Arc<dyn GenerationProvider> = match config.provider_type.as_str() {
        "openai" | "openai_image" | "dalle" => Arc::new(OpenAiImageProvider::new(
            api_key,
            config.base_url.clone(),
            config.default_model().map(|s| s.to_string()),
            resolved_url,
        )?),
        "openai_tts" | "tts" => Arc::new(
            OpenAiTtsProvider::new(
                api_key,
                config.base_url.clone(),
                config.default_model().map(|s| s.to_string()),
                config.defaults.voice.clone(),
                resolved_url,
            )?
            // Honor the (previously dead) `timeout_seconds` config knob.
            .with_timeout(config.timeout_seconds)?,
        ),
        "openai_whisper" | "whisper" => Arc::new(OpenAiWhisperProvider::new(
            api_key,
            config.base_url.clone(),
            config.default_model().map(|s| s.to_string()),
            resolved_url,
        )?),
        "deepgram_stt" | "deepgram" => Arc::new(DeepgramSttProvider::new(
            api_key,
            config.base_url.clone(),
            config.default_model().map(|s| s.to_string()),
        )?),
        "deepgram_tts" => Arc::new(DeepgramTtsProvider::new(
            api_key,
            config.base_url.clone(),
            config.default_model().map(|s| s.to_string()),
        )?),
        "azure_speech" | "azure_tts" => Arc::new(
            AzureSpeechProvider::new(
                api_key,
                config.base_url.clone(),
                config
                    .defaults
                    .voice
                    .clone()
                    .or_else(|| config.default_model().map(|s| s.to_string())),
            )?
            // Honor the (previously dead) `timeout_seconds` config knob.
            .with_timeout(config.timeout_seconds)?,
        ),
        "suno" => Arc::new(SunoProvider::new(
            api_key,
            config.base_url.clone(),
            config.default_model().map(|s| s.to_string()),
        )?),
        "bfl" | "bfl_flux" | "flux" => Arc::new(BflProvider::new(
            api_key,
            config.base_url.clone(),
            config.default_model().map(|s| s.to_string()),
        )?),
        "cartesia" => Arc::new(CartesiaProvider::new(
            api_key,
            config.base_url.clone(),
            config.default_model().map(|s| s.to_string()),
            config.defaults.voice.clone(),
        )?),
        "minimax_tts" => Arc::new(MinimaxTtsProvider::new(
            api_key,
            config.base_url.clone(),
            config.default_model().map(|s| s.to_string()),
            config.defaults.voice.clone(),
        )?),
        "volcengine_tts" => Arc::new(VolcengineTtsProvider::new(
            api_key,
            config.base_url.clone(),
            config.default_model().map(|s| s.to_string()),
            config.defaults.voice.clone(),
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

            // Honor the (previously dead) `timeout_seconds` config knob.
            builder = builder.timeout_secs(config.timeout_seconds);

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

            // Honor the (previously dead) `capabilities` config knob, the
            // same way the `openai_compat` and `fal` arms already do.
            // Guarded on non-empty so a config that never mentions capabilities
            // keeps the builder's own [Image, Audio] default.
            if !config.capabilities.is_empty() {
                builder = builder.supported_types(config.capabilities.clone());
            }

            Arc::new(builder.build())
        }
        "elevenlabs" => Arc::new(ElevenLabsProvider::new(
            api_key,
            config.base_url.clone(),
            config.default_model().map(|s| s.to_string()),
            config.defaults.voice.clone(),
        )?),
        "fal" => {
            // Fal serves image/video/music behind a single queue API.
            // `capabilities` from config determines which modalities this
            // preset will accept (factory rejects mismatches via supports()).
            let mut builder = FalProvider::builder(name, &api_key);
            if let Some(base_url) = &config.base_url {
                builder = builder.endpoint(base_url);
            }
            if let Some(model) = config.default_model() {
                builder = builder.model(model);
            }
            if !config.color.is_empty() {
                builder = builder.color(&config.color);
            }
            if !config.capabilities.is_empty() {
                builder = builder.supported_types(config.capabilities.clone());
            } else {
                // Default Fal preset is image-only; video/music presets must
                // opt in via config.capabilities.
                builder = builder.supported_types(vec![GenerationType::Image]);
            }
            Arc::new(builder.build()?)
        }
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
                    _ => {
                        return Err(GenerationError::invalid_parameters(
                            format!("Invalid midjourney mode: '{model}'. Supported: fast, relax"),
                            Some("model".to_string()),
                        ));
                    }
                };
                builder = builder.mode(mode);
            }

            if !config.color.is_empty() {
                builder = builder.color(&config.color);
            }

            // Honor the (previously dead) `timeout_seconds` config knob.
            // WARNING: this MOVES the unconfigured default. The builder's own
            // `DEFAULT_REQUEST_TIMEOUT_SECS` is 30 s and `timeout_seconds`
            // defaults to 120 s. That is deliberate: `timeout_seconds` is the
            // one place this subsystem derives a request timeout (its own default
            // reads `defaults_override::generation_timeout_seconds`), and a
            // per-module constant that silently wins over it is the same fact
            // stated twice. Poll CADENCE is unaffected -- that comes from
            // `POLL_INTERVAL_SECS` / `MAX_POLL_ATTEMPTS`.
            builder = builder.timeout_secs(config.timeout_seconds);

            Arc::new(builder.build()?)
        }
        other => {
            return Err(GenerationError::invalid_parameters(
                format!(
                    "Unknown provider type: '{other}'. Supported: openai, openai_image, dalle, openai_tts, tts, openai_whisper, whisper, deepgram_stt, deepgram, deepgram_tts, azure_speech, azure_tts, suno, bfl, bfl_flux, flux, cartesia, minimax_tts, local, openai_compat, stability, stability_image, sdxl, google, google_imagen, imagen, google_veo, veo, replicate, elevenlabs, midjourney, mj, fal"
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

// Only the `capabilities` wire is guarded below. The other two knobs this
// round connected -- `timeout_seconds` into the `openai_compat` and
// `midjourney` builders -- have NO observable surface on
// `Arc<dyn GenerationProvider>`: the value ends up inside a
// `reqwest::Client`, which exposes no getter. A test asserting the config
// default equals a constant would read back its own literal and could never go
// red, so those two are covered by review only. Guarding them means first
// giving a provider a way to report its own timeout.
#[cfg(test)]
mod tests {
    use super::*;

    fn config_for(
        provider_type: &str,
        capabilities: Vec<GenerationType>,
    ) -> GenerationProviderConfig {
        GenerationProviderConfig {
            provider_type: provider_type.to_string(),
            api_key: Some("test-key".to_string()),
            capabilities,
            ..Default::default()
        }
    }

    /// The `capabilities` knob reaches the Replicate provider.
    ///
    /// Falsification: drop the `supported_types` call from the
    /// `replicate` arm and this goes red on the second assertion -- the
    /// builder's own default is [Image, Audio], so an audio-less config still
    /// answered `supports(Audio) == true`. That silent widening is what
    /// the arm did for as long as the knob has existed, while
    /// `openai_compat` and `fal` right next to it always honoured
    /// it.
    #[test]
    fn replicate_honours_the_capabilities_knob() {
        let config = config_for("replicate", vec![GenerationType::Image]);
        let provider = create_provider("rep", &config, GenerationType::Image)
            .expect("image-capable replicate provider");

        assert!(provider.supports(GenerationType::Image));
        assert!(
            !provider.supports(GenerationType::Audio),
            "capabilities named Image only, so Audio must not be advertised"
        );
    }

    /// ...and an UNSET knob still means "whatever the provider defaults to",
    /// not "nothing". Without this half, changing the arm to apply
    /// `config.capabilities` unconditionally would stay green while
    /// silently emptying the capability set of every deployment that never set
    /// it -- replaying a list is not restoring it when the default is "all".
    #[test]
    fn an_unset_capabilities_knob_leaves_the_provider_default_alone() {
        let config = config_for("replicate", Vec::new());
        let provider = create_provider("rep", &config, GenerationType::Image)
            .expect("default replicate provider");

        assert!(provider.supports(GenerationType::Image));
        assert!(provider.supports(GenerationType::Audio));
    }
}
