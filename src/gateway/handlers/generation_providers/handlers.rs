use crate::config::types::generation::presets::{generation_metadata, PRESETS};
use crate::config::Config;
use crate::gateway::event_bus::GatewayEventBus;
use crate::gateway::handlers::generation_providers::helpers::{
    default_modalities, find_provider_type, get_typed_provider_map, get_typed_provider_map_mut,
    parse_generation_type, provider_exists,
};
use crate::gateway::handlers::generation_providers::wire::{
    config_from_wire, modality_str, provider_row,
};
use crate::gateway::handlers::generation_providers::{
    build_generation_provider_for_persistence, resolve_api_key, save_config, vault_key,
    TestConnectionResult,
};
use crate::gateway::handlers::parse_params;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::security::SharedTokenManager;
use crate::generation::GenerationType;
use crate::sync_primitives::Arc;
use aleph_protocol::providers::{
    GenerationPresetRow, GenerationProviderConfigJson, GenerationProviderRow,
};
use serde::Deserialize;
use tokio::sync::RwLock;
use tracing::{error, warn};

/// List all generation providers
pub async fn handle_list(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    let cfg = config.read().await;

    let mut rows: Vec<GenerationProviderRow> = cfg
        .generation
        .merged_providers()
        .into_iter()
        .map(|(name, provider_config, gen_type)| {
            let is_default_for = default_modalities(&cfg.generation, &name);
            let has_api_key = resolve_api_key(&name, &vault).is_some();
            provider_row(
                name,
                &provider_config,
                &is_default_for,
                gen_type,
                has_api_key,
            )
        })
        .collect();

    // Sort by name for consistent ordering
    rows.sort_by(|a, b| a.name.cmp(&b.name));

    match serde_json::to_value(&rows) {
        Ok(v) => JsonRpcResponse::success(request.id, v),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize providers: {e}"),
        ),
    }
}

/// Get a specific generation provider
pub async fn handle_get(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        name: String,
    }

    let params: Params = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let cfg = config.read().await;

    // Find provider across all typed maps and legacy
    let found = cfg
        .generation
        .merged_providers()
        .into_iter()
        .find(|(name, _, _)| name == &params.name);

    match found {
        Some((name, provider_config, gen_type)) => {
            let is_default_for = default_modalities(&cfg.generation, &name);
            let has_api_key = resolve_api_key(&name, &vault).is_some();
            let row = provider_row(
                name,
                &provider_config,
                &is_default_for,
                gen_type,
                has_api_key,
            );
            match serde_json::to_value(&row) {
                Ok(v) => JsonRpcResponse::success(request.id, v),
                Err(e) => JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to serialize provider: {e}"),
                ),
            }
        }
        None => JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("Provider '{}' not found", params.name),
        ),
    }
}

/// Create a new generation provider
pub async fn handle_create(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        name: String,
        config: GenerationProviderConfigJson,
        #[serde(default)]
        generation_type: Option<String>,
    }

    let params: Params = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let incoming = config_from_wire(params.config);

    // Determine generation type from explicit param or capabilities
    let gen_type_str = params
        .generation_type
        .as_deref()
        .or_else(|| incoming.capabilities.first().copied().map(modality_str))
        .unwrap_or("image");
    let gen_type = parse_generation_type(gen_type_str);

    {
        let mut cfg = config.write().await;

        // Check if provider already exists in any map
        if provider_exists(&cfg.generation, &params.name) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Provider '{}' already exists", params.name),
            );
        }

        let mut provider_config = build_generation_provider_for_persistence(
            &params.name,
            incoming,
            &cfg.presets_override.generation,
            // Create: nothing stored yet, so there is nothing to preserve.
            None,
        );

        // Set capabilities from generation type
        provider_config.capabilities = vec![gen_type];

        // Validate provider config
        if let Err(e) = provider_config.validate(&params.name) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Validation failed: {e}"),
            );
        }

        // Store API key in vault
        if let Some(ref api_key) = provider_config.api_key {
            if let Err(e) = vault.store_secret(&vault_key(&params.name), api_key) {
                error!(error = %e, "Failed to store generation API key in vault");
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to store API key: {e}"),
                );
            }
        }
        provider_config.api_key = None;

        // Insert into the correct typed map
        get_typed_provider_map_mut(&mut cfg.generation, gen_type_str)
            .insert(params.name.clone(), provider_config);

        // Save to file
        if let Err(e) = save_config(&cfg) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }
    }

    // Broadcast event
    let _ = event_bus.publish_json(&serde_json::json!({
        "topic": "config.generation.providers.changed",
        "action": "created",
        "provider": params.name,
    }));

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}

/// Update an existing generation provider
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        name: String,
        config: GenerationProviderConfigJson,
    }

    let params: Params = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let incoming = config_from_wire(params.config);

    {
        let mut cfg = config.write().await;

        // Find which typed map contains the provider
        let gen_type_str = match find_provider_type(&cfg.generation, &params.name) {
            Some(t) => t,
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Provider '{}' not found", params.name),
                );
            }
        };

        // The stored entry, read before the mutable borrow. Everything the
        // wire type cannot express lives only here.
        let prior = get_typed_provider_map(&cfg.generation, &gen_type_str)
            .get(&params.name)
            .cloned();

        let mut provider_config = build_generation_provider_for_persistence(
            &params.name,
            incoming,
            &cfg.presets_override.generation,
            prior.as_ref(),
        );

        // Preserve capabilities from the typed map
        provider_config.capabilities = vec![parse_generation_type(&gen_type_str)];

        // Validate provider config
        if let Err(e) = provider_config.validate(&params.name) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Validation failed: {e}"),
            );
        }

        // Store new API key in vault if provided
        if let Some(ref api_key) = provider_config.api_key {
            if let Err(e) = vault.store_secret(&vault_key(&params.name), api_key) {
                error!(error = %e, "Failed to store generation API key in vault");
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to store API key: {e}"),
                );
            }
        }
        provider_config.api_key = None;

        // Update provider — config change resets verified
        provider_config.verified = false;
        get_typed_provider_map_mut(&mut cfg.generation, &gen_type_str)
            .insert(params.name.clone(), provider_config);

        // Save to file
        if let Err(e) = save_config(&cfg) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }
    }

    // Broadcast event
    let _ = event_bus.publish_json(&serde_json::json!({
        "topic": "config.generation.providers.changed",
        "action": "updated",
        "provider": params.name,
    }));

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}

/// Delete a generation provider
pub async fn handle_delete(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        name: String,
    }

    let params: Params = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    {
        let mut cfg = config.write().await;

        // Find which typed map contains the provider
        let gen_type_str = match find_provider_type(&cfg.generation, &params.name) {
            Some(t) => t,
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Provider '{}' not found", params.name),
                );
            }
        };

        // Check if provider is set as default for any generation type
        let mut default_for = Vec::new();
        if cfg.generation.default_image_provider.as_deref() == Some(&params.name) {
            default_for.push("image");
        }
        if cfg.generation.default_video_provider.as_deref() == Some(&params.name) {
            default_for.push("video");
        }
        if cfg.generation.default_audio_provider.as_deref() == Some(&params.name) {
            default_for.push("audio");
        }
        if cfg.generation.default_speech_provider.as_deref() == Some(&params.name) {
            default_for.push("speech");
        }
        if cfg.generation.default_transcription_provider.as_deref() == Some(&params.name) {
            default_for.push("transcription");
        }

        if !default_for.is_empty() {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!(
                    "Cannot delete provider '{}': it is set as default for {}",
                    params.name,
                    default_for.join(", ")
                ),
            );
        }

        // Remove provider from the correct typed map
        get_typed_provider_map_mut(&mut cfg.generation, &gen_type_str).remove(&params.name);

        // Delete API key from vault
        if let Err(e) = vault.delete_secret(&vault_key(&params.name)) {
            warn!(provider = %params.name, error = %e, "Failed to delete generation API key from vault");
        }

        // Save to file
        if let Err(e) = cfg.save_incremental(&["generation"]) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }
    }

    // Broadcast event
    let _ = event_bus.publish_json(&serde_json::json!({
        "topic": "config.generation.providers.changed",
        "action": "deleted",
        "provider": params.name,
    }));

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}

/// Set default provider for a generation type
pub async fn handle_set_default(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        name: String,
        generation_type: GenerationType,
    }

    let params: Params = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    {
        let mut cfg = config.write().await;

        // Find provider across all typed maps
        let gen_type_str = match find_provider_type(&cfg.generation, &params.name) {
            Some(t) => t,
            None => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Provider '{}' not found", params.name),
                );
            }
        };

        let provider_config =
            match get_typed_provider_map(&cfg.generation, &gen_type_str).get(&params.name) {
                Some(c) => c,
                None => {
                    return JsonRpcResponse::error(
                        request.id,
                        INTERNAL_ERROR,
                        format!("Provider '{}' disappeared unexpectedly", params.name),
                    );
                }
            };

        if !provider_config.verified {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!(
                    "Provider '{}' must pass a connection test before being set as default",
                    params.name
                ),
            );
        }

        // Validate that the provider's type matches the requested default type
        let provider_gen_type = parse_generation_type(&gen_type_str);
        if provider_gen_type != params.generation_type {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!(
                    "Provider '{}' is a {} provider and cannot be set as default for {:?}",
                    params.name, gen_type_str, params.generation_type
                ),
            );
        }

        // Set as default
        match params.generation_type {
            GenerationType::Image => {
                cfg.generation.default_image_provider = Some(params.name.clone());
            }
            GenerationType::Video => {
                cfg.generation.default_video_provider = Some(params.name.clone());
            }
            GenerationType::Audio => {
                cfg.generation.default_audio_provider = Some(params.name.clone());
            }
            GenerationType::Speech => {
                cfg.generation.default_speech_provider = Some(params.name.clone());
            }
            GenerationType::Transcription => {
                cfg.generation.default_transcription_provider = Some(params.name.clone());
            }
        }

        // Save to file
        if let Err(e) = cfg.save_incremental(&["generation"]) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }
    }

    // Broadcast event
    let _ = event_bus.publish_json(&serde_json::json!({
        "topic": "config.generation.providers.changed",
        "action": "set_default",
        "provider": params.name,
        "generation_type": params.generation_type,
    }));

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}

/// Test connection to a generation provider
pub async fn handle_test_connection(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        /// Provider name (if provided, persist verified=true on success)
        #[serde(default)]
        name: Option<String>,
        provider_type: String,
        api_key: Option<String>,
        base_url: Option<String>,
    }

    let params: Params = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let provider_name = params.name;

    // If no inline api_key, resolve from vault
    let api_key = match super::super::normalize_optional_string(params.api_key) {
        Some(k) => Some(k),
        None => provider_name
            .as_ref()
            .and_then(|name| resolve_api_key(name, &vault)),
    };

    // Real connectivity probe: an authenticated `GET` against the provider's
    // endpoint (no media generated, so it costs nothing). An API key is
    // required because the probe validates credential acceptance.
    let result = match api_key {
        None => TestConnectionResult {
            success: false,
            message: "API key is required".to_string(),
        },
        Some(key) => {
            let outcome = crate::generation::probe_generation_provider(
                &params.provider_type,
                &key,
                params.base_url.as_deref(),
            )
            .await;

            // Reflect the real verdict in persisted `verified` so the UI badge
            // never claims a provider is verified when the test actually failed.
            if let Some(ref name) = provider_name {
                let mut cfg = config.write().await;
                if let Some(type_str) = find_provider_type(&cfg.generation, name) {
                    if let Some(p) =
                        get_typed_provider_map_mut(&mut cfg.generation, &type_str).get_mut(name)
                    {
                        p.verified = outcome.success;
                        if let Err(e) = save_config(&cfg) {
                            tracing::error!(error = %e, "Failed to save config after generation test");
                        }
                    }
                }
            }

            TestConnectionResult {
                success: outcome.success,
                message: outcome.message,
            }
        }
    };

    match serde_json::to_value(result) {
        Ok(v) => JsonRpcResponse::success(request.id, v),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize result: {e}"),
        ),
    }
}

/// `generation_providers.list_presets` — return the authoritative preset
/// catalogue (id, `provider_type`, `default_model`, `base_url`, `display_name`,
/// modalities, homepage, notes, `signup_url`) read off the unified
/// `GenerationPreset` (Phase 2 fold removed the parallel metadata map).
///
/// Stateless. Panel consumes this to render the preset-card grid; the panel
/// no longer keeps a parallel hard-coded registry. Entries without metadata
/// (i.e. zero declared modalities) are skipped — `modality unknown` means
/// the catalog can't decide which category tab to show.
///
/// # Built from the contract type, not from a `json!` literal
///
/// [`GenerationPresetRow`] is what the Panel parses. Constructing the response
/// from it makes "the server sends a key the client never declared" a compile
/// error rather than a silent drop — which is exactly how `signup_url` reached
/// nobody for the whole life of this endpoint, on the one page where "where do
/// I get a key" is the only action available.
pub async fn handle_list_presets(request: JsonRpcRequest) -> JsonRpcResponse {
    let mut items: Vec<GenerationPresetRow> = PRESETS
        .iter()
        .filter_map(|(id, preset)| {
            let meta = generation_metadata(id)?;
            if preset.modalities.is_empty() {
                return None;
            }
            Some(GenerationPresetRow {
                id: (*id).to_string(),
                provider_type: preset.provider_type.to_string(),
                default_model: preset.default_model.to_string(),
                base_url: preset.base_url.map(String::from),
                display_name: meta.display_name.to_string(),
                modalities: preset
                    .modalities
                    .iter()
                    .map(|m| m.as_str().to_string())
                    .collect(),
                homepage: meta.homepage.map(String::from),
                notes: meta.notes.map(String::from),
                signup_url: preset.signup_url.map(String::from),
            })
        })
        .collect();

    // Stable order: by id ascending (HashMap iteration is unordered).
    items.sort_by(|a, b| a.id.cmp(&b.id));

    match serde_json::to_value(&items) {
        Ok(value) => JsonRpcResponse::success(request.id, value),
        // Unreachable for a Vec of plain owned strings, but an empty array is
        // a lie about the catalogue — say which it is.
        Err(e) => {
            error!(error = %e, "list_presets: failed to encode the preset catalogue");
            JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                "failed to encode the generation preset catalogue".to_string(),
            )
        }
    }
}

#[cfg(test)]
mod list_presets_tests {
    use super::*;
    use crate::gateway::protocol::JsonRpcRequest;

    fn request() -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(1)),
            method: "generation_providers.list_presets".to_string(),
            params: None,
        }
    }

    #[tokio::test]
    async fn list_presets_returns_known_presets_with_metadata() {
        let resp = handle_list_presets(request()).await;
        let arr = resp.result.expect("result").as_array().cloned().unwrap();
        assert!(arr.len() >= 20, "expected ≥20 presets, got {}", arr.len());

        // Every entry has the required UI fields.
        for entry in &arr {
            assert!(entry.get("id").and_then(|v| v.as_str()).is_some());
            assert!(entry
                .get("provider_type")
                .and_then(|v| v.as_str())
                .is_some());
            assert!(entry
                .get("default_model")
                .and_then(|v| v.as_str())
                .is_some());
            assert!(entry.get("display_name").and_then(|v| v.as_str()).is_some());
            let mods = entry.get("modalities").and_then(|v| v.as_array()).unwrap();
            assert!(!mods.is_empty(), "modalities cannot be empty for {entry}");
        }
    }

    #[tokio::test]
    async fn list_presets_includes_round3_additions() {
        let resp = handle_list_presets(request()).await;
        let arr = resp.result.expect("result").as_array().cloned().unwrap();
        let ids: Vec<&str> = arr
            .iter()
            .filter_map(|e| e.get("id").and_then(|v| v.as_str()))
            .collect();
        for required in [
            "deepgram-tts",
            "azure-speech",
            "suno",
            "bfl-flux",
            "cartesia",
        ] {
            assert!(ids.contains(&required), "missing preset {required}");
        }
    }

    #[tokio::test]
    async fn list_presets_is_sorted_by_id() {
        let resp = handle_list_presets(request()).await;
        let arr = resp.result.expect("result").as_array().cloned().unwrap();
        let ids: Vec<&str> = arr
            .iter()
            .filter_map(|e| e.get("id").and_then(|v| v.as_str()))
            .collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted, "preset list must be sorted by id");
    }

    /// The catalogue speaks only the contract's vocabulary — no more, no less.
    ///
    /// Both directions matter and only one of them is free. Parsing proves the
    /// response is a *superset* of what the client declared, because serde
    /// ignores unknown keys; it can never see an over-send. So the expected key
    /// set is derived by serialising a fully populated contract row rather than
    /// written out as a literal — a literal list is the same enumeration error
    /// one level up.
    #[tokio::test]
    async fn the_preset_catalogue_speaks_only_the_contracts_vocabulary() {
        let expected: std::collections::BTreeSet<String> =
            serde_json::to_value(GenerationPresetRow {
                id: "x".into(),
                provider_type: "x".into(),
                default_model: "x".into(),
                base_url: Some("x".into()),
                display_name: "x".into(),
                modalities: vec!["image".into()],
                homepage: Some("x".into()),
                notes: Some("x".into()),
                signup_url: Some("x".into()),
            })
            .expect("contract row must serialise")
            .as_object()
            .expect("contract row must be an object")
            .keys()
            .cloned()
            .collect();

        let resp = handle_list_presets(request()).await;
        let arr = resp.result.expect("result").as_array().cloned().unwrap();
        assert!(!arr.is_empty(), "an empty catalogue proves nothing");

        for entry in &arr {
            // Every row must decode into the type the Panel holds…
            let row: GenerationPresetRow = serde_json::from_value(entry.clone())
                .unwrap_or_else(|e| panic!("client must decode {entry}: {e}"));
            // …and carry no key the contract does not declare. Optionals the
            // preset legitimately omits are absent, so compare the row's own
            // keys against the contract's, minus the ones it skipped.
            let sent: std::collections::BTreeSet<String> = entry
                .as_object()
                .expect("row must be an object")
                .keys()
                .cloned()
                .collect();
            assert!(
                sent.is_subset(&expected),
                "row {} sends {:?}, which the contract does not declare",
                row.id,
                sent.difference(&expected).collect::<Vec<_>>()
            );
        }
    }

    /// `signup_url` is the reason this endpoint has a typed row at all: it was
    /// served for every preset that declares one and declared by no client, so
    /// serde dropped it silently on the one page where "where do I get a key"
    /// is the only available action.
    #[tokio::test]
    async fn the_signup_link_reaches_the_client() {
        let resp = handle_list_presets(request()).await;
        let arr = resp.result.expect("result").as_array().cloned().unwrap();
        let rows: Vec<GenerationPresetRow> = arr
            .iter()
            .map(|e| serde_json::from_value(e.clone()).expect("decode"))
            .collect();

        let with_signup = rows.iter().filter(|r| r.signup_url.is_some()).count();
        assert!(
            with_signup >= 10,
            "expected the curated signup links to survive the wire, got {with_signup}"
        );
        let dalle = rows
            .iter()
            .find(|r| r.id == "openai-dalle")
            .expect("openai-dalle is in the registry");
        assert!(
            dalle.signup_url.is_some(),
            "openai-dalle declares a signup url in the registry"
        );
    }
}
