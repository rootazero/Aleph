//! Single join point for TTS voice catalogs.
//!
//! Every speech provider already owns its voice list as `static_voice_list()`
//! (and returns the same list from the `GenerationProvider::list_voices`
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
    canonical_provider_type, AzureSpeechProvider, CartesiaProvider, DeepgramTtsProvider,
    ElevenLabsProvider, MinimaxTtsProvider, OpenAiTtsProvider, VolcengineTtsProvider,
};
use crate::generation::VoiceInfo;

/// The date the static voice catalogs were last audited against the vendor
/// docs. Voice lists are inherently stale-prone — vendors add voices between
/// releases (OpenAI added `sage`/`verse`/`marin`/`cedar` in 2025), and users
/// otherwise have no signal that their picker is out of date.
///
/// This was a `pub const` that nothing read, while its own doc promised it
/// was "surfaced in the Settings RPC payload" — a wire that was never built.
/// The Settings RPC answers with a bare `Vec<VoiceInfo>`, so carrying a date
/// would mean changing that response's shape and the Panel's voice picker
/// with it; that is a contract change, not a wire repair, so it is
/// deliberately NOT done here. Until then the audit date lives here as a
/// maintenance note: bump it whenever a `static_voice_list()` is updated.
#[cfg(test)]
const CATALOG_GENERATED_AT: &str = "2026-08-27";

/// A test that the catalog audit date is still written as a date, so a
/// careless edit ("2026-8-27") is caught here rather than in a UI that will
/// never show it.
#[cfg(test)]
mod catalog_date_tests {
    use super::CATALOG_GENERATED_AT;

    #[test]
    fn the_catalog_audit_date_is_an_iso_date() {
        assert!(
            CATALOG_GENERATED_AT.len() == 10 && CATALOG_GENERATED_AT.contains('-'),
            "the catalog audit date must be written as YYYY-MM-DD"
        );
    }
}

/// Voices known statically for a `provider_type`, using the same aliases the
/// provider factory accepts (`create_provider`) so a type that constructs a
/// speech provider always resolves a catalog here too.
///
/// The provider type is canonicalized with the factory's own alias table
/// (`canonical_provider_type`), so the two cannot drift: before that, this
/// function listed `"openai"` beside the OpenAI TTS voices while the factory
/// maps `"openai"` to the **image** provider, and it listed spellings the
/// factory rejects (`"openai-tts"`, `"openai-compat"`), so a row could get a
/// voice picker for a provider type that cannot even be constructed.
///
/// Returns an empty vec for types with no static catalog — notably `local`
/// (a BYO server's voices are whatever *it* serves, discovered live from
/// `{endpoint}/audio/voices`) and every non-speech type.
///
/// `openai_compat` deliberately has NO catalog: the OpenAI-compatible shim
/// does not implement `list_voices`, so the live registry answers `[]` for
/// it. A static list here would claim voices the provider cannot serve — the
/// catalog and the live list would drift apart, which is exactly what the
/// two accessors are supposed to prevent.
#[must_use]
pub fn static_voices_for_provider_type(provider_type: &str) -> Vec<VoiceInfo> {
    let canonical = canonical_provider_type(provider_type);
    match canonical.as_ref() {
        "openai_tts" => OpenAiTtsProvider::static_voice_list(),
        "elevenlabs" => ElevenLabsProvider::static_voice_list(),
        "minimax_tts" => MinimaxTtsProvider::static_voice_list(),
        "volcengine_tts" => VolcengineTtsProvider::static_voice_list(),
        // Newly reachable: all three ship a static list that no caller could see.
        "cartesia" => CartesiaProvider::static_voice_list(),
        "azure_speech" => AzureSpeechProvider::static_voice_list(),
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
    fn openai_compat_has_no_voice_catalog() {
        // The OpenAI-compatible shim implements no `list_voices`, so the live
        // registry answers `[]` for it. The static catalog must not claim
        // voices the provider cannot serve.
        assert!(static_voices_for_provider_type("openai_compat").is_empty());
    }

    /// The catalog must answer for the SAME provider the factory builds.
    ///
    /// `"openai"` selects the image provider in the factory, so a voice
    /// picker for it was offering OpenAI TTS voices for a provider that
    /// cannot speak.
    #[test]
    fn the_image_provider_type_has_no_voice_catalog() {
        assert!(static_voices_for_provider_type("openai").is_empty());
        assert!(static_voices_for_provider_type("dalle").is_empty());
    }

    /// A spelling the factory rejects must not resolve a catalog either.
    #[test]
    fn spellings_the_factory_rejects_have_no_catalog() {
        assert!(static_voices_for_provider_type("openai-tts").is_empty());
        assert!(static_voices_for_provider_type("openai-compat").is_empty());
    }
}
