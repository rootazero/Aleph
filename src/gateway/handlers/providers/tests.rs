//! Tests for provider handlers.

use super::*;
use crate::config::{Config, ProviderConfig};
use crate::gateway::protocol::JsonRpcRequest;
use crate::gateway::security::{SecurityStore, SharedTokenManager};
use crate::sync_primitives::Arc;
use serde_json::json;
use tokio::sync::RwLock;

fn test_vault() -> Arc<SharedTokenManager> {
    let store = Arc::new(SecurityStore::in_memory().unwrap());
    let tmp = std::env::temp_dir().join(format!(
        "test_ai_provider_vault_{}_{}.vault",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let vault = Arc::new(SharedTokenManager::new(store, tmp));
    let _ = vault.generate_token().unwrap();
    vault
}

fn config_with_provider(name: &str) -> Config {
    let mut config = Config::default();
    config
        .providers
        .insert(name.to_string(), ProviderConfig::test_config("gpt-4o"));
    config
}

#[test]
fn test_update_params() {
    let json = json!({
        "name": "openai",
        "config": {
            "enabled": true,
            "model": "gpt-4"
        }
    });
    let params: UpdateParams = serde_json::from_value(json).unwrap();
    assert_eq!(params.name, "openai");
    assert_eq!(params.config.models, vec!["gpt-4"]);
}

#[test]
fn provider_config_json_accepts_context_window() {
    // create/update accept an operator-declared window through the JSON DTO.
    let parsed: ProviderConfigJson = serde_json::from_value(json!({
        "enabled": true,
        "model": "kimi-k2",
        "context_window": 200_000
    }))
    .unwrap();
    assert_eq!(parsed.context_window, Some(200_000));

    // Absent → None (back-compat: old panel/CLI payloads still deserialize).
    let bare: ProviderConfigJson =
        serde_json::from_value(json!({ "enabled": true, "model": "gpt-4o" })).unwrap();
    assert_eq!(bare.context_window, None);
}

#[test]
fn provider_info_round_trips_context_window() {
    // get/list expose the window; None is omitted from the wire (skip_if).
    let info = ProviderInfo {
        name: "kimi".into(),
        enabled: true,
        models: vec!["kimi-k2".into()],
        model: "kimi-k2".into(),
        provider_type: None,
        has_api_key: false,
        api_key: None,
        base_url: None,
        color: "#808080".into(),
        timeout_seconds: 300,
        max_tokens: None,
        context_window: Some(200_000),
        temperature: None,
        is_default: false,
        verified: false,
    };
    let j = serde_json::to_value(&info).unwrap();
    assert_eq!(j["context_window"], 200_000);

    let omitted = ProviderInfo {
        context_window: None,
        ..info
    };
    let j2 = serde_json::to_value(&omitted).unwrap();
    assert!(
        j2.get("context_window").is_none(),
        "None context_window omitted from response"
    );
}

#[test]
fn test_test_result_serialize() {
    let result = TestResult {
        success: true,
        error: None,
        latency_ms: Some(150),
    };
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["success"], true);
    assert_eq!(json["latency_ms"], 150);
}

#[tokio::test]
async fn test_healthcheck_empty_providers() {
    let config = Arc::new(RwLock::new(Config::default()));
    let vault = test_vault();
    let request = JsonRpcRequest::with_id("providers.healthcheck", None, json!(1));
    let response = handle_healthcheck(request, config, vault).await;
    let result = response.result.unwrap();
    let providers = result["providers"].as_array().unwrap();
    assert!(
        providers.is_empty(),
        "no providers configured → empty sweep"
    );
}

#[tokio::test]
async fn test_healthcheck_skips_disabled_without_probing() {
    // A disabled provider must be reported as skipped and must NOT trigger a
    // network probe — this keeps the test hermetic (no outbound I/O).
    let mut config = config_with_provider("openai");
    config.providers.get_mut("openai").unwrap().enabled = false;
    let config = Arc::new(RwLock::new(config));
    let vault = test_vault();

    let request = JsonRpcRequest::with_id("providers.healthcheck", None, json!(1));
    let response = handle_healthcheck(request, config, vault).await;
    let result = response.result.unwrap();
    let providers = result["providers"].as_array().unwrap();

    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0]["name"], "openai");
    assert_eq!(providers[0]["enabled"], false);
    assert_eq!(providers[0]["skipped"], true);
    assert_eq!(providers[0]["ok"], false);
    assert!(providers[0].get("latency_ms").is_none());
}

#[test]
fn test_provider_health_row_serialize() {
    let row = ProviderHealthRow {
        name: "openai".to_string(),
        enabled: true,
        ok: true,
        skipped: false,
        latency_ms: Some(120),
        error: None,
    };
    let json = serde_json::to_value(&row).unwrap();
    assert_eq!(json["name"], "openai");
    assert_eq!(json["ok"], true);
    assert_eq!(json["latency_ms"], 120);
    // error omitted when None
    assert!(json.get("error").is_none());
}

#[tokio::test]
async fn test_needs_setup_empty_providers() {
    let config = Arc::new(RwLock::new(Config::default()));
    let request = JsonRpcRequest::with_id("providers.needsSetup", None, serde_json::json!(1));
    let response = handle_needs_setup(request, config).await;
    let result: serde_json::Value = serde_json::from_value(response.result.unwrap()).unwrap();
    assert_eq!(result["needs_setup"], true);
    assert_eq!(result["provider_count"], 0);
    assert_eq!(result["has_verified"], false);
}

#[tokio::test]
async fn test_needs_setup_has_verified_provider() {
    let mut config = Config::default();
    let mut provider_cfg = ProviderConfig::test_config("gpt-4o");
    provider_cfg.enabled = true;
    provider_cfg.verified = true;
    config.providers.insert("openai".to_string(), provider_cfg);
    let config = Arc::new(RwLock::new(config));
    let request = JsonRpcRequest::with_id("providers.needsSetup", None, serde_json::json!(1));
    let response = handle_needs_setup(request, config).await;
    let result: serde_json::Value = serde_json::from_value(response.result.unwrap()).unwrap();
    assert_eq!(result["needs_setup"], false);
    assert_eq!(result["provider_count"], 1);
    assert_eq!(result["has_verified"], true);
}

#[tokio::test]
async fn test_needs_setup_has_unverified_provider() {
    let mut config = Config::default();
    let mut provider_cfg = ProviderConfig::test_config("gpt-4o");
    provider_cfg.enabled = true;
    provider_cfg.verified = false;
    config.providers.insert("openai".to_string(), provider_cfg);
    let config = Arc::new(RwLock::new(config));
    let request = JsonRpcRequest::with_id("providers.needsSetup", None, serde_json::json!(1));
    let response = handle_needs_setup(request, config).await;
    let result: serde_json::Value = serde_json::from_value(response.result.unwrap()).unwrap();
    assert_eq!(result["needs_setup"], true);
    assert_eq!(result["provider_count"], 1);
    assert_eq!(result["has_verified"], false);
}

// Security (3def857c6): list/get report `has_api_key` from the vault but never
// echo the plaintext secret back in the response.
#[tokio::test]
async fn test_handle_list_reports_has_api_key_without_echoing_secret() {
    let config = Arc::new(RwLock::new(config_with_provider("toapis")));
    let vault = test_vault();
    vault
        .store_secret("ai:toapis", "test-toapis-key")
        .expect("store ai provider secret");

    let request = JsonRpcRequest::with_id("providers.list", None, json!(1));
    let response = handle_list(request, config, vault).await;
    assert!(response.is_success());

    let result = response.result.unwrap();
    let providers = result["providers"].as_array().unwrap();
    assert_eq!(providers.len(), 1);
    let provider = &providers[0];
    assert_eq!(provider["name"], "toapis");
    // Plaintext key is never serialized into list responses.
    assert!(provider.get("api_key").is_none() || provider["api_key"].is_null());
    assert_eq!(provider["has_api_key"], true);
}

#[tokio::test]
async fn test_handle_get_reports_has_api_key_without_echoing_secret() {
    let config = Arc::new(RwLock::new(config_with_provider("toapis")));
    let vault = test_vault();
    vault
        .store_secret("ai:toapis", "test-toapis-key")
        .expect("store ai provider secret");

    let request =
        JsonRpcRequest::with_id("providers.get", Some(json!({ "name": "toapis" })), json!(1));
    let response = handle_get(request, config, vault).await;
    assert!(response.is_success());

    let result = response.result.unwrap();
    let provider = &result["provider"];
    assert_eq!(provider["name"], "toapis");
    // Plaintext key is never serialized into get responses.
    assert!(provider.get("api_key").is_none() || provider["api_key"].is_null());
    assert_eq!(provider["has_api_key"], true);
}

// ============================================================================
// providers.catalog — chat-window picker join of presets + credentials.
// ============================================================================

fn catalog_request(view: Option<&str>) -> JsonRpcRequest {
    let params = view.map(|v| json!({ "view": v }));
    JsonRpcRequest::with_id("providers.catalog", params, json!(1))
}

fn items_array(response: &crate::gateway::protocol::JsonRpcResponse) -> Vec<serde_json::Value> {
    response
        .result
        .as_ref()
        .and_then(|v| v.get("items"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn catalog_all_view_lists_every_chat_preset() {
    let config = Arc::new(RwLock::new(Config::default()));
    let vault = test_vault();
    let response = handle_catalog(catalog_request(Some("all")), config, vault).await;
    assert!(response.is_success());
    let items = items_array(&response);
    assert!(
        items.len() >= 20,
        "expected ≥20 chat presets, got {}",
        items.len()
    );
    let ids: Vec<&str> = items
        .iter()
        .filter_map(|e| e.get("id").and_then(|v| v.as_str()))
        .collect();
    for required in ["openai", "claude", "deepseek", "qwen", "gemini"] {
        assert!(ids.contains(&required), "missing chat preset {required}");
    }
}

#[tokio::test]
async fn catalog_configured_view_filters_empty_config_to_empty() {
    // [moa] config is process-global; pin it to None under the shared lock so
    // a concurrent moa-catalog test can't inject a stray "moa" row while this
    // test asserts strict emptiness (same pattern as moa_manage.rs tests).
    let _guard = crate::providers::moa::config_handle::moa_config_test_lock();
    crate::providers::moa::store_moa_config(None);

    let config = Arc::new(RwLock::new(Config::default()));
    let vault = test_vault();
    let response = handle_catalog(catalog_request(Some("configured")), config, vault).await;
    assert!(response.is_success());
    let items = items_array(&response);
    assert!(
        items.is_empty(),
        "configured view on empty config should be empty"
    );
}

#[tokio::test]
async fn catalog_default_view_is_configured() {
    // Emptiness assertion → pin the process-global [moa] slot (see above).
    let _guard = crate::providers::moa::config_handle::moa_config_test_lock();
    crate::providers::moa::store_moa_config(None);

    let config = Arc::new(RwLock::new(Config::default()));
    let vault = test_vault();
    let response = handle_catalog(catalog_request(None), config, vault).await;
    assert!(response.is_success());
    assert!(
        items_array(&response).is_empty(),
        "omitting `view` must mirror 'configured'"
    );
}

#[tokio::test]
async fn catalog_configured_view_returns_verified_enabled_entry() {
    // Strict `len == 1` assertion → pin the process-global [moa] slot.
    let _guard = crate::providers::moa::config_handle::moa_config_test_lock();
    crate::providers::moa::store_moa_config(None);

    let mut config = Config::default();
    let mut cfg = ProviderConfig::test_config("gpt-4o");
    cfg.enabled = true;
    cfg.verified = true;
    config.providers.insert("openai".to_string(), cfg);
    let config = Arc::new(RwLock::new(config));
    let vault = test_vault();

    let response = handle_catalog(catalog_request(Some("configured")), config, vault).await;
    let items = items_array(&response);
    assert_eq!(items.len(), 1);
    let entry = &items[0];
    assert_eq!(entry["id"], "openai");
    assert_eq!(entry["verified"], true);
    assert_eq!(entry["enabled"], true);
    // Endpoint locality is surfaced; the OpenAI preset is a public API.
    assert_eq!(entry["endpoint"], "cloud");
    // User-extended model list flows through.
    let models = entry["models"].as_array().unwrap();
    assert_eq!(models[0], "gpt-4o");
}

#[tokio::test]
async fn catalog_configured_view_includes_custom_non_preset_provider() {
    // A user-defined provider whose name is NOT a built-in chat preset (e.g.
    // an OpenAI-compatible relay added via `providers.create`). It must still
    // surface in the model picker once enabled + verified — regression guard
    // for the preset-only catalog that hid such providers entirely.
    let mut config = Config::default();
    let mut cfg = ProviderConfig::test_config("claude-sonnet-4-6");
    cfg.enabled = true;
    cfg.verified = true;
    config.providers.insert("302ai".to_string(), cfg);
    let config = Arc::new(RwLock::new(config));
    let vault = test_vault();

    let response = handle_catalog(catalog_request(Some("configured")), config, vault).await;
    let items = items_array(&response);
    let entry = items
        .iter()
        .find(|e| e["id"] == "302ai")
        .expect("custom provider must appear in the configured catalog");
    assert_eq!(entry["verified"], true);
    assert_eq!(entry["enabled"], true);
    assert_eq!(entry["default_model"], "claude-sonnet-4-6");
    let mods = entry["modalities"].as_array().unwrap();
    assert!(mods.iter().any(|m| m.as_str() == Some("chat")));
}

#[tokio::test]
async fn catalog_configured_view_hides_unverified_custom_provider() {
    // Same custom provider but unverified → excluded from the "configured"
    // view, mirroring preset behaviour.
    let mut config = Config::default();
    let mut cfg = ProviderConfig::test_config("claude-sonnet-4-6");
    cfg.enabled = true;
    cfg.verified = false;
    config.providers.insert("302ai".to_string(), cfg);
    let config = Arc::new(RwLock::new(config));
    let vault = test_vault();

    let response = handle_catalog(catalog_request(Some("configured")), config, vault).await;
    let items = items_array(&response);
    assert!(
        !items.iter().any(|e| e["id"] == "302ai"),
        "unverified custom provider must not appear in the configured view"
    );
}

#[tokio::test]
async fn catalog_entries_carry_modalities_default_model() {
    let config = Arc::new(RwLock::new(Config::default()));
    let vault = test_vault();
    let response = handle_catalog(catalog_request(Some("all")), config, vault).await;
    for item in items_array(&response) {
        assert!(item.get("id").and_then(|v| v.as_str()).is_some());
        assert!(item.get("display_name").and_then(|v| v.as_str()).is_some());
        assert!(item.get("default_model").and_then(|v| v.as_str()).is_some());
        assert!(item.get("protocol").and_then(|v| v.as_str()).is_some());
        let mods = item.get("modalities").and_then(|v| v.as_array()).unwrap();
        assert!(mods.iter().any(|m| m.as_str() == Some("chat")));
    }
}

#[tokio::test]
async fn catalog_unknown_view_treats_as_all() {
    // Row-set assertion → pin the process-global [moa] slot so the fall-
    // through view is exactly the preset catalog, no synthetic moa row.
    let _guard = crate::providers::moa::config_handle::moa_config_test_lock();
    crate::providers::moa::store_moa_config(None);

    let config = Arc::new(RwLock::new(Config::default()));
    let vault = test_vault();
    let response = handle_catalog(catalog_request(Some("nonsense")), config, vault).await;
    let items = items_array(&response);
    // Unknown view → fall through to "all", returning every preset.
    assert!(items.len() >= 20);
}

// ============================================================================
// Round-2 E3: MoA presets ride providers.catalog as a "moa" pseudo-provider
// row. `[moa]` config lives in a process-global slot (`config_handle`), so
// these tests take `moa_config_test_lock()` — the same guard
// `builtin_tools::moa_manage`'s tests use — to serialize against other tests
// that mutate it.
// ============================================================================

fn solo_moa_preset() -> crate::config::MoaPreset {
    crate::config::MoaPreset {
        enabled: true,
        advisors: vec![crate::config::MoaSlot {
            provider: "openai".to_string(),
            model: "gpt-5".to_string(),
        }],
        aggregator: crate::config::MoaSlot {
            provider: "anthropic".to_string(),
            model: "claude-opus-4".to_string(),
        },
        fanout: crate::config::MoaFanout::default(),
        advisor_timeout_secs: 120,
        advisor_max_tokens: None,
        advisor_temperature: None,
        aggregator_temperature: None,
    }
}

#[tokio::test]
async fn catalog_includes_moa_pseudo_entry_when_presets_enabled() {
    let _guard = crate::providers::moa::config_handle::moa_config_test_lock();
    let mut moa = crate::config::MoaToml::default();
    moa.presets.insert("deep".to_string(), solo_moa_preset());
    moa.default_preset = Some("deep".to_string());
    crate::providers::moa::store_moa_config(Some(moa));

    let config = Arc::new(RwLock::new(Config::default()));
    let vault = test_vault();
    let response = handle_catalog(catalog_request(Some("all")), config, vault).await;
    let items = items_array(&response);
    let entry = items
        .iter()
        .find(|e| e["id"] == "moa")
        .expect("moa pseudo-entry must appear when an enabled preset exists");
    assert_eq!(entry["display_name"], "Mixture of Agents");
    assert_eq!(entry["default_model"], "deep");
    assert_eq!(entry["models"], json!(["deep"]));
    assert_eq!(entry["has_api_key"], true);
    assert_eq!(entry["protocol"], "moa");

    crate::providers::moa::store_moa_config(None);
}

#[tokio::test]
async fn catalog_omits_moa_entry_without_moa_config() {
    let _guard = crate::providers::moa::config_handle::moa_config_test_lock();
    crate::providers::moa::store_moa_config(None);

    let config = Arc::new(RwLock::new(Config::default()));
    let vault = test_vault();
    let response = handle_catalog(catalog_request(Some("all")), config, vault).await;
    let items = items_array(&response);
    assert!(
        !items.iter().any(|e| e["id"] == "moa"),
        "no moa entry expected when [moa] config is absent"
    );
}

// Regression: providers.update must hot-reload the runtime provider instance
// so protocol/base_url/model changes take effect without a daemon restart.
#[tokio::test]
async fn test_handle_update_hot_reloads_runtime_provider_protocol() {
    use crate::providers::create_provider;
    use crate::thinker::{MultiProviderRegistry, ProviderRegistry};

    // Isolate on-disk config writes to a temp dir; ALEPH_HOME is process-global
    // and other tests may leave it pointing at a dropped temp directory.
    let _guard = crate::utils::paths::ALEPH_HOME_TEST_GUARD
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let prev_aleph_home = std::env::var_os("ALEPH_HOME");
    let tmp = std::env::temp_dir().join(".aleph").join(format!(
        "providers_test_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    tokio::fs::create_dir_all(&tmp)
        .await
        .expect("create test aleph home");
    std::env::set_var("ALEPH_HOME", &tmp);

    let mut config = Config::default();
    let initial_cfg = ProviderConfig::test_config("gpt-4o");
    config
        .providers
        .insert("custom".to_string(), initial_cfg.clone());
    let config = Arc::new(RwLock::new(config));

    let vault = test_vault();
    vault
        .store_secret("ai:custom", "test-key")
        .expect("store provider key");

    // Seed the live registry with an openai-protocol instance.
    let registry = {
        let mut cfg_with_key = initial_cfg.clone();
        cfg_with_key.api_key = Some("test-key".to_string());
        cfg_with_key.protocol = Some("openai".to_string());
        let provider = create_provider("custom", cfg_with_key).expect("create initial provider");
        Arc::new(MultiProviderRegistry::new("custom".to_string(), provider))
    };

    assert_eq!(
        registry.default_provider().protocol().as_ref(),
        "openai",
        "initial runtime protocol should be openai"
    );

    let request = JsonRpcRequest::with_id(
        "providers.update",
        Some(json!({
            "name": "custom",
            "config": {
                "protocol": "anthropic",
                "enabled": true,
                "model": "claude-sonnet-4-6",
                "api_key": "test-key"
            }
        })),
        json!(1),
    );

    let event_bus = Arc::new(crate::gateway::event_bus::GatewayEventBus::new());
    let response = handle_update_hot(request, config, event_bus, vault, registry.clone()).await;
    assert!(response.is_success(), "update failed: {:?}", response.error);

    assert_eq!(
        registry.default_provider().protocol().as_ref(),
        "anthropic",
        "providers.update must hot-reload the runtime provider with the new protocol"
    );

    match prev_aleph_home {
        Some(v) => std::env::set_var("ALEPH_HOME", v),
        None => std::env::remove_var("ALEPH_HOME"),
    }
    let _ = std::fs::remove_dir_all(&tmp);
}
