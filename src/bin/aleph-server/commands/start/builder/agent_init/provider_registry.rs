//! Multi-provider registry construction.
//!
//! Extracted verbatim from `agent_init/mod.rs`. Builds the
//! `MultiProviderRegistry` by registering every configured + keyed provider,
//! injecting vault-stored API keys, and honoring the configured default.
//!
//! Behavior note: the original inline block assigned the resulting registry to
//! the outer `multi_reg` local *and* returned it as the block value; the two
//! were always equal. The caller now derives `multi_reg` from this fn's return
//! value, preserving that invariant.

use alephcore::sync_primitives::Arc;

use alephcore::gateway::{
    available_provider_from_env, can_create_provider_from_env, create_provider_registry_from_env,
};
use alephcore::ProviderRegistry;

/// Build the multi-provider registry from env + config.
///
/// Returns `Some(registry)` when at least a default provider could be
/// constructed (from env or config), otherwise `None`.
pub(super) fn build_multi_provider_registry(
    app_config: &alephcore::Config,
    shared_token_mgr: &Arc<alephcore::gateway::security::SharedTokenManager>,
    daemon: bool,
) -> Option<Arc<alephcore::MultiProviderRegistry>> {
    use alephcore::providers::create_provider;

    // Read api_key from vault for a given provider name
    let vault_key_for = |name: &str| format!("ai:{name}");
    let vault_lookup = |name: &str| -> Option<String> {
        match shared_token_mgr.get_secret(&vault_key_for(name)) {
            Ok(Some(secret)) => Some(secret.expose().to_string()),
            _ => None,
        }
    };
    // Hydrate a ProviderConfig clone with its vault api_key (if present)
    let hydrate = |name: &str, cfg: &alephcore::ProviderConfig| -> alephcore::ProviderConfig {
        let mut c = cfg.clone();
        if c.api_key.as_ref().is_none_or(std::string::String::is_empty) {
            c.api_key = vault_lookup(name);
        }
        c
    };
    let has_key = |name: &str, cfg: &alephcore::ProviderConfig| -> bool {
        cfg.api_key.as_ref().is_some_and(|k| !k.is_empty()) || vault_lookup(name).is_some()
    };

    // Determine default provider name. When no explicit default is
    // configured, pick the first enabled+keyed provider in NAME ORDER —
    // `providers` is a HashMap, so iterating it directly would pick a
    // different default across restarts (non-deterministic routing).
    let default_name = app_config.general.default_provider.clone().or_else(|| {
        let mut candidates: Vec<(&String, &alephcore::ProviderConfig)> =
            app_config.providers.iter().collect();
        candidates.sort_by(|a, b| a.0.cmp(b.0));
        candidates
            .into_iter()
            .find(|(name, cfg)| cfg.enabled && has_key(name, cfg))
            .map(|(name, _)| name.clone())
    });

    // Try env vars first for the initial provider
    let env_provider = if can_create_provider_from_env() {
        create_provider_registry_from_env().ok().map(|reg| {
            let p = reg.default_provider();
            let name = available_provider_from_env().unwrap_or("env");
            (name, p)
        })
    } else {
        None
    };

    // Build multi-provider registry
    if let Some((env_name, env_prov)) = env_provider {
        let registry = Arc::new(alephcore::MultiProviderRegistry::new(
            env_name.to_string(),
            env_prov,
        ));
        // Also register all config providers
        for (name, provider_cfg) in &app_config.providers {
            if !provider_cfg.enabled || name.as_str() == env_name {
                continue;
            }
            if !has_key(name, provider_cfg) {
                continue;
            }
            let hydrated = hydrate(name, provider_cfg);
            if let Ok(p) = create_provider(name, hydrated) {
                registry.register(name.clone(), p);
                tracing::info!(provider = %name, "Registered provider from config");
            }
        }
        // An env key seeded the registry's initial default. If the operator
        // configured an explicit default_provider that is actually
        // registered, honor it — otherwise the env provider silently wins
        // over the configured choice.
        if let Some(cfg_default) = app_config.general.default_provider.as_deref() {
            if registry
                .list_providers()
                .iter()
                .any(|p| p.as_str() == cfg_default)
            {
                let _ = registry.set_default(cfg_default);
            }
        }
        if !daemon {
            println!(
                "  Providers: {} registered",
                registry.list_providers().len()
            );
        }
        Some(registry as Arc<alephcore::MultiProviderRegistry>)
    } else if let Some(def_name) = default_name {
        // No env provider — create from config
        if let Some(provider_cfg) = app_config.providers.get(&def_name) {
            let default_hydrated = hydrate(&def_name, provider_cfg);
            if let Ok(default_prov) = create_provider(&def_name, default_hydrated) {
                let registry = Arc::new(alephcore::MultiProviderRegistry::new(
                    def_name.clone(),
                    default_prov,
                ));
                // Register remaining providers
                for (name, pcfg) in &app_config.providers {
                    if !pcfg.enabled || name == &def_name {
                        continue;
                    }
                    if !has_key(name, pcfg) {
                        continue;
                    }
                    let hydrated = hydrate(name, pcfg);
                    if let Ok(p) = create_provider(name, hydrated) {
                        registry.register(name.clone(), p);
                        tracing::info!(provider = %name, "Registered provider from config");
                    }
                }
                if !daemon {
                    println!(
                        "  Providers: {} registered (default: {})",
                        registry.list_providers().len(),
                        def_name
                    );
                }
                Some(registry as Arc<alephcore::MultiProviderRegistry>)
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    }
}
