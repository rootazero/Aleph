//! Generation-provider registry: initial build + hot-reload subscriber.

use alephcore::generation::{providers as gen_providers, GenerationProviderRegistry};
use alephcore::sync_primitives::{Arc, RwLock};

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
            // Resolve API key from vault if not in config
            if provider_cfg
                .api_key
                .as_ref()
                .map(|k| k.is_empty())
                .unwrap_or(true)
            {
                if let Ok(Some(secret)) = shared_token_mgr.get_secret(&format!("gen:{}", name)) {
                    provider_cfg.api_key = Some(secret.expose().to_string());
                }
            }
            if provider_cfg
                .api_key
                .as_ref()
                .map(|k| k.is_empty())
                .unwrap_or(true)
            {
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
                    .and_then(|v| v.get("topic")?.as_str().map(|s| s.to_string()))
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
                    // Resolve API key from vault (RPC handlers store keys in vault, not config)
                    if provider_cfg.api_key.is_none() {
                        if let Ok(Some(secret)) = vault.get_secret(&format!("gen:{}", name)) {
                            provider_cfg.api_key = Some(secret.expose().to_string());
                        }
                    }
                    if provider_cfg
                        .api_key
                        .as_ref()
                        .map(|k| k.is_empty())
                        .unwrap_or(true)
                    {
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

                let mut guard = gen_reg.write().unwrap_or_else(|e| e.into_inner());
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
