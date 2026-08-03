//! Single source for "which transcription backend is configured".
//!
//! Two places need this answer and they must agree: the `MediaProcessor` the
//! server builds at startup (attachment transcription) and the `MediaPipeline`
//! the builtin tool registry builds (the `audio_transcribe` tool). They used to
//! disagree — startup resolved a real backend while the registry registered no
//! audio provider at all, so a user with a configured Whisper key still got
//! "no media pipeline configured" from the tool.
//!
//! Vault access arrives as a closure so this stays free of the token-manager
//! type: `api_key` is `#[serde(skip)]` on `GenerationProviderConfig` and is
//! injected from the vault under `gen:<provider_name>` at runtime.

use crate::config::{GenerationConfig, GenerationProviderConfig};
use crate::config::types::voice_local::LOCAL_PROVIDER_TYPE;
use crate::media::transcription::TranscriptionService;

/// A resolved transcription backend plus a label for startup logging.
pub struct ResolvedTranscription {
    pub service: Box<dyn TranscriptionService>,
    /// Which shape was picked — for the operator-facing startup line.
    pub label: &'static str,
}

/// Resolve the configured transcription provider, if any.
///
/// Preference order matches the rest of `[generation]`: the explicitly named
/// `default_transcription_provider` first, then any enabled entry.
///
/// `vault_lookup` is called with the provider name and should return the
/// secret stored under `gen:<name>`.
#[must_use]
pub fn transcription_service(
    gen: &GenerationConfig,
    vault_lookup: &dyn Fn(&str) -> Option<String>,
) -> Option<ResolvedTranscription> {
    // BYO local endpoints commonly run unauthenticated — an empty key is valid
    // for them (no Authorization header), so the presence walk must not skip
    // the entry.
    let resolve_key = |name: &str, pcfg: &GenerationProviderConfig| -> Option<String> {
        if pcfg.provider_type == LOCAL_PROVIDER_TYPE {
            return Some(pcfg.api_key.clone().unwrap_or_default());
        }
        if let Some(ref key) = pcfg.api_key {
            if !key.is_empty() {
                return Some(key.clone());
            }
        }
        vault_lookup(name).filter(|v| !v.is_empty())
    };

    let (key, pcfg) = gen
        .default_transcription_provider
        .as_ref()
        .and_then(|name| {
            gen.transcription_providers
                .get(name)
                .filter(|pcfg| pcfg.enabled)
                .and_then(|pcfg| resolve_key(name, pcfg).map(|key| (key, pcfg)))
        })
        .or_else(|| {
            gen.transcription_providers.iter().find_map(|(name, pcfg)| {
                if pcfg.enabled {
                    resolve_key(name, pcfg).map(|key| (key, pcfg))
                } else {
                    None
                }
            })
        })?;

    if pcfg.provider_type == LOCAL_PROVIDER_TYPE {
        // BYO endpoint: connection values live on the entry itself.
        Some(ResolvedTranscription {
            service: Box::new(crate::gateway::voice::local_provider::LocalTranscription::from_config(
                pcfg,
            )),
            label: "local voice transcription enabled (BYO endpoint)",
        })
    } else {
        Some(ResolvedTranscription {
            service: Box::new(crate::media::whisper::WhisperTranscription::new(
                key,
                pcfg.base_url.clone(),
                pcfg.models.first().cloned(),
            )),
            label: "Whisper transcription enabled (from transcription provider)",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(provider_type: &str, enabled: bool, key: Option<&str>) -> GenerationProviderConfig {
        GenerationProviderConfig {
            provider_type: provider_type.to_string(),
            enabled,
            api_key: key.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn none_when_nothing_configured() {
        let gen = GenerationConfig::default();
        assert!(transcription_service(&gen, &|_| None).is_none());
    }

    /// Disabled entries are not a backend, however complete they look.
    #[test]
    fn skips_disabled_entries() {
        let mut gen = GenerationConfig::default();
        gen.transcription_providers
            .insert("openai".into(), provider("openai", false, Some("sk-x")));
        assert!(transcription_service(&gen, &|_| None).is_none());
    }

    /// `api_key` is `#[serde(skip)]`, so a configured provider looks keyless
    /// until the vault is consulted. Missing that hop is what makes a
    /// configured backend read as "not configured".
    #[test]
    fn falls_back_to_vault_for_the_key() {
        let mut gen = GenerationConfig::default();
        gen.transcription_providers
            .insert("openai".into(), provider("openai", true, None));
        assert!(transcription_service(&gen, &|_| None).is_none());
        let resolved = transcription_service(&gen, &|name| {
            (name == "openai").then(|| "sk-from-vault".to_string())
        })
        .expect("vault key resolves the provider");
        assert!(resolved.label.contains("Whisper"));
    }

    /// A local BYO endpoint is valid with no key at all.
    #[test]
    fn local_endpoint_needs_no_key() {
        let mut gen = GenerationConfig::default();
        gen.transcription_providers.insert(
            "local".into(),
            provider(LOCAL_PROVIDER_TYPE, true, None),
        );
        let resolved = transcription_service(&gen, &|_| None).expect("local resolves");
        assert!(resolved.label.contains("BYO"));
    }

    #[test]
    fn named_default_wins_over_arbitrary_enabled_entry() {
        let mut gen = GenerationConfig::default();
        gen.transcription_providers
            .insert("openai".into(), provider("openai", true, Some("sk-a")));
        gen.transcription_providers.insert(
            "byo".into(),
            provider(LOCAL_PROVIDER_TYPE, true, None),
        );
        gen.default_transcription_provider = Some("byo".into());
        let resolved = transcription_service(&gen, &|_| None).expect("resolves");
        assert!(resolved.label.contains("BYO"));
    }
}
