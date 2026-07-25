//! Transcription-provider resolution: turn generation config + vault into a
//! ready [`SttSource`], preferring the BYO local endpoint with a cloud
//! fallback riding along (P7 graceful degradation).

use super::stt::{SttConfig, SttSource};

/// Resolve the active STT source from generation config + vault.
///
/// Local (`provider_type == "local"`) wins per the normalized defaults; a cloud
/// candidate (if any) rides along as the fallback. Pure cloud setups produce
/// `Static` exactly as before. Returns `None` when no usable provider exists.
///
/// `vocabulary` is the rendered `[voice] vocabulary` hint
/// ([`VoiceSection::vocabulary_hint`](crate::config::types::voice_local::VoiceSection::vocabulary_hint)),
/// carried onto every config in the source — primary *and* fallback — so a
/// degradation hop does not silently lose the user's proper nouns.
///
/// Shared by the boot path (inbound router wiring) and the `voice.transcribe`
/// panel RPC so both resolve the provider identically.
pub fn resolve_stt_source(
    gen_cfg: &crate::config::types::generation::GenerationConfig,
    vault: &crate::gateway::security::SharedTokenManager,
    vocabulary: Option<&str>,
) -> Option<SttSource> {
    let vocab = vocabulary.unwrap_or_default();
    let chosen = choose_transcription_provider(gen_cfg, vault, false)?;
    if chosen.1.provider_type == crate::config::types::voice_local::LOCAL_PROVIDER_TYPE {
        let config = local_stt_config(chosen.0, chosen.1, vocab);
        let fallback = choose_transcription_provider(gen_cfg, vault, true)
            .map(|(key, pcfg)| Box::new(static_stt_config(key, pcfg, vocab)));
        Some(SttSource::Local { config, fallback })
    } else {
        let (key, pcfg) = chosen;
        Some(SttSource::Static(static_stt_config(key, pcfg, vocab)))
    }
}

/// Selection walk shared by primary + fallback resolution.
/// `skip_local = true` excludes `provider_type == "local"` entries.
///
/// Picks the `default_transcription_provider` when set and enabled, otherwise
/// the first enabled transcription provider with a resolvable API key (config
/// inline or vault `gen:<name>`).
fn choose_transcription_provider<'a>(
    gen_cfg: &'a crate::config::types::generation::GenerationConfig,
    vault: &crate::gateway::security::SharedTokenManager,
    skip_local: bool,
) -> Option<(String, &'a crate::GenerationProviderConfig)> {
    let resolve_key = |name: &str, pcfg: &crate::GenerationProviderConfig| -> Option<String> {
        // BYO local endpoints commonly run unauthenticated — an empty key is
        // valid for them (means "no Authorization header"), so the presence
        // walk must not skip the entry.
        if pcfg.provider_type == crate::config::types::voice_local::LOCAL_PROVIDER_TYPE {
            return Some(pcfg.api_key.clone().unwrap_or_default());
        }
        if let Some(ref key) = pcfg.api_key {
            if !key.is_empty() {
                return Some(key.clone());
            }
        }
        if let Ok(Some(secret)) = vault.get_secret(&format!("gen:{name}")) {
            let val = secret.expose().to_string();
            if !val.is_empty() {
                return Some(val);
            }
        }
        None
    };
    let eligible = |pcfg: &crate::GenerationProviderConfig| {
        pcfg.enabled
            && !(skip_local
                && pcfg.provider_type == crate::config::types::voice_local::LOCAL_PROVIDER_TYPE)
    };

    gen_cfg
        .default_transcription_provider
        .as_ref()
        .and_then(|default_name| {
            gen_cfg
                .transcription_providers
                .get_key_value(default_name)
                .filter(|(_, pcfg)| eligible(pcfg))
                .and_then(|(name, pcfg)| resolve_key(name, pcfg).map(|key| (key, pcfg)))
        })
        .or_else(|| {
            gen_cfg
                .transcription_providers
                .iter()
                .find_map(|(name, pcfg)| {
                    if eligible(pcfg) {
                        resolve_key(name, pcfg).map(|key| (key, pcfg))
                    } else {
                        None
                    }
                })
        })
}

/// `SttConfig` for the BYO local endpoint: `base_url` is the configured
/// endpoint verbatim (no URL rewriting — it already carries its path prefix),
/// and an empty model means "omit the field, let the server default decide".
fn local_stt_config(
    key: String,
    pcfg: &crate::GenerationProviderConfig,
    vocabulary: &str,
) -> SttConfig {
    SttConfig {
        api_key: key,
        base_url: pcfg
            .base_url
            .as_deref()
            .unwrap_or("http://127.0.0.1:8000/v1")
            .trim_end_matches('/')
            .to_string(),
        model: pcfg.models.first().cloned().unwrap_or_default(),
        vocabulary: vocabulary.to_string(),
    }
}

/// The original static `SttConfig` construction (url normalize + model pick).
fn static_stt_config(
    key: String,
    pcfg: &crate::GenerationProviderConfig,
    vocabulary: &str,
) -> SttConfig {
    let base = pcfg.base_url.as_deref().unwrap_or("https://api.openai.com");
    let resolved = crate::generation::providers::url_normalize::resolve_base_url(base);
    let stt_endpoint = resolved.primary_endpoint(crate::generation::GenerationType::Transcription);
    let stt_base = stt_endpoint
        .trim_end_matches("/audio/transcriptions")
        .to_string();
    let stt_model = pcfg
        .models
        .first()
        .cloned()
        .unwrap_or_else(|| "whisper-1".to_string());

    SttConfig {
        api_key: key,
        base_url: stt_base,
        model: stt_model,
        vocabulary: vocabulary.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_vault() -> (
        tempfile::TempDir,
        crate::gateway::security::SharedTokenManager,
    ) {
        use crate::gateway::security::{SecurityStore, SharedTokenManager};
        let dir = tempfile::TempDir::new().unwrap();
        let store = std::sync::Arc::new(SecurityStore::in_memory().unwrap());
        let vault = SharedTokenManager::new(store, dir.path().join("test.vault"));
        (dir, vault)
    }

    #[test]
    fn resolve_prefers_local_with_cloud_fallback() {
        use crate::config::types::generation::GenerationConfig;
        let (_dir, vault) = make_vault();
        let mut gen = GenerationConfig::default();
        // local (normalized BYO shape: real endpoint, unauthenticated, no
        // model pinned) + one cloud provider with inline key
        let mut local = crate::GenerationProviderConfig::new("local");
        local.api_key = Some(String::new());
        local.base_url = Some("http://127.0.0.1:8000/v1".into());
        gen.transcription_providers.insert("local".into(), local);
        let mut cloud = crate::GenerationProviderConfig::new("openai_whisper");
        cloud.api_key = Some("sk-cloud".into());
        gen.transcription_providers
            .insert("openai_whisper".into(), cloud);
        gen.default_transcription_provider = Some("local".into());

        match resolve_stt_source(&gen, &vault, None) {
            Some(SttSource::Local {
                config,
                fallback: Some(f),
            }) => {
                // Local config built from the entry: endpoint verbatim, empty
                // key (no auth), empty model (server default).
                assert_eq!(config.base_url, "http://127.0.0.1:8000/v1");
                assert_eq!(config.api_key, "");
                assert_eq!(config.model, "");
                assert_eq!(f.api_key, "sk-cloud");
            }
            other => panic!("expected Local with fallback, got {:?}", other.is_some()),
        }
    }

    #[test]
    fn resolve_local_without_cloud_has_no_fallback() {
        use crate::config::types::generation::GenerationConfig;
        let (_dir, vault) = make_vault();
        let mut gen = GenerationConfig::default();
        let mut local = crate::GenerationProviderConfig::new("local");
        // No api_key at all — BYO unauthenticated entries must still resolve.
        local.api_key = None;
        local.base_url = Some("http://127.0.0.1:8000/v1/".into());
        local.models = vec!["whisper-large-v3".into()];
        gen.transcription_providers.insert("local".into(), local);
        gen.default_transcription_provider = Some("local".into());

        match resolve_stt_source(&gen, &vault, None) {
            Some(SttSource::Local {
                config,
                fallback: None,
            }) => {
                // Trailing slash trimmed; configured model carried.
                assert_eq!(config.base_url, "http://127.0.0.1:8000/v1");
                assert_eq!(config.model, "whisper-large-v3");
            }
            _ => panic!("expected Local without fallback"),
        }
    }

    #[test]
    fn vocabulary_reaches_the_fallback_too() {
        // A local→cloud degradation hop must not silently lose the user's proper
        // nouns — the whole point of the hint is that it survives the retry.
        use crate::config::types::generation::GenerationConfig;
        let (_dir, vault) = make_vault();
        let mut gen = GenerationConfig::default();
        let mut local = crate::GenerationProviderConfig::new("local");
        local.api_key = Some(String::new());
        local.base_url = Some("http://127.0.0.1:8000/v1".into());
        gen.transcription_providers.insert("local".into(), local);
        let mut cloud = crate::GenerationProviderConfig::new("openai_whisper");
        cloud.api_key = Some("sk-cloud".into());
        gen.transcription_providers
            .insert("openai_whisper".into(), cloud);
        gen.default_transcription_provider = Some("local".into());

        match resolve_stt_source(&gen, &vault, Some("Aleph, Leptos")) {
            Some(SttSource::Local {
                config,
                fallback: Some(f),
            }) => {
                assert_eq!(config.vocabulary, "Aleph, Leptos");
                assert_eq!(f.vocabulary, "Aleph, Leptos");
            }
            _ => panic!("expected Local with fallback"),
        }
    }

    #[test]
    fn resolve_pure_cloud_stays_static() {
        use crate::config::types::generation::GenerationConfig;
        let (_dir, vault) = make_vault();
        let mut gen = GenerationConfig::default();
        let mut cloud = crate::GenerationProviderConfig::new("openai_whisper");
        cloud.api_key = Some("sk-cloud".into());
        gen.transcription_providers
            .insert("openai_whisper".into(), cloud);
        match resolve_stt_source(&gen, &vault, None) {
            Some(SttSource::Static(cfg)) => assert_eq!(cfg.api_key, "sk-cloud"),
            _ => panic!("expected Static"),
        }
    }
}
