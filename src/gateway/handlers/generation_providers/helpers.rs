use crate::config::types::generation::GenerationProviderConfig;
use crate::config::GenerationConfig;
use crate::generation::GenerationType;

/// Get an immutable reference to the typed provider map for a given generation type string.
pub(crate) fn get_typed_provider_map<'a>(
    config: &'a GenerationConfig,
    gen_type_str: &str,
) -> &'a std::collections::HashMap<String, GenerationProviderConfig> {
    match gen_type_str {
        "image" => &config.image_providers,
        "video" => &config.video_providers,
        "speech" => &config.speech_providers,
        "audio" => &config.audio_providers,
        "transcription" => &config.transcription_providers,
        _ => &config.providers, // fallback to legacy
    }
}

/// Get a mutable reference to the typed provider map for a given generation type string.
pub(crate) fn get_typed_provider_map_mut<'a>(
    config: &'a mut GenerationConfig,
    gen_type_str: &str,
) -> &'a mut std::collections::HashMap<String, GenerationProviderConfig> {
    match gen_type_str {
        "image" => &mut config.image_providers,
        "video" => &mut config.video_providers,
        "speech" => &mut config.speech_providers,
        "audio" => &mut config.audio_providers,
        "transcription" => &mut config.transcription_providers,
        _ => &mut config.providers,
    }
}

/// Find which typed map contains a provider by name. Returns the type string.
pub(crate) fn find_provider_type(config: &GenerationConfig, name: &str) -> Option<String> {
    if config.image_providers.contains_key(name) {
        return Some("image".to_string());
    }
    if config.video_providers.contains_key(name) {
        return Some("video".to_string());
    }
    if config.speech_providers.contains_key(name) {
        return Some("speech".to_string());
    }
    if config.audio_providers.contains_key(name) {
        return Some("audio".to_string());
    }
    if config.transcription_providers.contains_key(name) {
        return Some("transcription".to_string());
    }
    // Legacy: derive from capabilities
    if let Some(cfg) = config.providers.get(name) {
        if let Some(gen_type) = cfg.capabilities.first() {
            return Some(format!("{gen_type:?}").to_lowercase());
        }
        // Legacy provider with no capabilities — treat as legacy bucket
        return Some("legacy".to_string());
    }
    None
}

/// Check if a provider exists in any map (typed or legacy).
pub(crate) fn provider_exists(config: &GenerationConfig, name: &str) -> bool {
    config.image_providers.contains_key(name)
        || config.video_providers.contains_key(name)
        || config.speech_providers.contains_key(name)
        || config.audio_providers.contains_key(name)
        || config.transcription_providers.contains_key(name)
        || config.providers.contains_key(name)
}

/// The modalities this provider is the configured default for.
///
/// `list` and `get` both need it and both used to spell out the same five
/// `if` statements, which is two tables of one fact: a sixth modality would
/// have had to be remembered twice (判据 §1).
pub(crate) fn default_modalities(config: &GenerationConfig, name: &str) -> Vec<GenerationType> {
    [
        (&config.default_image_provider, GenerationType::Image),
        (&config.default_video_provider, GenerationType::Video),
        (&config.default_audio_provider, GenerationType::Audio),
        (&config.default_speech_provider, GenerationType::Speech),
        (
            &config.default_transcription_provider,
            GenerationType::Transcription,
        ),
    ]
    .into_iter()
    .filter(|(configured, _)| configured.as_deref() == Some(name))
    .map(|(_, gen_type)| gen_type)
    .collect()
}

/// Parse a generation type string into a `GenerationType` enum.
pub(crate) fn parse_generation_type(s: &str) -> GenerationType {
    match s {
        "video" => GenerationType::Video,
        "speech" => GenerationType::Speech,
        "audio" => GenerationType::Audio,
        "transcription" => GenerationType::Transcription,
        _ => GenerationType::Image,
    }
}
