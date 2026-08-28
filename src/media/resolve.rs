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

use crate::config::types::voice_local::LOCAL_PROVIDER_TYPE;
use crate::config::{GenerationConfig, GenerationProviderConfig};
use crate::media::transcription::{TranscriptionConfigError, TranscriptionService};

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
///
/// # Errors
/// `Err` means a provider *is* configured and was refused (see
/// [`TranscriptionConfigError`]) — distinct from `Ok(None)`, which means
/// nothing is configured. Callers must keep those apart: reporting a rejection
/// as absence hands the operator the one answer that cannot lead them to the
/// setting they need to fix.
pub fn transcription_service(
    gen: &GenerationConfig,
    vault_lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<ResolvedTranscription>, TranscriptionConfigError> {
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

    // The entry's *name* rides along with its config: it is the only thing a
    // rejection can point at, and neither the key nor the URL carries it.
    let picked = {
        // Surface the case where the operator explicitly named a default
        // provider but then disabled it. Silently falling through to any
        // enabled entry is a worse answer than a startup log line that
        // names the override.
        if let Some(name) = gen.default_transcription_provider.as_ref() {
            if let Some(pcfg) = gen.transcription_providers.get(name) {
                if !pcfg.enabled {
                    tracing::warn!(
                        provider = %name,
                        "default_transcription_provider is disabled; falling back to any enabled entry"
                    );
                }
            }
        }
        gen.default_transcription_provider
            .as_ref()
            .and_then(|name| {
                gen.transcription_providers
                    .get(name)
                    .filter(|pcfg| pcfg.enabled)
                    .and_then(|pcfg| resolve_key(name, pcfg).map(|key| (name.clone(), key, pcfg)))
            })
    }
        .or_else(|| {
            gen.transcription_providers.iter().find_map(|(name, pcfg)| {
                if pcfg.enabled {
                    resolve_key(name, pcfg).map(|key| (name.clone(), key, pcfg))
                } else {
                    None
                }
            })
        });
    let Some((name, key, pcfg)) = picked else {
        return Ok(None);
    };

    if pcfg.provider_type == LOCAL_PROVIDER_TYPE {
        // BYO endpoint: connection values live on the entry itself.
        Ok(Some(ResolvedTranscription {
            service: Box::new(
                crate::gateway::voice::local_provider::LocalTranscription::from_config(pcfg),
            ),
            label: "local voice transcription enabled (BYO endpoint)",
        }))
    } else {
        // Deliberately no fall-through to the next enabled entry: silently
        // running on a backend the operator did not name is a worse answer than
        // saying which one was refused.
        let whisper = crate::media::whisper::WhisperTranscription::new(
            &name,
            key,
            pcfg.base_url.clone(),
            pcfg.models.first().cloned(),
        )?;
        Ok(Some(ResolvedTranscription {
            service: Box::new(whisper),
            label: "Whisper transcription enabled (from transcription provider)",
        }))
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
        assert!(transcription_service(&gen, &|_| None)
            .expect("no provider is not a rejection")
            .is_none());
    }

    /// Disabled entries are not a backend, however complete they look.
    #[test]
    fn skips_disabled_entries() {
        let mut gen = GenerationConfig::default();
        gen.transcription_providers
            .insert("openai".into(), provider("openai", false, Some("sk-x")));
        assert!(transcription_service(&gen, &|_| None)
            .expect("a disabled entry is not a rejection")
            .is_none());
    }

    /// `api_key` is `#[serde(skip)]`, so a configured provider looks keyless
    /// until the vault is consulted. Missing that hop is what makes a
    /// configured backend read as "not configured".
    #[test]
    fn falls_back_to_vault_for_the_key() {
        let mut gen = GenerationConfig::default();
        gen.transcription_providers
            .insert("openai".into(), provider("openai", true, None));
        assert!(transcription_service(&gen, &|_| None)
            .expect("a keyless entry is not a rejection")
            .is_none());
        let resolved = transcription_service(&gen, &|name| {
            (name == "openai").then(|| "sk-from-vault".to_string())
        })
        .expect("a valid endpoint is not rejected")
        .expect("vault key resolves the provider");
        assert!(resolved.label.contains("Whisper"));
    }

    /// A local BYO endpoint is valid with no key at all.
    #[test]
    fn local_endpoint_needs_no_key() {
        let mut gen = GenerationConfig::default();
        gen.transcription_providers
            .insert("local".into(), provider(LOCAL_PROVIDER_TYPE, true, None));
        let resolved = transcription_service(&gen, &|_| None)
            .expect("not rejected")
            .expect("local resolves");
        assert!(resolved.label.contains("BYO"));
    }

    #[test]
    fn named_default_wins_over_arbitrary_enabled_entry() {
        let mut gen = GenerationConfig::default();
        gen.transcription_providers
            .insert("openai".into(), provider("openai", true, Some("sk-a")));
        gen.transcription_providers
            .insert("byo".into(), provider(LOCAL_PROVIDER_TYPE, true, None));
        gen.default_transcription_provider = Some("byo".into());
        let resolved = transcription_service(&gen, &|_| None)
            .expect("not rejected")
            .expect("resolves");
        assert!(resolved.label.contains("BYO"));
    }

    /// The whole point of the `Result`: a configured-but-refused entry must not
    /// come back as `Ok(None)`. That reading is what sends an operator looking
    /// for a provider they already added, and it is the shape this replaced —
    /// except the old shape did not return at all, it aborted the process.
    #[test]
    fn a_refused_endpoint_is_a_rejection_not_an_absence() {
        let mut gen = GenerationConfig::default();
        let mut pcfg = provider("openai", true, Some("sk-x"));
        pcfg.base_url = Some("http://whisper.example.com/v1".into());
        gen.transcription_providers.insert("openai".into(), pcfg);

        let Err(err) = transcription_service(&gen, &|_| None) else {
            panic!("a plain-HTTP non-loopback endpoint must be refused");
        };
        assert_eq!(err.provider, "openai");
        assert_eq!(err.field, "base_url");
    }

    /// A refused entry does not hand the turn to whatever else happens to be
    /// enabled: running on a backend the operator did not name is a quieter
    /// failure than saying which one was refused.
    #[test]
    fn a_refused_named_default_does_not_fall_through_to_another_entry() {
        let mut gen = GenerationConfig::default();
        let mut bad = provider("openai", true, Some("sk-x"));
        bad.base_url = Some("http://whisper.example.com/v1".into());
        gen.transcription_providers.insert("openai".into(), bad);
        gen.transcription_providers
            .insert("byo".into(), provider(LOCAL_PROVIDER_TYPE, true, None));
        gen.default_transcription_provider = Some("openai".into());

        let Err(err) = transcription_service(&gen, &|_| None) else {
            panic!("a plain-HTTP non-loopback endpoint must be refused");
        };
        assert_eq!(err.provider, "openai");
    }
}
