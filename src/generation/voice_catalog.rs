//! Single join point for TTS voice catalogs.
//!
//! Every speech provider already owns its voice list as `static_voice_list()`
//! (and returns the same list from the [`GenerationProvider::list_voices`]
//! trait method). What was missing was one place that maps a *`provider_type`*
//! to that list, so callers who only hold config — the Settings RPC
//! (`generation_providers.voices`) — could reach it without hand-maintaining a
//! second table.
//!
//! Before this module the Settings handler carried its own `match provider_type`
//! covering only 3 of the 7 speech providers, so `cartesia`, `azure_speech` and
//! `deepgram_tts` presented an **empty voice picker** even though each ships a
//! full static list. This is the one table; the provider's own
//! `static_voice_list()` remains the single content source, so the two
//! accessors (this one, by type; [`GenerationProviderRegistry::get_voices_for_provider`]
//! by live registry entry) cannot drift.
//!
//! [`GenerationProviderRegistry::get_voices_for_provider`]: crate::generation::GenerationProviderRegistry::get_voices_for_provider

use crate::generation::providers::{
    AzureSpeechProvider, CartesiaProvider, DeepgramTtsProvider, ElevenLabsProvider,
    MinimaxTtsProvider, OpenAiTtsProvider, VolcengineTtsProvider,
};
use crate::generation::VoiceInfo;

/// Voices known statically for a `provider_type`, using the same aliases the
/// provider factory accepts (`create_provider`) so a type that constructs a
/// speech provider always resolves a catalog here too.
///
/// Returns an empty vec for types with no static catalog — notably `local`
/// (a BYO server's voices are whatever *it* serves, discovered live from
/// `{endpoint}/audio/voices`) and every non-speech type.
#[must_use]
pub fn static_voices_for_provider_type(provider_type: &str) -> Vec<VoiceInfo> {
    match provider_type.trim().to_ascii_lowercase().as_str() {
        // OpenAI TTS and the OpenAI-compatible shim speak the same voice names.
        "openai" | "openai_tts" | "openai-tts" | "tts" | "openai_compat" | "openai-compat" => {
            OpenAiTtsProvider::static_voice_list()
        }
        "elevenlabs" => ElevenLabsProvider::static_voice_list(),
        "minimax_tts" => MinimaxTtsProvider::static_voice_list(),
        "volcengine_tts" => VolcengineTtsProvider::static_voice_list(),
        // Newly reachable: all three ship a static list that no caller could see.
        "cartesia" => CartesiaProvider::static_voice_list(),
        "azure_speech" | "azure_tts" => AzureSpeechProvider::static_voice_list(),
        "deepgram_tts" => DeepgramTtsProvider::static_voice_list(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `provider_type` the factory maps to a *speech* provider must
    /// resolve a non-empty catalog — this is the guard against the empty-picker
    /// bug returning when a new speech provider is added to the factory.
    #[test]
    fn every_speech_provider_type_has_a_catalog() {
        for t in [
            "openai_tts",
            "tts",
            "elevenlabs",
            "minimax_tts",
            "volcengine_tts",
            "cartesia",
            "azure_speech",
            "azure_tts",
            "deepgram_tts",
        ] {
            assert!(
                !static_voices_for_provider_type(t).is_empty(),
                "{t} must resolve a voice catalog"
            );
        }
    }

    #[test]
    fn provider_type_matching_is_case_insensitive_and_trimmed() {
        assert!(!static_voices_for_provider_type("  ElevenLabs ").is_empty());
    }

    #[test]
    fn local_and_unknown_types_have_no_static_catalog() {
        // BYO endpoints are discovered live, never guessed.
        assert!(static_voices_for_provider_type("local").is_empty());
        assert!(static_voices_for_provider_type("mystery").is_empty());
        assert!(static_voices_for_provider_type("").is_empty());
    }

    #[test]
    fn openai_compat_reuses_openai_voice_names() {
        let compat = static_voices_for_provider_type("openai_compat");
        assert!(compat.iter().any(|v| v.id == "alloy"));
    }
}
