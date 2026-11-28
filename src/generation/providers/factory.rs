//! Factory function for creating generation providers from configuration.

use super::http::WithRequestTimeout;
use super::url_normalize::resolve_base_url;
use super::{
    AzureSpeechProvider, BflProvider, CartesiaProvider, DeepgramSttProvider, DeepgramTtsProvider,
    ElevenLabsProvider, FalProvider, GoogleImagenProvider, GoogleVeoProvider, MidjourneyMode,
    MidjourneyProvider, MinimaxTtsProvider, OpenAiCompatProvider, OpenAiImageProvider,
    OpenAiTtsProvider, OpenAiWhisperProvider, ReplicateProvider, StabilityImageProvider,
    SunoProvider, VolcengineTtsProvider,
};
use crate::config::GenerationProviderConfig;
use crate::generation::{
    GenerationError, GenerationOutput, GenerationParams, GenerationProgress, GenerationProvider,
    GenerationRequest, GenerationResult, GenerationType, VoiceInfo,
};
use crate::sync_primitives::Arc;
use std::borrow::Cow;
use std::collections::HashMap;

/// Every spelling of `provider_type` the factory accepts, and the canonical
/// provider type each one selects.
///
/// One table, four readers: the dispatch in `create_provider`, the preset
/// lookup in the connectivity probe (`src/generation/probe.rs`), the error
/// message that lists the accepted spellings, and the test that keeps the
/// dispatch and this table in agreement.
///
/// The table exists because the factory's `match` used to BE the only list,
/// and two other readers each carried their own copy of it. The probe's copy
/// was the incomplete one: it looked presets up by an exact `provider_type`
/// match against the 18 canonical preset types, so the 15 aliases the
/// factory accepted -- `"tts"`, `"dalle"`, `"mj"`, `"veo"`, `"imagen"`,
/// `"flux"`, … -- had no preset to find. A provider configured under any of
/// them answered the connectivity test with "未配置 base_url，无法测试连接"
/// while the provider itself worked fine, because the factory had resolved
/// the alias and the probe had not.
pub(crate) const PROVIDER_TYPE_ALIASES: &[(&str, &str)] = &[
    ("openai", "openai"),
    ("openai_image", "openai"),
    ("dalle", "openai"),
    ("openai_tts", "openai_tts"),
    ("tts", "openai_tts"),
    ("openai_whisper", "openai_whisper"),
    ("whisper", "openai_whisper"),
    ("deepgram_stt", "deepgram_stt"),
    ("deepgram", "deepgram_stt"),
    ("deepgram_tts", "deepgram_tts"),
    ("azure_speech", "azure_speech"),
    ("azure_tts", "azure_speech"),
    ("suno", "suno"),
    ("bfl", "bfl"),
    ("bfl_flux", "bfl"),
    ("flux", "bfl"),
    ("cartesia", "cartesia"),
    ("minimax_tts", "minimax_tts"),
    ("volcengine_tts", "volcengine_tts"),
    ("openai_compat", "openai_compat"),
    ("stability", "stability"),
    ("stability_image", "stability"),
    ("sdxl", "stability"),
    ("google", "google"),
    ("google_imagen", "google"),
    ("imagen", "google"),
    ("google_veo", "google_veo"),
    ("veo", "google_veo"),
    ("replicate", "replicate"),
    ("elevenlabs", "elevenlabs"),
    ("midjourney", "midjourney"),
    ("mj", "midjourney"),
    ("fal", "fal"),
    ("local", "local"),
];

/// The canonical provider type for any spelling the factory accepts.
///
/// Returns the canonical name for a known spelling, and the trimmed,
/// lower-cased input for anything else -- so an unknown type still reaches
/// the catch-all arm and still gets the "Unknown provider type" error, now
/// spelled the way the operator wrote it rather than the way it was
/// capitalized.
///
/// Case-insensitivity is deliberate and applies to the whole table: a
/// `provider_type` of `"OpenAI"` used to fall through to the catch-all while
/// `"openai"` worked, which is the same defect as an alias going missing,
/// only quieter.
pub(crate) fn canonical_provider_type(provider_type: &str) -> Cow<'_, str> {
    let normalized = provider_type.trim().to_ascii_lowercase();
    match PROVIDER_TYPE_ALIASES
        .iter()
        .find(|(alias, _)| *alias == normalized)
    {
        Some((_, canonical)) => Cow::Borrowed(*canonical),
        None => Cow::Owned(normalized),
    }
}

/// The operator's colour for this provider entry, or `None` when the entry
/// carries only the neutral default.
///
/// A config that never mentions the colour deserializes as the neutral grey
/// `DEFAULT_GENERATION_COLOR`; applying that on top of every provider's own
/// brand colour would erase it. Only a colour the operator actually chose
/// counts as an opinion.
fn configured_color(config: &GenerationProviderConfig) -> Option<String> {
    let color = config.color.trim();
    if color.is_empty() || color == aleph_protocol::providers::default_generation_color() {
        None
    } else {
        Some(color.to_string())
    }
}

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
/// Every spelling below selects the provider named on the right. The list
/// is derived from `PROVIDER_TYPE_ALIASES`, which is also what the
/// connectivity probe and the "Unknown provider type" error read.
///
/// - `"openai"` / `"openai_image"` / `"dalle"` - `OpenAI` DALL-E image generation
/// - `"openai_tts"` / `"tts"` - `OpenAI` Text-to-Speech
/// - `"openai_whisper"` / `"whisper"` - `OpenAI` speech-to-text
/// - `"deepgram_stt"` / `"deepgram"` - Deepgram speech-to-text
/// - `"deepgram_tts"` - Deepgram Text-to-Speech
/// - `"azure_speech"` / `"azure_tts"` - `Azure` Speech
/// - `"suno"` - Suno music generation
/// - `"bfl"` / `"bfl_flux"` / `"flux"` - Black Forest Labs image generation
/// - `"cartesia"` - Cartesia Text-to-Speech
/// - `"minimax_tts"` - MiniMax Text-to-Speech
/// - `"volcengine_tts"` - Volcengine Text-to-Speech
/// - `"openai_compat"` - Generic OpenAI-compatible API
/// - `"stability"` / `"stability_image"` / `"sdxl"` - Stability AI image generation
/// - `"google"` / `"google_imagen"` / `"imagen"` - Google Imagen image generation
/// - `"google_veo"` / `"veo"` - Google Veo video generation
/// - `"replicate"` - Replicate API for various models
/// - `"elevenlabs"` - `ElevenLabs` Text-to-Speech
/// - `"midjourney"` / `"mj"` - `T8Star` Midjourney API proxy
/// - `"fal"` - fal.ai queue API (image / video / music)
/// - `"local"` - a BYO OpenAI-compatible endpoint the user runs
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

    // The model the provider uses when the request does not name one.
    //
    // `model_aliases` maps a friendly name to the real model ID, and until
    // now only the `replicate` arm consulted it: the other nineteen arms
    // sent `models[0]` verbatim, so a config whose `models` list held a
    // friendly name asked the upstream API for that name. Resolving once
    // here means every arm honours the same map.
    let default_model = config
        .resolve_model(config.default_model())
        .map(str::to_string);

    // BYO local voice endpoint (OpenAI-compatible server the user runs).
    // Handled before the api_key gate: BYO endpoints are commonly
    // unauthenticated, so no key is required for this provider type.
    if canonical_provider_type(&config.provider_type).as_ref() == "local" {
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
        return Ok(DefaultsOverlay::wrap(
            provider,
            config.defaults.to_params(),
            &config.model_aliases,
            configured_color(config),
        ));
    }

    let api_key = config.api_key.clone().ok_or_else(|| {
        GenerationError::authentication(format!("API key is required for provider '{name}'"), name)
    })?;

    // Reject empty / whitespace-only api keys at the factory gate. Six
    // individual builders (OpenAiImageProvider, StabilityImageProvider,
    // GoogleImagenProvider, MidjourneyProviderBuilder, ReplicateProviderBuilder,
    // FalProviderBuilder) used to accept `Some("")` silently and only fail
    // at the first request with a 401 — a misconfigured provider would run
    // for an arbitrary time before the operator noticed. Centralising the
    // check here means future arms inherit the gate for free.
    if api_key.trim().is_empty() {
        return Err(GenerationError::authentication(
            format!(
                "API key for provider '{name}' is empty; set \
                 [generation.{name}].api_key in config.toml"
            ),
            name,
        ));
    }

    // Dispatch on the CANONICAL type. The alias table above is the single
    // place that knows which spellings select which provider, so the
    // `match` arms below name one provider each and cannot drift apart
    // from the table the probe reads.
    let provider_type = canonical_provider_type(&config.provider_type);

    let provider: Arc<dyn GenerationProvider> = match provider_type.as_ref() {
        "openai" => Arc::new(
            OpenAiImageProvider::new(
                api_key,
                config.base_url.clone(),
                default_model.clone(),
                resolved_url,
            )?
            // Honor the `timeout_seconds` config knob.
            .with_timeout(config.request_timeout_secs())?,
        ),
        "openai_tts" => Arc::new(
            OpenAiTtsProvider::new(
                api_key,
                config.base_url.clone(),
                default_model.clone(),
                config.defaults.voice.clone(),
                resolved_url,
            )?
            // Honor the (previously dead) `timeout_seconds` config knob.
            .with_timeout(config.request_timeout_secs())?,
        ),
        "openai_whisper" => Arc::new(
            OpenAiWhisperProvider::new(
                api_key,
                config.base_url.clone(),
                default_model.clone(),
                resolved_url,
            )?
            // Honor the `timeout_seconds` config knob.
            .with_timeout(config.request_timeout_secs())?,
        ),
        "deepgram_stt" => Arc::new(
            DeepgramSttProvider::new(
                api_key,
                config.base_url.clone(),
                default_model.clone(),
            )?
            // Honor the `timeout_seconds` config knob.
            .with_timeout(config.request_timeout_secs())?,
        ),
        "deepgram_tts" => Arc::new(
            DeepgramTtsProvider::new(
                api_key,
                config.base_url.clone(),
                default_model.clone(),
            )?
            // Honor the `timeout_seconds` config knob.
            .with_timeout(config.request_timeout_secs())?,
        ),
        "azure_speech" => Arc::new(
            AzureSpeechProvider::new(
                api_key,
                config.base_url.clone(),
                config
                    .defaults
                    .voice
                    .clone()
                    .or_else(|| default_model.clone()),
            )?
            // Honor the (previously dead) `timeout_seconds` config knob.
            .with_timeout(config.request_timeout_secs())?,
        ),
        "suno" => Arc::new(
            SunoProvider::new(
                api_key,
                config.base_url.clone(),
                default_model.clone(),
            )?
            // Honor the `timeout_seconds` config knob.
            .with_timeout(config.request_timeout_secs())?,
        ),
        "bfl" => Arc::new(
            BflProvider::new(
                api_key,
                config.base_url.clone(),
                default_model.clone(),
            )?
            // Honor the `timeout_seconds` config knob.
            .with_timeout(config.request_timeout_secs())?,
        ),
        "cartesia" => Arc::new(
            CartesiaProvider::new(
                api_key,
                config.base_url.clone(),
                default_model.clone(),
                config.defaults.voice.clone(),
            )?
            // Honor the `timeout_seconds` config knob.
            .with_timeout(config.request_timeout_secs())?,
        ),
        "minimax_tts" => Arc::new(
            MinimaxTtsProvider::new(
                api_key,
                config.base_url.clone(),
                default_model.clone(),
                config.defaults.voice.clone(),
            )?
            // Honor the `timeout_seconds` config knob.
            .with_timeout(config.request_timeout_secs())?,
        ),
        "volcengine_tts" => Arc::new(
            VolcengineTtsProvider::new(
                api_key,
                config.base_url.clone(),
                default_model.clone(),
                config.defaults.voice.clone(),
            )?
            // Honor the `timeout_seconds` config knob.
            .with_timeout(config.request_timeout_secs())?,
        ),
        "openai_compat" => {
            let base_url = config.base_url.clone().ok_or_else(|| {
                GenerationError::invalid_parameters(
                    "base_url is required for openai_compat provider",
                    Some("base_url".to_string()),
                )
            })?;

            let mut builder = OpenAiCompatProvider::builder(name, &api_key, &base_url);

            if let Some(model) = default_model.as_deref() {
                builder = builder.model(model);
            }

            builder = builder.color(&config.color);

            // Honor the (previously dead) `timeout_seconds` config knob.
            builder = builder.timeout_secs(config.request_timeout_secs());

            if let Some(ref edit_url) = config.edit_url {
                builder = builder.edit_endpoint(edit_url);
            }

            // Use capabilities directly (already Vec<GenerationType>)
            if !config.capabilities.is_empty() {
                builder = builder.supported_types(config.capabilities.clone());
            }

            Arc::new(builder.build()?)
        }
        "stability" => Arc::new(
            StabilityImageProvider::new(
                api_key,
                config.base_url.clone(),
                default_model.clone(),
            )?
            // Honor the `timeout_seconds` config knob.
            .with_timeout(config.request_timeout_secs())?,
        ),
        "google" => Arc::new(
            GoogleImagenProvider::new(
                api_key,
                config.base_url.clone(),
                default_model.clone(),
            )?
            // Honor the `timeout_seconds` config knob.
            .with_timeout(config.request_timeout_secs())?,
        ),
        "google_veo" => Arc::new(
            GoogleVeoProvider::new(
                api_key,
                config.base_url.clone(),
                default_model.clone(),
            )?
            // Honor the `timeout_seconds` config knob.
            .with_timeout(config.request_timeout_secs())?,
        ),
        "replicate" => {
            let mut builder = ReplicateProvider::builder(&api_key);

            if let Some(base_url) = &config.base_url {
                builder = builder.endpoint(base_url);
            }

            // Add model as "default" alias if specified
            if let Some(model) = default_model.as_deref() {
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

            // Honor the `timeout_seconds` config knob.
            builder = builder.timeout_secs(config.request_timeout_secs());

            // Use `try_build` so a TLS / DNS resolver misconfiguration
            // surfaces at startup instead of silently producing a provider
            // whose reqwest::Client has no per-request timeout. The other
            // arms in this factory already use `with_timeout` for the same
            // reason.
            Arc::new(builder.try_build()?)
        }
        "elevenlabs" => Arc::new(
            ElevenLabsProvider::new(
                api_key,
                config.base_url.clone(),
                default_model.clone(),
                config.defaults.voice.clone(),
            )?
            // Honor the `timeout_seconds` config knob.
            .with_timeout(config.request_timeout_secs())?,
        ),
        "fal" => {
            // Fal serves image/video/music behind a single queue API.
            // `capabilities` from config determines which modalities this
            // preset will accept (factory rejects mismatches via supports()).
            let mut builder = FalProvider::builder(name, &api_key);
            if let Some(base_url) = &config.base_url {
                builder = builder.endpoint(base_url);
            }
            if let Some(model) = default_model.as_deref() {
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
            // Honor the `timeout_seconds` config knob. This is the PER-REQUEST
            // cap; the job's wall clock is `JOB_DEADLINE_SECS` and is not this.
            builder = builder.timeout_secs(config.request_timeout_secs());
            Arc::new(builder.build()?)
        }
        "midjourney" => {
            let mut builder = MidjourneyProvider::builder(&api_key);

            if let Some(base_url) = &config.base_url {
                builder = builder.endpoint(base_url);
            }

            // Check for mode in extra config or model field
            if let Some(model) = default_model.as_deref() {
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
            // Unconfigured this is a no-op and Midjourney keeps its own 30 s
            // `DEFAULT_REQUEST_TIMEOUT_SECS`. It briefly did not: while the
            // config field was a plain `u64` there was no way to tell "unset"
            // from "120", so for one commit this line moved the unconfigured
            // default 30 -> 120. Poll CADENCE was never affected -- that comes
            // from `POLL_INTERVAL_SECS` / `MAX_POLL_ATTEMPTS`.
            builder = builder.timeout_secs(config.request_timeout_secs());

            Arc::new(builder.build()?)
        }
        other => {
            let supported: Vec<&str> = PROVIDER_TYPE_ALIASES
                .iter()
                .map(|(alias, _)| *alias)
                .collect();
            return Err(GenerationError::invalid_parameters(
                format!(
                    "Unknown provider type: '{other}'. Supported: {}",
                    supported.join(", ")
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

    Ok(DefaultsOverlay::wrap(
        provider,
        config.defaults.to_params(),
        &config.model_aliases,
        configured_color(config),
    ))
}

/// Applies the configured `[generation.providers.<name>.defaults]` to every
/// request the provider receives.
///
/// Exactly two of the fourteen default fields used to reach a provider:
/// `voice` and `format`, both copied into the provider at construction time.
/// The other twelve -- `width`, `height`, `aspect_ratio`, `quality`, `style`,
/// `n`, `duration_seconds`, `fps`, `speed`, `language`, `guidance_scale`,
/// `steps` -- were written by an operator, checked by
/// `GenerationDefaults::validate`, and then silently dropped: nothing
/// between the config file and the outgoing request ever read them. A
/// config that says `width = 1024` behaved exactly like one that said
/// nothing, and there was no way to tell the two apart from the outside.
///
/// The overlay merges the configured defaults UNDER the request's own
/// parameters, so a value the caller set explicitly still wins.
///
/// It also resolves `model_aliases`: a request that names a model by its
/// friendly alias has to reach the provider under the real model ID, and
/// the alias map is the only place that mapping exists.
struct DefaultsOverlay {
    inner: Arc<dyn GenerationProvider>,
    defaults: GenerationParams,
    model_aliases: HashMap<String, String>,
    /// The operator's colour, when they expressed one.
    ///
    /// `None` means "no opinion": the provider keeps the brand colour it
    /// ships with. The config default is the neutral grey
    /// (`DEFAULT_GENERATION_COLOR`), and every provider entry carries that
    /// default whether or not the operator ever touched the knob -- so the
    /// default has to be recognisable as "unset" rather than applied on top
    /// of the brand colours.
    color: Option<String>,
}

impl DefaultsOverlay {
    /// Wrap `inner` so that `defaults` are applied to every request, or
    /// return `inner` unchanged when nothing is configured.
    fn wrap(
        inner: Arc<dyn GenerationProvider>,
        defaults: GenerationParams,
        model_aliases: &HashMap<String, String>,
        color: Option<String>,
    ) -> Arc<dyn GenerationProvider> {
        if !defaults.configure_any_field() && model_aliases.is_empty() && color.is_none() {
            return inner;
        }
        Arc::new(Self {
            inner,
            defaults,
            model_aliases: model_aliases.clone(),
            color,
        })
    }
}

impl GenerationProvider for DefaultsOverlay {
    fn generate(
        &self,
        mut request: GenerationRequest,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = GenerationResult<GenerationOutput>> + Send + '_,
        >,
    > {
        request.params = self.defaults.merged_with(request.params);
        if let Some(model) = request.params.model.as_deref() {
            if let Some(real_model) = self.model_aliases.get(model) {
                request.params.model = Some(real_model.clone());
            }
        }
        self.inner.generate(request)
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn supported_types(&self) -> Vec<GenerationType> {
        self.inner.supported_types()
    }

    fn color(&self) -> &str {
        self.color.as_deref().unwrap_or_else(|| self.inner.color())
    }

    fn default_model(&self) -> Option<&str> {
        self.inner.default_model()
    }

    fn check_progress(
        &self,
        job_id: &str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = GenerationResult<GenerationProgress>> + Send + '_>,
    > {
        self.inner.check_progress(job_id)
    }

    fn cancel(
        &self,
        job_id: &str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = GenerationResult<()>> + Send + '_>,
    > {
        self.inner.cancel(job_id)
    }

    fn edit_image(
        &self,
        request: GenerationRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = GenerationResult<GenerationOutput>> + Send + '_>,
    > {
        self.inner.edit_image(request)
    }

    fn supports_image_editing(&self) -> bool {
        self.inner.supports_image_editing()
    }

    fn list_voices(&self) -> Vec<VoiceInfo> {
        self.inner.list_voices()
    }
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

    /// EVERY canonical provider type the alias table promises has a
    /// dispatch arm.
    ///
    /// The alias table is what the connectivity probe reads to find a preset,
    /// what the "Unknown provider type" error lists, and what a config's
    /// `provider_type` is resolved against. A canonical type that is in the
    /// table but has no arm would fall through to the catch-all and answer
    /// "Unknown provider type" for a spelling the table promised -- the same
    /// class of defect the table was written to remove.
    ///
    /// Falsification: delete the `"bfl" =>` arm, or add a canonical type to
    /// the table without an arm, and this goes red naming the type.
    #[test]
    fn every_canonical_provider_type_has_a_dispatch_arm() {
        let src = include_str!("factory.rs");
        let after = src
            .split_once("match provider_type.as_ref() {")
            .expect("create_provider still dispatches on the canonical provider type")
            .1;
        let body = &after[..after
            .find("        other => {")
            .expect("the match still ends in a catch-all arm")];

        let mut canonicals: Vec<&str> = PROVIDER_TYPE_ALIASES.iter().map(|(_, c)| *c).collect();
        canonicals.sort_unstable();
        canonicals.dedup();

        for canonical in canonicals {
            if canonical == "local" {
                // `local` is answered BEFORE the match: it is a BYO endpoint
                // the user runs, and it needs no API key.
                assert!(
                    src.contains("== \"local\" {"),
                    "the alias table promises the provider type 'local', but \
                     the early local branch is gone"
                );
                continue;
            }
            assert!(
                body.contains(&format!("\"{canonical}\" =>")),
                "the alias table promises the provider type '{canonical}', but \
                 the dispatch has no arm for it"
            );
        }
    }

    /// No spelling appears twice in the alias table.
    ///
    /// A duplicated alias would make the table's meaning depend on the order
    /// of its rows, which is the kind of "the same fact in two places" that
    /// the table exists to remove.
    #[test]
    fn the_alias_table_has_no_duplicate_spelling() {
        let mut seen: Vec<&str> = PROVIDER_TYPE_ALIASES.iter().map(|(alias, _)| *alias).collect();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(
            before,
            seen.len(),
            "the alias table spells the same provider type twice"
        );
    }

    /// An alias resolves to the canonical type it is documented to select.
    ///
    /// Falsification: change the table row for "dalle" to point at
    /// "openai_tts" and this goes red.
    #[test]
    fn an_alias_resolves_to_its_canonical_type() {
        assert_eq!(canonical_provider_type("dalle"), "openai");
        assert_eq!(canonical_provider_type("tts"), "openai_tts");
        assert_eq!(canonical_provider_type("mj"), "midjourney");
        assert_eq!(canonical_provider_type("veo"), "google_veo");
        // An unknown spelling is passed through, lower-cased, so the
        // catch-all arm can name it.
        assert_eq!(canonical_provider_type("Some_New_Provider"), "some_new_provider");
    }

    /// EVERY arm of `create_provider`'s match applies `timeout_seconds`.
    ///
    /// A count taken from the source rather than a number written here: a
    /// hand-computed literal has to be edited when an arm is added, and the
    /// version that does not get edited is the one that stops constraining
    /// anything.
    ///
    /// This exists because the knob has been connected THREE times, one arm at
    /// a time. Two arms carried "Honor the (previously dead)
    /// `timeout_seconds` config knob" from an earlier round while seventeen
    /// others silently dropped it, and clippy only noticed the two whose
    /// builder setter went unused — the other fifteen construct their client
    /// inline, so nothing was unused and nothing was red. Adding an eighteenth
    /// provider and forgetting the knob is what this makes loud (判据 §11:
    /// fix the class at the executor, not each instance you happen to find).
    ///
    /// Falsification: delete `.with_timeout(config.request_timeout_secs())?` from any
    /// arm and this reds with both counts named.
    #[test]
    fn every_provider_arm_applies_the_timeout_knob() {
        let src = include_str!("factory.rs");
        let after = src
            .split_once("match config.provider_type.as_str() {")
            .expect("create_provider still dispatches on provider_type")
            .1;
        // Bounded at the catch-all so this test's OWN source, which mentions
        // `config.request_timeout_secs` below, is not counted as an
        // application.
        let body = &after[..after
            .find("        other => {")
            .expect("the match still ends in a catch-all arm")];

        let arms: Vec<&str> = body
            .lines()
            .filter(|l| l.starts_with("        \""))
            .collect();
        let applied = body.matches("(config.request_timeout_secs())").count();

        assert!(
            arms.len() > 15,
            "the arm scanner found only {} arms -- it stopped matching the \
             source's shape, which makes this guard green for the wrong reason \
             (判据 §3)",
            arms.len()
        );
        assert_eq!(
            applied,
            arms.len(),
            "{} provider arms but {applied} apply `timeout_seconds`; arms are:\n{}",
            arms.len(),
            arms.join("\n")
        );
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
