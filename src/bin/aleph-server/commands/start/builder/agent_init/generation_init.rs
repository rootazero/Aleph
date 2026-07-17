//! Generation-provider registry: initial build + hot-reload subscriber.

use alephcore::generation::{providers as gen_providers, GenerationProviderRegistry};
use alephcore::sync_primitives::{Arc, RwLock};

/// Vault-hydrate a provider's api_key and decide whether it may register.
/// Shared by the boot build and the hot-reload rebuild so their gate rules
/// cannot drift.
///
/// The BYO local voice endpoint (`provider_type == "local"`) is exempt from
/// the key requirement: those servers are commonly unauthenticated
/// (`normalize_voice_local` injects `api_key = Some("")`), and the factory's
/// dedicated local arm never reads the key. Without the exemption the default
/// `[voice.local]` TTS provider was silently dropped here at boot — dead
/// voice output while `local_voice status` reported everything OK.
fn hydrate_key_and_gate(
    name: &str,
    provider_cfg: &mut alephcore::GenerationProviderConfig,
    vault: &alephcore::gateway::security::SharedTokenManager,
) -> bool {
    if provider_cfg.provider_type == alephcore::LOCAL_PROVIDER_TYPE {
        return true;
    }
    // Resolve API key from vault if not in config. Some("") counts as "no
    // key" so a provider carrying an empty-string key still hydrates from
    // vault (otherwise a Panel edit drops a provider that worked at boot).
    if provider_cfg
        .api_key
        .as_ref()
        .is_none_or(std::string::String::is_empty)
    {
        if let Ok(Some(secret)) = vault.get_secret(&format!("gen:{name}")) {
            provider_cfg.api_key = Some(secret.expose().to_string());
        }
    }
    provider_cfg.api_key.as_ref().is_some_and(|k| !k.is_empty())
}

/// Build the generation provider registry from `app_config.generation` (with
/// vault-resolved keys) and spawn the panel hot-reload listener. Returns the
/// shared registry handle that gets stored on `AgentHandlersResult`.
pub(super) fn init_generation_registry(
    app_config: &alephcore::Config,
    app_config_arc: Arc<tokio::sync::RwLock<alephcore::Config>>,
    event_bus: Arc<alephcore::gateway::event_bus::GatewayEventBus>,
    shared_token_mgr: Arc<alephcore::gateway::security::SharedTokenManager>,
    daemon: bool,
) -> Arc<RwLock<GenerationProviderRegistry>> {
    let registry = {
        let mut registry = GenerationProviderRegistry::new();
        for (name, mut provider_cfg, gen_type) in app_config.generation.merged_providers() {
            if !provider_cfg.enabled {
                continue;
            }
            if !hydrate_key_and_gate(&name, &mut provider_cfg, &shared_token_mgr) {
                continue;
            }
            match gen_providers::create_provider(&name, &provider_cfg, gen_type) {
                Ok(provider) => {
                    if registry.register(name.clone(), provider).is_ok() {
                        tracing::info!(provider = %name, gen_type = ?gen_type, "Registered generation provider");
                    }
                }
                Err(e) => {
                    tracing::warn!(provider = %name, error = %e, "Skip generation provider");
                }
            }
        }
        if !registry.is_empty() && !daemon {
            println!("  Generation providers: {} registered", registry.len());
        }
        Arc::new(RwLock::new(registry))
    };

    // Hot-reload: rebuild generation registry when Panel updates providers
    {
        let gen_reg = registry.clone();
        let config_handle = app_config_arc;
        let vault = shared_token_mgr;
        let mut rx = event_bus.subscribe();

        tokio::spawn(async move {
            while let Ok(event_json) = rx.recv().await {
                let is_gen_event = serde_json::from_str::<serde_json::Value>(&event_json)
                    .ok()
                    .and_then(|v| {
                        v.get("topic")?
                            .as_str()
                            .map(std::string::ToString::to_string)
                    })
                    == Some("config.generation.providers.changed".to_string());
                if !is_gen_event {
                    continue;
                }

                // Snapshot merged providers (drop read guard before creating providers)
                let merged_snapshot = {
                    let cfg = config_handle.read().await;
                    cfg.generation.merged_providers()
                };

                let mut new_registry = GenerationProviderRegistry::new();
                for (name, mut provider_cfg, gen_type) in merged_snapshot {
                    if !provider_cfg.enabled {
                        continue;
                    }
                    if !hydrate_key_and_gate(&name, &mut provider_cfg, &vault) {
                        continue;
                    }
                    match gen_providers::create_provider(&name, &provider_cfg, gen_type) {
                        Ok(provider) => {
                            new_registry.register(name.clone(), provider).ok();
                        }
                        Err(e) => {
                            tracing::warn!(
                                provider = %name, error = %e,
                                "Skip generation provider on reload"
                            );
                        }
                    }
                }

                let mut guard = gen_reg
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *guard = new_registry;
                tracing::info!(
                    "Generation provider registry reloaded ({} providers)",
                    guard.len()
                );
            }
        });
    }

    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use alephcore::gateway::security::{SecurityStore, SharedTokenManager};

    fn vault() -> (tempfile::TempDir, SharedTokenManager) {
        let dir = tempfile::TempDir::new().unwrap();
        let store = std::sync::Arc::new(SecurityStore::in_memory().unwrap());
        let mgr = SharedTokenManager::new(store, dir.path().join("test.vault"));
        (dir, mgr)
    }

    // Regression: `normalize_voice_local` injects `api_key = Some("")` for the
    // unauthenticated BYO endpoint; the key gate must not eat it (it did — the
    // default [voice.local] TTS provider never registered, so voice replies
    // were silently dead while the status tool reported OK).
    #[test]
    fn local_provider_registers_without_api_key() {
        let (_dir, vault) = vault();
        let mut cfg = alephcore::GenerationProviderConfig::new(alephcore::LOCAL_PROVIDER_TYPE);
        cfg.api_key = Some(String::new());
        assert!(hydrate_key_and_gate("local", &mut cfg, &vault));
    }

    #[test]
    fn cloud_provider_without_key_is_gated() {
        let (_dir, vault) = vault();
        let mut cfg = alephcore::GenerationProviderConfig::new("openai_tts");
        cfg.api_key = Some(String::new());
        assert!(!hydrate_key_and_gate("openai_tts", &mut cfg, &vault));
        cfg.api_key = None;
        assert!(!hydrate_key_and_gate("openai_tts", &mut cfg, &vault));
    }

    #[test]
    fn cloud_provider_with_config_key_passes() {
        let (_dir, vault) = vault();
        let mut cfg = alephcore::GenerationProviderConfig::new("openai_tts");
        cfg.api_key = Some("sk-test".into());
        assert!(hydrate_key_and_gate("openai_tts", &mut cfg, &vault));
        assert_eq!(cfg.api_key.as_deref(), Some("sk-test"));
    }
}
