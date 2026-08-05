//! Provider RPC handler functions.

use crate::sync_primitives::Arc;
use serde_json::json;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use super::helpers::{
    build_provider_config_for_persistence, normalize_optional_string, resolve_api_key, save_config,
    vault_key,
};
use super::parse_params;
use super::types::{
    CatalogEntryView, CatalogParams, CreateParams, DeleteParams, GetParams, ProviderHealthRow,
    ProviderInfo, SetDefaultParams, TestParams, TestResult, UpdateParams,
};
use crate::config::{Config, ProviderConfig};
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::security::SharedTokenManager;
use crate::providers::probe::probe_provider_bounded;

// ============================================================================
// List
// ============================================================================

/// List all providers
pub async fn handle_list(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    let config = config.read().await;
    let default_provider = config.general.default_provider.clone();

    let providers: Vec<ProviderInfo> = config
        .providers
        .iter()
        .map(|(name, cfg)| {
            let has_api_key = resolve_api_key(name, &vault).is_some();
            ProviderInfo {
                name: name.clone(),
                enabled: cfg.enabled,
                model: cfg.models.first().cloned().unwrap_or_default(),
                models: cfg.models.clone(),
                provider_type: Some(cfg.protocol()),
                base_url: cfg.base_url.clone(),
                color: cfg.color.clone(),
                timeout_seconds: cfg.timeout_seconds,
                max_tokens: cfg.max_tokens,
                context_window: cfg.context_window,
                temperature: cfg.temperature,
                has_api_key,
                api_key: None,
                is_default: default_provider.as_ref() == Some(name),
                verified: cfg.verified,
            }
        })
        .collect();

    JsonRpcResponse::success(request.id, json!({ "providers": providers }))
}

// ============================================================================
// Get
// ============================================================================

/// Get a single provider
pub async fn handle_get(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    let params: GetParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let config = config.read().await;
    match config.providers.get(&params.name) {
        Some(cfg) => {
            let default_provider = config.general.default_provider.clone();
            let info = ProviderInfo {
                name: params.name.clone(),
                enabled: cfg.enabled,
                model: cfg.models.first().cloned().unwrap_or_default(),
                models: cfg.models.clone(),
                provider_type: Some(cfg.protocol()),
                base_url: cfg.base_url.clone(),
                color: cfg.color.clone(),
                timeout_seconds: cfg.timeout_seconds,
                max_tokens: cfg.max_tokens,
                context_window: cfg.context_window,
                temperature: cfg.temperature,
                has_api_key: resolve_api_key(&params.name, &vault).is_some(),
                api_key: None,
                is_default: default_provider.as_ref() == Some(&params.name),
                verified: cfg.verified,
            };
            JsonRpcResponse::success(request.id, json!({ "provider": info }))
        }
        None => JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Provider not found: {}", params.name),
        ),
    }
}

// ============================================================================
// Update
// ============================================================================

/// Update a provider (config-only, no runtime swap)
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    update_provider_inner(request, config, event_bus, vault, None).await
}

/// Update a provider and hot-reload the live registry instance.
pub async fn handle_update_hot(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
    vault: Arc<SharedTokenManager>,
    multi_registry: Arc<crate::thinker::MultiProviderRegistry>,
) -> JsonRpcResponse {
    update_provider_inner(request, config, event_bus, vault, Some(multi_registry)).await
}

async fn update_provider_inner(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
    vault: Arc<SharedTokenManager>,
    multi_registry: Option<Arc<crate::thinker::MultiProviderRegistry>>,
) -> JsonRpcResponse {
    let params: UpdateParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Snapshot (name + config WITH api_key) for live reload, captured before
    // the key is stripped for persistence. None when no registry.
    let mut registration: Option<(String, ProviderConfig)> = None;

    // Update config
    {
        let mut cfg = config.write().await;

        // Check if provider exists
        if !cfg.providers.contains_key(&params.name) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Provider not found: {}", params.name),
            );
        }

        // Convert JSON config to ProviderConfig. The stored entry is the
        // baseline so fields the DTO cannot carry (cache_retention,
        // stop_sequences, thinking_level, service_tier, effort, …) survive an
        // edit to an unrelated field.
        let prior = cfg.providers.get(&params.name).cloned();
        let mut provider_config = build_provider_config_for_persistence(
            &params.name,
            params.config,
            &cfg.presets_override,
            prior.as_ref(),
        );

        // Store new API key in vault if provided; otherwise vault retains the old one
        if let Some(ref api_key) = provider_config.api_key {
            if let Err(e) = vault.store_secret(&vault_key(&params.name), api_key) {
                error!(error = %e, "Failed to store API key in vault");
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to store API key: {e}"),
                );
            }
        }

        // Capture the registration snapshot (with the key) before clearing it,
        // so the live registry can build a working provider instance.
        if multi_registry.is_some() {
            registration = Some((params.name.clone(), provider_config.clone()));
        }
        provider_config.api_key = None;

        // Update provider — config change resets verified status
        provider_config.verified = false;
        cfg.providers.insert(params.name.clone(), provider_config);

        // Save to file
        if let Err(e) = save_config(&cfg, &["providers"]) {
            error!(error = %e, "Failed to save config");
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }
    }

    // Hot-reload into the live registry so protocol/base_url/model changes take
    // effect on the very next prompt, matching create/delete/setDefault behavior.
    if let (Some(registry), Some((name, pc))) = (&multi_registry, registration) {
        match crate::providers::create_provider(&name, pc) {
            Ok(provider) => {
                registry.register(name.clone(), provider);
                info!(name = %name, "Provider hot-reloaded in live registry");
            }
            Err(e) => error!(
                name = %name, error = %e,
                "Failed to build provider for hot reload (config saved)"
            ),
        }
    }

    // Broadcast event
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("providers".to_string()),
        value: json!({ "action": "updated", "provider": params.name }),
        timestamp,
    });

    if let Err(e) = event_bus.publish_json(&event) {
        error!(error = %e, "Failed to broadcast event");
    }

    info!(name = %params.name, "Provider updated");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

// ============================================================================
// Create
// ============================================================================

/// Create a new provider
pub async fn handle_create(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    create_provider_inner(request, config, event_bus, vault, None).await
}

/// Like [`handle_create`] but also registers the new provider into the live
/// `MultiProviderRegistry`, so it is immediately usable — and an auto-derived
/// failover fallback — without a server restart.
pub async fn handle_create_hot(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
    vault: Arc<SharedTokenManager>,
    multi_registry: Arc<crate::thinker::MultiProviderRegistry>,
) -> JsonRpcResponse {
    create_provider_inner(request, config, event_bus, vault, Some(multi_registry)).await
}

async fn create_provider_inner(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
    vault: Arc<SharedTokenManager>,
    multi_registry: Option<Arc<crate::thinker::MultiProviderRegistry>>,
) -> JsonRpcResponse {
    let params: CreateParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Snapshot (name + config WITH api_key) for live registration, captured
    // before the key is stripped for persistence. None when no registry.
    let mut registration: Option<(String, ProviderConfig)> = None;

    // Create provider
    {
        let mut cfg = config.write().await;

        // Check if provider already exists
        if cfg.providers.contains_key(&params.name) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Provider already exists: {}", params.name),
            );
        }

        // Convert JSON config to ProviderConfig. `create` has no prior entry
        // (the guard above rejects an existing name), so there is nothing to
        // carry forward.
        let mut provider_config = build_provider_config_for_persistence(
            &params.name,
            params.config,
            &cfg.presets_override,
            None,
        );

        // Store API key in vault (then clear from config so it's never persisted)
        if let Some(ref api_key) = provider_config.api_key {
            if let Err(e) = vault.store_secret(&vault_key(&params.name), api_key) {
                error!(error = %e, "Failed to store API key in vault");
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to store API key: {e}"),
                );
            }
        }
        // Capture the registration snapshot (with the key) before clearing it,
        // so the live registry can build a working provider instance.
        if multi_registry.is_some() {
            registration = Some((params.name.clone(), provider_config.clone()));
        }
        provider_config.api_key = None;

        // Insert provider
        cfg.providers.insert(params.name.clone(), provider_config);

        // Save to file
        if let Err(e) = save_config(&cfg, &["providers"]) {
            error!(error = %e, "Failed to save config");
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }
    }

    // Hot-register into the live registry so the provider is usable and an
    // auto-derived failover fallback without a restart (mirrors set_default's
    // runtime swap). A build failure leaves the config saved — the provider
    // becomes live on the next restart.
    if let (Some(registry), Some((name, pc))) = (&multi_registry, registration) {
        match crate::providers::create_provider(&name, pc) {
            Ok(provider) => {
                registry.register(name.clone(), provider);
                info!(name = %name, "Provider registered in live registry");
            }
            Err(e) => error!(
                name = %name, error = %e,
                "Failed to build provider for live registration (config saved)"
            ),
        }
    }

    // Broadcast event
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("providers".to_string()),
        value: json!({ "action": "created", "provider": params.name }),
        timestamp,
    });

    if let Err(e) = event_bus.publish_json(&event) {
        error!(error = %e, "Failed to broadcast event");
    }

    info!(name = %params.name, "Provider created");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

// ============================================================================
// Delete
// ============================================================================

/// Delete a provider
pub async fn handle_delete(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    delete_provider_inner(request, config, event_bus, vault, None).await
}

/// Like [`handle_delete`] but also removes the provider from the live
/// `MultiProviderRegistry`, so it stops being an auto-derived failover fallback
/// without a server restart.
pub async fn handle_delete_hot(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
    vault: Arc<SharedTokenManager>,
    multi_registry: Arc<crate::thinker::MultiProviderRegistry>,
) -> JsonRpcResponse {
    delete_provider_inner(request, config, event_bus, vault, Some(multi_registry)).await
}

async fn delete_provider_inner(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
    vault: Arc<SharedTokenManager>,
    multi_registry: Option<Arc<crate::thinker::MultiProviderRegistry>>,
) -> JsonRpcResponse {
    let params: DeleteParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Delete provider
    {
        let mut cfg = config.write().await;

        // Check if provider exists
        if !cfg.providers.contains_key(&params.name) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Provider not found: {}", params.name),
            );
        }

        // Check if it's the default provider
        if cfg.general.default_provider.as_ref() == Some(&params.name) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Cannot delete default provider: {}", params.name),
            );
        }

        // Remove provider
        cfg.providers.remove(&params.name);

        // Scrub the deleted provider from the failover chain so deletion does
        // not leave a dangling reference behind. The `[fallback_provider]`
        // section is only persisted when the chain actually changed.
        let mut sections: Vec<&str> = vec!["providers"];
        if let Some(fb) = cfg.fallback_provider.as_mut() {
            if crate::gateway::handlers::remove_from_chain(&mut fb.chain, &params.name) {
                sections.push("fallback_provider");
            }
        }

        // Delete API key from vault
        if let Err(e) = vault.delete_secret(&vault_key(&params.name)) {
            tracing::warn!(provider = %params.name, error = %e, "Failed to delete API key from vault");
        }

        // Save to file
        if let Err(e) = save_config(&cfg, &sections) {
            error!(error = %e, "Failed to save config");
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }
    }

    // Drop the provider from the live registry so it stops being dialed (as an
    // auto-derived failover fallback) without a restart. The registry refuses
    // to remove its last provider — harmless here since the default provider
    // (guarded above) can never be the one being deleted.
    if let Some(registry) = &multi_registry {
        if let Err(e) = registry.remove(&params.name) {
            tracing::warn!(provider = %params.name, error = %e, "Failed to remove provider from live registry");
        }
    }

    // Broadcast event
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("providers".to_string()),
        value: json!({ "action": "deleted", "provider": params.name }),
        timestamp,
    });

    if let Err(e) = event_bus.publish_json(&event) {
        error!(error = %e, "Failed to broadcast event");
    }

    info!(name = %params.name, "Provider deleted");
    JsonRpcResponse::success(request.id, json!({ "ok": true }))
}

// ============================================================================
// Test
// ============================================================================

/// Test a provider connection, resetting its circuit breaker on success.
///
/// This used to come in two variants — one taking the `MultiProviderRegistry`
/// and one without — because the reset went through that registry's own health
/// map. The breaker that actually gates dialing is reached through the process
/// route-observability handle instead, so the registry is not needed here and
/// the split collapsed with it.
pub async fn handle_test(
    request: JsonRpcRequest,
    config_store: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    let params: TestParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let provider_name = params.name;
    let config = params.config;

    // If no api_key in request, fall back to vault
    let effective_api_key = {
        let inline_key = normalize_optional_string(config.api_key.clone());
        if inline_key.is_none() {
            if let Some(ref name) = provider_name {
                resolve_api_key(name, &vault)
            } else {
                None
            }
        } else {
            inline_key
        }
    };

    // Convert JSON config to runtime ProviderConfig
    let provider_config = ProviderConfig {
        protocol: config.protocol,
        api_key: effective_api_key,
        models: config.models.clone(),
        base_url: config.base_url,
        color: config.color.unwrap_or_else(|| "#808080".to_string()),
        timeout_seconds: config.timeout_seconds.unwrap_or(300),
        enabled: config.enabled,
        max_tokens: config.max_tokens,
        context_window: None,
        temperature: config.temperature,
        top_p: config.top_p,
        top_k: config.top_k,
        frequency_penalty: None,
        presence_penalty: None,
        stop_sequences: None,
        thinking_level: None,
        media_resolution: None,
        repeat_penalty: None,
        system_prompt_mode: None,
        model_behavior: None,
        verified: false,
        service_tier: None,
        stream_idle_timeout_secs: None,
        cache_retention: None,
        response_format: None,
        parallel_tool_calls: None,
        seed: None,
        logprobs: None,
        top_logprobs: None,
        metadata_user_id: None,
        effort: None,
    };

    // Probe connectivity (shared with the bulk `providers.healthcheck` probe),
    // bounded by the shared per-provider deadline so a hung endpoint can't
    // stall the RPC.
    let result = TestResult::from(probe_provider_bounded("test", provider_config).await);

    // On success, persist verified=true and reset health. Only `providers.test`
    // mutates state; the bulk healthcheck is read-only.
    if result.success {
        if let Some(ref name) = provider_name {
            let mut cfg = config_store.write().await;
            if let Some(p) = cfg.providers.get_mut(name) {
                p.verified = true;
                if let Err(e) = save_config(&cfg, &["providers"]) {
                    error!(error = %e, "Failed to save config after test");
                }
            }
            // Reset the circuit breaker that decides whether a request is even
            // dialed, so a rotated credential is tried again immediately instead
            // of staying shut out for the full 10-minute permanent-failure
            // cooldown despite "Test connection" going green. (There used to be
            // a second reset here, against a registry health map that fed only
            // the `ModelResolved` badge and gated no dial; that map is gone.)
            if let Some(obs) = crate::providers::route_observe::global_route_observability() {
                if obs.health.reset(name).await {
                    info!(provider = %name, "connection test passed; failover circuit reset");
                }
            }
        }
    }

    JsonRpcResponse::success(request.id, json!(result))
}

// Probe logic lives in `crate::providers::probe::probe_provider_bounded` — the
// single source of truth (probe + shared timeout) shared with the diagnostics
// engine's connectivity check. This module maps its `ProbeOutcome` onto the
// wire-stable `TestResult`.

/// `providers.healthcheck` — concurrently probe every configured provider.
///
/// Read-only liveness sweep: snapshots each provider's config (injecting its
/// vault key), then pings them all concurrently via [`futures::future::join_all`]
/// — superior to the reference's serial per-provider checks. Disabled providers
/// are reported as `skipped` without a network call. Returns
/// `{ "providers": [ProviderHealthRow, ...] }` sorted by name.
pub async fn handle_healthcheck(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    // Snapshot configs + resolve keys under the read lock, then release it
    // before any network I/O so probes never hold the config lock.
    let probes: Vec<(String, bool, ProviderConfig)> = {
        let cfg = config.read().await;
        cfg.providers
            .iter()
            .map(|(name, pc)| {
                let mut runtime = pc.clone();
                runtime.api_key = resolve_api_key(name, &vault);
                (name.clone(), pc.enabled, runtime)
            })
            .collect()
    };

    let probe_futures = probes
        .into_iter()
        .map(|(name, enabled, runtime)| async move {
            if !enabled {
                return ProviderHealthRow {
                    name,
                    enabled,
                    ok: false,
                    skipped: true,
                    latency_ms: None,
                    error: None,
                };
            }
            let result = probe_provider_bounded(&name, runtime).await;
            ProviderHealthRow {
                name,
                enabled,
                ok: result.success,
                skipped: false,
                latency_ms: result.latency_ms,
                error: result
                    .error
                    .map(|error| crate::diagnostics::redact_secrets(&error)),
            }
        });

    let mut rows = futures::future::join_all(probe_futures).await;
    rows.sort_by(|a, b| a.name.cmp(&b.name));

    JsonRpcResponse::success(request.id, json!({ "providers": rows }))
}

// ============================================================================
// Live model discovery
// ============================================================================

/// `providers.modelsRefresh` — ask configured providers what they serve now.
///
/// The operator-facing half of `model_catalog::discovery`; the LLM-facing half
/// is `list_models { refresh: true }`. Both call the same leaf, so the picker
/// and the model can never disagree about what a provider offers.
///
/// Optional `provider` param narrows the sweep to one id (what the picker's
/// per-row refresh button wants); omitted, it sweeps every enabled provider
/// concurrently. Per-provider failures are reported as rows, not as an RPC
/// error — one unreachable vendor must not blank the whole result.
pub async fn handle_models_refresh(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    let only: Option<String> = request
        .params
        .as_ref()
        .and_then(|p| p.get("provider"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);

    // Same discipline as `handle_healthcheck`: snapshot under the read lock,
    // release before any network I/O.
    let targets: Vec<(String, String, String, String)> = {
        let cfg = config.read().await;
        cfg.providers
            .iter()
            .filter(|(name, pc)| {
                pc.enabled && only.as_ref().is_none_or(|o| o.eq_ignore_ascii_case(name))
            })
            .filter_map(|(name, pc)| {
                let preset = crate::providers::presets::get_preset(name);
                let base_url = pc
                    .base_url
                    .clone()
                    .or_else(|| preset.map(|p| p.base_url.to_string()))?;
                let protocol = pc
                    .protocol
                    .clone()
                    .or_else(|| preset.map(|p| p.protocol.to_string()))
                    .unwrap_or_else(|| "openai".to_string());
                let api_key = pc.api_key.clone().or_else(|| resolve_api_key(name, &vault));
                if api_key.is_none() {
                    warn!(
                        provider = %name,
                        "Skipping modelsRefresh: no API key resolvable"
                    );
                    return None;
                }
                Some((name.clone(), base_url, protocol, api_key.unwrap()))
            })
            .collect()
    };

    let sweeps = targets
        .into_iter()
        .map(|(name, base_url, protocol, api_key)| async move {
            let outcome = crate::providers::model_catalog::refresh_models(
                &name, &base_url, &protocol, &api_key,
            )
            .await;
            match outcome {
                Ok(listing) => json!({
                    "provider": name,
                    "ok": true,
                    "fetched_at": listing.fetched_at,
                    "models": listing.models,
                }),
                Err(e) => {
                    // Stale snapshot beats an error row: report what the
                    // provider served last time, marked so the picker can
                    // show it as dated rather than live.
                    match crate::providers::model_catalog::cached_models(&name, &base_url) {
                        Some(stale) => json!({
                            "provider": name,
                            "ok": true,
                            "stale": true,
                            "error": e.to_string(),
                            "fetched_at": stale.fetched_at,
                            "models": stale.models,
                        }),
                        None => json!({
                            "provider": name,
                            "ok": false,
                            "error": e.to_string(),
                        }),
                    }
                }
            }
        });

    let mut rows = futures::future::join_all(sweeps).await;
    rows.sort_by(|a, b| {
        a.get("provider")
            .and_then(serde_json::Value::as_str)
            .cmp(&b.get("provider").and_then(serde_json::Value::as_str))
    });

    JsonRpcResponse::success(request.id, json!({ "providers": rows }))
}

// ============================================================================
// Needs Setup
// ============================================================================

/// Check if first-run setup is needed
///
/// Returns true if no provider is both enabled and verified.
/// Panel calls this on startup to decide whether to show the setup wizard.
pub async fn handle_needs_setup(
    request: JsonRpcRequest,
    config_store: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    let cfg = config_store.read().await;
    let provider_count = cfg.providers.len();
    let has_verified = cfg.providers.values().any(|p| p.enabled && p.verified);

    JsonRpcResponse::success(
        request.id,
        json!({
            "needs_setup": !has_verified,
            "provider_count": provider_count,
            "has_verified": has_verified,
        }),
    )
}

// ============================================================================
// Set Default
// ============================================================================

/// Set the default provider (config-only, no runtime swap)
pub async fn handle_set_default_config_only(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: SetDefaultParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    set_default_provider_inner(&request, &params, &config, &event_bus, None, None).await
}

/// Set the default provider with runtime hot-swap
pub async fn handle_set_default(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
    multi_registry: Arc<crate::thinker::MultiProviderRegistry>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    let params: SetDefaultParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    set_default_provider_inner(
        &request,
        &params,
        &config,
        &event_bus,
        Some(&multi_registry),
        Some(&vault),
    )
    .await
}

/// Shared implementation for setting the default provider
async fn set_default_provider_inner(
    request: &JsonRpcRequest,
    params: &SetDefaultParams,
    config: &Arc<RwLock<Config>>,
    event_bus: &Arc<GatewayEventBus>,
    multi_registry: Option<&Arc<crate::thinker::MultiProviderRegistry>>,
    vault: Option<&Arc<SharedTokenManager>>,
) -> JsonRpcResponse {
    // Resolve canonical name through the preset alias table (e.g. "codex" →
    // "chatgpt"). An exact config key match wins first (applied below against
    // the live config): an operator who literally named a provider "codex"
    // keeps their own entry. Anything the alias table doesn't know is used
    // as-is (custom providers).
    let name = crate::providers::presets::canonical_preset_id(&params.name.to_lowercase())
        .map(str::to_string)
        .unwrap_or_else(|| params.name.clone());

    // Set default provider and build new provider instance
    let provider_config_for_swap: Option<(String, crate::config::ProviderConfig)>;
    {
        let mut cfg = config.write().await;

        // An exact (case-insensitive) config key beats the alias resolution:
        // `canonical_preset_id("kimi")` is `moonshot`, but if the operator
        // configured a provider literally named "kimi", that is the one they
        // mean.
        let name = cfg
            .providers
            .keys()
            .find(|k| k.eq_ignore_ascii_case(&params.name))
            .cloned()
            .unwrap_or(name);

        // Guard: provider must exist and be verified
        match cfg.providers.get(&name) {
            None => {
                return JsonRpcResponse::error(
                    request.id.clone(),
                    INVALID_PARAMS,
                    format!("Provider not found: {name}"),
                );
            }
            Some(provider) if !provider.verified => {
                return JsonRpcResponse::error(
                    request.id.clone(),
                    INVALID_PARAMS,
                    format!(
                        "Provider '{name}' must pass a connection test before being set as default"
                    ),
                );
            }
            _ => {}
        }

        // Capture provider config before setting default (for runtime swap).
        // api_key is #[serde(skip)] so it's None on the in-memory config — inject from vault.
        provider_config_for_swap = if multi_registry.is_some() {
            cfg.providers.get(&name).map(|pc| {
                let mut pc = pc.clone();
                if pc.api_key.as_ref().is_none_or(|k| k.is_empty()) {
                    if let Some(v) = vault {
                        pc.api_key = resolve_api_key(&name, v);
                    }
                }
                (name.clone(), pc)
            })
        } else {
            None
        };

        // Use the existing set_default_provider method
        if let Err(e) = cfg.set_default_provider(&name) {
            return JsonRpcResponse::error(
                request.id.clone(),
                INVALID_PARAMS,
                format!("Failed to set default provider: {e}"),
            );
        }

        // Save to file (redact resolved secrets before write)
        if let Err(e) = save_config(&cfg, &["general"]) {
            error!(error = %e, "Failed to save config");
            return JsonRpcResponse::error(
                request.id.clone(),
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }
    }

    // Hot-swap the runtime provider using MultiProviderRegistry
    if let (Some(registry), Some((name, provider_config))) =
        (multi_registry, provider_config_for_swap)
    {
        match crate::providers::create_provider(&name, provider_config) {
            Ok(new_provider) => {
                registry.register(name.clone(), new_provider);
                if let Err(e) = registry.set_default(&name) {
                    error!(name = %name, error = %e, "Failed to set default in multi-registry");
                } else {
                    // Clear the breaker that gates dialing, so a provider
                    // promoted to default is actually tried. This used to reset
                    // the registry's own health map instead — which fed only the
                    // `ModelResolved` badge and gated nothing — so promoting a
                    // provider whose circuit was open left it skipped for the
                    // rest of the cooldown. Same fix, and same reason, as the
                    // connection-test path above.
                    if let Some(obs) = crate::providers::route_observe::global_route_observability()
                    {
                        if obs.health.reset(&name).await {
                            info!(name = %name, "set as default; failover circuit reset");
                        }
                    }
                    info!(name = %name, providers = ?registry.list_providers(), "Provider set as default in multi-registry");
                }
            }
            Err(e) => {
                error!(name = %name, error = %e, "Failed to create provider for hot-swap (config saved)");
            }
        }
    }

    // Broadcast event
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("providers".to_string()),
        value: json!({ "action": "set_default", "provider": params.name }),
        timestamp,
    });

    if let Err(e) = event_bus.publish_json(&event) {
        error!(error = %e, "Failed to broadcast event");
    }

    info!(name = %params.name, "Default provider set");
    JsonRpcResponse::success(request.id.clone(), json!({ "ok": true }))
}

// ============================================================================
// Catalog (chat-side picker)
// ============================================================================

/// `providers.catalog` — return the chat-side provider/model catalog joined
/// with per-user credential state. Drives the chat-window model picker.
///
/// Equivalent in shape to openclaw's `models.list { view }`. Aleph's Rust
/// implementation differs in two ways:
/// 1. Catalogue data is read directly from compile-time `Lazy<HashMap>`
///    constants — no async I/O on the hot path, no TTL cache, no
///    generation counter required.
/// 2. Credential filtering uses the already-populated
///    `config.providers[name].{verified, api_key}` fields, eliminating
///    openclaw's parallel `auth checker` round-trip.
pub async fn handle_catalog(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    use crate::providers::catalog;
    use crate::providers::metadata::Modality;
    use crate::providers::presets as chat_presets;

    let params: CatalogParams = match request.params {
        Some(ref v) if !v.is_null() => match serde_json::from_value(v.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("invalid params: {e}"),
                )
            }
        },
        _ => CatalogParams::default(),
    };

    let config_guard = config.read().await;
    let default_provider = config_guard.general.default_provider.clone();

    let entries = catalog::presets_for_modality(Modality::Chat);
    let view = params.view.as_deref().unwrap_or("configured");

    let mut items: Vec<CatalogEntryView> = entries
        .into_iter()
        .filter_map(|entry| {
            // Built-in template (always present for chat entries returned by
            // catalog::presets_for_modality). Phase 2 folded
            // display_name/modalities/homepage/notes into the preset itself,
            // so this is the single source of truth — no metadata-map fallback.
            let preset = chat_presets::get_preset(entry.name)?;
            // A provider configured under an *alias* (`kimi` for `moonshot`)
            // attaches to the canonical row rather than falling through to
            // the custom section — aliases are resolution keys, and the
            // enumeration above is canonical-only since this round. The
            // matched config key doubles as the vault key (`ai:<name>`), so
            // it is tracked alongside the config.
            let (cfg_name, cfg) =
                match config_guard
                    .providers
                    .get_key_value(entry.name)
                    .or_else(|| {
                        preset
                            .aliases
                            .iter()
                            .find_map(|a| config_guard.providers.get_key_value(*a))
                    }) {
                    Some((n, c)) => (n.as_str(), Some(c)),
                    None => (entry.name, None),
                };
            let api_key = resolve_api_key(cfg_name, &vault);
            let has_api_key = api_key.is_some() || cfg.and_then(|c| c.api_key.as_ref()).is_some();
            let verified = cfg.is_some_and(|c| c.verified);
            let enabled = cfg.is_some_and(|c| c.enabled);
            let models = cfg.map(|c| c.models.clone()).unwrap_or_default();

            let display_name = preset
                .display_name
                .map_or_else(|| entry.name.to_string(), String::from);

            // Per-model metadata for the default model, via the one join
            // point (`ModelRecord::resolve`) rather than a hand-written
            // capability + cost + endpoint lookup. Best-effort: unknown /
            // unpriced families leave the JSON fields absent
            // (skip_serializing_if). This is the R7 "enable the LLM" surface —
            // reference data the picker and the model can reason over, not an
            // auto-router.
            let record = crate::providers::model_catalog::ModelRecord::resolve(
                entry.name,
                preset.default_model,
                Some(preset.base_url),
                crate::providers::model_catalog::ModelSource::PresetDefault,
            );

            // The picker roster merges through the same leaf the failover
            // walk uses (`presets::model_ladder`), seeded with the operator's
            // `models` or, when none are listed, the preset default. This is
            // what stops the picker from recommending curated ids on a relay
            // whose base_url the operator moved — they would 400 there.
            let roster = chat_presets::model_ladder(
                entry.name,
                if models.is_empty() {
                    let d = preset.default_model;
                    if d.is_empty() {
                        Vec::new()
                    } else {
                        vec![d.to_string()]
                    }
                } else {
                    models.clone()
                },
                cfg.and_then(|c| c.base_url.as_deref()),
            );

            Some(CatalogEntryView {
                id: entry.name.to_string(),
                display_name,
                default_model: preset.default_model.to_string(),
                base_url: preset.base_url.to_string(),
                protocol: preset.protocol.to_string(),
                color: preset.color.to_string(),
                homepage: preset.homepage.map(String::from),
                notes: preset.description.map(String::from),
                signup_url: preset.signup_url.map(String::from),
                fallback_models: preset
                    .fallback_models
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                default_aux_model: preset.default_aux_model.map(String::from),
                aliases: preset.aliases.iter().map(|s| s.to_string()).collect(),
                modalities: preset
                    .modalities
                    .iter()
                    .map(|m| m.as_str().to_string())
                    .collect(),
                models,
                has_api_key,
                verified,
                enabled,
                is_default: default_provider.as_deref() == Some(entry.name),
                capabilities: record.capabilities,
                cost: record.cost,
                endpoint: record.endpoint.as_str().to_string(),
                lifecycle: record.lifecycle,
                requires_explicit_model: preset.requires_explicit_model,
                roster,
            })
        })
        .collect();

    // Custom providers: user-defined entries in `config.providers` with no
    // matching built-in chat preset (e.g. an OpenAI-compatible relay added via
    // `providers.create`). The catalog above is preset-driven, so without this
    // a fully configured custom provider stays invisible to the model picker.
    let preset_ids: std::collections::HashSet<&str> = items.iter().map(|i| i.id.as_str()).collect();
    let mut custom: Vec<CatalogEntryView> = config_guard
        .providers
        .iter()
        // Custom = no built-in preset answers to this name, canonical *or*
        // alias (an alias-keyed config attached to its canonical row above).
        .filter(|(name, _)| {
            !preset_ids.contains(name.as_str()) && chat_presets::get_preset(name).is_none()
        })
        .map(|(name, cfg)| {
            let api_key = resolve_api_key(name, &vault);
            let has_api_key = api_key.is_some() || cfg.api_key.is_some();
            let default_model = cfg.models.first().cloned().unwrap_or_default();
            let record = crate::providers::model_catalog::ModelRecord::resolve(
                name,
                &default_model,
                cfg.base_url.as_deref(),
                crate::providers::model_catalog::ModelSource::Configured,
            );
            CatalogEntryView {
                id: name.clone(),
                display_name: name.clone(),
                default_model,
                base_url: cfg.base_url.clone().unwrap_or_default(),
                protocol: cfg.protocol.clone().unwrap_or_else(|| "openai".to_string()),
                color: cfg.color.clone(),
                homepage: None,
                notes: None,
                signup_url: None,
                fallback_models: Vec::new(),
                default_aux_model: None,
                aliases: Vec::new(),
                modalities: vec![Modality::Chat.as_str().to_string()],
                models: cfg.models.clone(),
                has_api_key,
                verified: cfg.verified,
                enabled: cfg.enabled,
                is_default: default_provider.as_deref() == Some(name.as_str()),
                capabilities: record.capabilities,
                cost: record.cost,
                endpoint: record.endpoint.as_str().to_string(),
                lifecycle: record.lifecycle,
                // Custom providers are operator-defined: whatever they listed
                // in `models` is the roster, so there is never a "no default
                // shipped" state to announce.
                requires_explicit_model: false,
                roster: cfg.models.clone(),
            }
        })
        .collect();
    items.append(&mut custom);

    // Apply the requested credential view filter to the merged catalog.
    items.retain(|item| match view {
        "configured" => item.verified && item.enabled,
        "available" => item.has_api_key,
        _ => true, // "all" or unrecognised — return everything chat-capable
    });

    // Round-2 E3: MoA presets ride the picker as a pseudo-provider row.
    // Selecting one sends model_override {provider:"moa", model:<preset>},
    // which chat.send's `apply_moa_selector_semantics` intercept
    // (`src/gateway/handlers/agent.rs`) converts into a session MoA
    // activation — wired into both the Simulated-fallback path
    // (`AgentRunManager::start_run`) and the real-`ExecutionEngine` path
    // (`handle_chat_send_with_engine`, `src/bin/aleph-server/server_init.rs`,
    // since the Task 18 fix that closed the gap where a real deployment
    // silently ignored this pick). Appended AFTER the view retain: this
    // synthetic row is always shown when presets exist (it has no
    // credential of its own).
    if let Some(moa_cfg) = crate::providers::moa::get_moa_config() {
        let mut names: Vec<String> = moa_cfg
            .presets
            .iter()
            .filter(|(_, p)| p.enabled)
            .map(|(n, _)| n.clone())
            .collect();
        names.sort();
        if !names.is_empty() {
            let default_model = moa_cfg
                .default_preset
                .clone()
                .filter(|d| names.contains(d))
                .unwrap_or_else(|| names[0].clone());
            items.push(CatalogEntryView {
                id: "moa".to_string(),
                display_name: "Mixture of Agents".to_string(),
                default_model,
                base_url: "moa://virtual".to_string(),
                protocol: "moa".to_string(),
                color: "#8b5cf6".to_string(),
                homepage: None,
                notes: Some(
                    "MoA advisory presets — selecting one activates advisor fan-out \
                     for this session"
                        .to_string(),
                ),
                signup_url: None,
                fallback_models: Vec::new(),
                default_aux_model: None,
                aliases: Vec::new(),
                modalities: vec!["chat".to_string()],
                models: names.clone(),
                has_api_key: true,
                verified: true,
                enabled: true,
                is_default: false,
                // No single capability/cost profile applies across a preset's
                // mixed advisors + aggregator; leave both absent rather than
                // guess from one slot.
                capabilities: None,
                cost: None,
                // Virtual multiplexer, not a reachable host — conservatively
                // "cloud" like any other absent/unparseable base_url.
                endpoint: "cloud".to_string(),
                // A preset name is not a vendor model id, so no vendor
                // lifecycle applies. (A preset whose *slots* name retired
                // models is caught where those slots are resolved, not here.)
                lifecycle: crate::providers::model_catalog::ModelLifecycle::ACTIVE,
                requires_explicit_model: false,
                roster: names,
            });
        }
    }

    JsonRpcResponse::success(request.id, json!({ "items": items }))
}
