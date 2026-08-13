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
async fn test_provider_test_redacts_probe_error() {
    let secret = "dXNlcjpwYXNzd29yZA==";
    let request = JsonRpcRequest::with_id(
        "providers.test",
        Some(json!({
            "config": {
                "protocol": format!("Basic {secret}"),
                "enabled": true,
                "model": "test-model"
            }
        })),
        json!(1),
    );
    let response = handle_test(
        request,
        Arc::new(RwLock::new(Config::default())),
        test_vault(),
    )
    .await;
    let result = response.result.unwrap();
    let error = result["error"].as_str().unwrap();
    assert!(!error.contains(secret));
    assert!(error.contains("***"));
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

/// The sweep must not dial a preset that declares `/models` probing cannot
/// answer for it.
///
/// This arm read `enabled` alone, so all six opt-out presets — the OAuth-only
/// endpoints and the per-deployment hosts — were dialled and came back
/// `unreachable`. The doctor check next door had honoured the opt-out since it
/// was written; the two faces of one verb had two derivations, and only the
/// wrong one was operator-facing. Both now read `probe::probe_disposition`.
///
/// `chatgpt` carries `.no_health_check()`. Skipped-because-opted-out is told
/// apart from skipped-because-disabled by `enabled`, which is why the row
/// needs no third field.
#[tokio::test]
async fn healthcheck_skips_a_preset_that_opts_out_of_probing() {
    let mut config = config_with_provider("chatgpt");
    config.providers.get_mut("chatgpt").unwrap().enabled = true;
    let request = JsonRpcRequest::with_id("providers.healthcheck", None, json!(1));
    let response = handle_healthcheck(request, Arc::new(RwLock::new(config)), test_vault()).await;
    let result = response.result.unwrap();
    let row = &result["providers"][0];

    assert_eq!(row["name"], "chatgpt");
    assert_eq!(
        row["skipped"], true,
        "an opt-out preset must not be dialled"
    );
    assert_eq!(
        row["enabled"], true,
        "the operator did not disable it — `enabled` is what separates the two \
         reasons to skip"
    );
    assert!(
        row.get("error").is_none(),
        "not dialling is not a failure, so there is nothing to report"
    );
}

#[tokio::test]
async fn test_healthcheck_redacts_probe_error() {
    let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
    let mut config = config_with_provider("leaky");
    let provider = config.providers.get_mut("leaky").unwrap();
    provider.enabled = true;
    provider.protocol = Some(jwt.to_string());
    let request = JsonRpcRequest::with_id("providers.healthcheck", None, json!(1));
    let response = handle_healthcheck(request, Arc::new(RwLock::new(config)), test_vault()).await;
    let result = response.result.unwrap();
    let error = result["providers"][0]["error"].as_str().unwrap();
    assert!(!error.contains(jwt));
    assert!(error.contains("***"));
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

/// The roster's ids, in order.
///
/// A roster row carries provenance and lifecycle beside the id — projecting it
/// back down to a `Vec<String>` here is fine because these tests are about
/// *order and membership*; the row shape has its own assertions.
fn roster_ids(entry: &serde_json::Value) -> Vec<String> {
    entry["roster"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| m.get("id").and_then(|v| v.as_str()).map(str::to_string))
                .collect()
        })
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
async fn catalog_roster_merges_curated_rungs_behind_operator_models() {
    // The picker roster comes from the same `presets::model_ladder` leaf the
    // failover walk merges through: operator models first, unlisted curated
    // `fallback_models` rungs appended behind them.
    let mut config = Config::default();
    let mut cfg = ProviderConfig::test_config("gpt-4o");
    cfg.enabled = true;
    cfg.verified = true;
    config.providers.insert("openai".to_string(), cfg);
    let config = Arc::new(RwLock::new(config));
    let vault = test_vault();

    let response = handle_catalog(catalog_request(Some("configured")), config, vault).await;
    let items = items_array(&response);
    let entry = items.iter().find(|e| e["id"] == "openai").unwrap();
    let roster = roster_ids(entry);
    assert_eq!(roster[0], "gpt-4o", "operator's first model stays first");
    let preset = crate::providers::presets::get_preset("openai").unwrap();
    for rung in preset.fallback_models {
        assert!(
            roster.iter().any(|m| m.eq_ignore_ascii_case(rung)),
            "curated rung {rung} missing from the picker roster"
        );
    }
}

#[tokio::test]
async fn catalog_roster_skips_curated_rungs_when_base_url_moved() {
    // A relay whose base_url the operator moved serves its own inventory —
    // the curated preset ids would be opaque 400s there, so the roster must
    // be the operator's list alone. This is the guard the picker could not
    // evaluate frontend-side.
    let mut config = Config::default();
    let mut cfg = ProviderConfig::test_config("gpt-4o");
    cfg.enabled = true;
    cfg.verified = true;
    cfg.base_url = Some("https://relay.internal/v1".to_string());
    config.providers.insert("openai".to_string(), cfg);
    let config = Arc::new(RwLock::new(config));
    let vault = test_vault();

    let response = handle_catalog(catalog_request(Some("configured")), config, vault).await;
    let items = items_array(&response);
    let entry = items.iter().find(|e| e["id"] == "openai").unwrap();
    assert_eq!(roster_ids(entry), vec!["gpt-4o"]);
}

#[tokio::test]
async fn catalog_roster_defaults_to_preset_chain_when_unconfigured() {
    let config = Arc::new(RwLock::new(Config::default()));
    let vault = test_vault();
    let response = handle_catalog(catalog_request(Some("all")), config, vault).await;
    let items = items_array(&response);

    // Unconfigured preset: roster is the curated chain, default first.
    let entry = items.iter().find(|e| e["id"] == "openai").unwrap();
    let roster = roster_ids(entry);
    assert_eq!(roster[0], entry["default_model"].as_str().unwrap());
    assert!(roster.len() > 1, "curated rungs must ride the roster");

    // BYO-model relay: no default, no rungs → empty roster, never [""].
    let byo = items.iter().find(|e| e["id"] == "t8star").unwrap();
    assert!(roster_ids(byo).is_empty());
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

// ============================================================================
// Cross-crate contract reconciliation
// ============================================================================
//
// `aleph-cli` and `aleph-tui` cannot depend on `alephcore`, so `alephcore` is
// the only crate that sees both halves of this wire. These tests live here for
// that reason and no other.
//
// They check the direction a parse test structurally cannot: deserialising a
// real response into a contract type proves the response is a **superset** —
// serde ignores unknown keys — so an over-sending handler stays invisible. The
// expected key set is therefore *derived from the contract type itself*, never
// written out as a literal, because a literal list is the same drift moved one
// level up.

/// Every key a fully-populated contract instance can emit.
///
/// Serialising a value with every `Option` set and every `Vec` non-empty
/// defeats `skip_serializing_if`, so this is the complete vocabulary the type
/// is allowed to speak.
fn contract_keys<T: serde::Serialize>(fully_populated: &T) -> std::collections::BTreeSet<String> {
    serde_json::to_value(fully_populated)
        .expect("contract type must serialise")
        .as_object()
        .expect("contract row must be a JSON object")
        .keys()
        .cloned()
        .collect()
}

fn object_keys(v: &serde_json::Value) -> std::collections::BTreeSet<String> {
    v.as_object()
        .expect("row must be a JSON object")
        .keys()
        .cloned()
        .collect()
}

fn fully_populated_catalog_entry() -> CatalogEntry {
    CatalogEntry {
        id: "openai".into(),
        display_name: "OpenAI".into(),
        default_model: "gpt-5.6".into(),
        base_url: "https://api.openai.com/v1".into(),
        protocol: "openai".into(),
        color: "#10A37F".into(),
        homepage: Some("https://openai.com".into()),
        notes: Some("note".into()),
        signup_url: Some("https://platform.openai.com".into()),
        fallback_models: vec!["gpt-5.6-luna".into()],
        default_aux_model: Some("gpt-5.6-luna".into()),
        aliases: vec!["oai".into()],
        modalities: vec!["chat".into()],
        models: vec!["gpt-5.6".into()],
        has_api_key: true,
        verified: true,
        enabled: true,
        is_default: true,
        auth_kind: AuthKind::ApiKey,
        capabilities: Some(crate::providers::ModelCapabilities {
            context_window: 1,
            max_output_tokens: 1,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        }),
        cost: Some(crate::pricing::RateCard {
            input_per_mtok: Some(1.0),
            output_per_mtok: Some(1.0),
            cache_read_per_mtok: Some(1.0),
            cache_creation_per_mtok: Some(1.0),
            reasoning_per_mtok: Some(1.0),
            basis: crate::pricing::RateBasis::Direct,
        }),
        endpoint: "cloud".into(),
        lifecycle: crate::providers::model_catalog::ModelLifecycle::ACTIVE,
        requires_explicit_model: true,
        discoverable: true,
        roster: vec![RosterModel::new(
            "gpt-5.6",
            crate::providers::model_catalog::ModelSource::PresetDefault,
        )],
    }
}

#[tokio::test]
async fn the_catalog_response_speaks_only_the_contracts_vocabulary() {
    // Over-sending is what the `workspace.get` round found: an entire internal
    // struct reached the wire, four fields of which had no writer and no
    // reader anywhere, and the parse-shaped test could not see it.
    let config = Arc::new(RwLock::new(Config::default()));
    let response = handle_catalog(catalog_request(Some("all")), config, test_vault()).await;
    let items = items_array(&response);
    assert!(!items.is_empty(), "fixture must produce rows to inspect");

    let allowed = contract_keys(&fully_populated_catalog_entry());
    for item in &items {
        let extra: Vec<String> = object_keys(item).difference(&allowed).cloned().collect();
        assert!(
            extra.is_empty(),
            "providers.catalog emitted {extra:?}, which `aleph_protocol::providers::CatalogEntry` \
             does not declare. Either add the field to the contract (so every client can read it) \
             or stop sending it — a key no client can name is bytes paid for on every call."
        );
    }
}

#[tokio::test]
async fn every_catalog_row_deserialises_into_the_contract_type() {
    // The other direction: a client holding only `aleph-protocol` must be able
    // to read what the server sends, including the rows built by the custom
    // and MoA arms rather than the preset arm.
    let mut config = Config::default();
    let mut cfg = ProviderConfig::test_config("some-relay-model");
    cfg.enabled = true;
    cfg.verified = true;
    config.providers.insert("my-relay".to_string(), cfg);
    let config = Arc::new(RwLock::new(config));

    let response = handle_catalog(catalog_request(Some("all")), config, test_vault()).await;
    for item in items_array(&response) {
        let parsed: Result<CatalogEntry, _> = serde_json::from_value(item.clone());
        assert!(
            parsed.is_ok(),
            "a client cannot decode this row: {}\n{item}",
            parsed.unwrap_err()
        );
    }
}

#[tokio::test]
async fn a_roster_row_carries_its_provenance_and_lifecycle() {
    // The roster used to be `Vec<String>`. Projecting records down to scalars
    // deletes every other field for every renderer at once, and it happens in
    // the producer, so each renderer still looks correct. This pins the shape.
    let config = Arc::new(RwLock::new(Config::default()));
    let response = handle_catalog(catalog_request(Some("all")), config, test_vault()).await;
    let items = items_array(&response);
    let entry = items
        .iter()
        .find(|e| e["id"] == "openai")
        .expect("openai preset must be listed");

    let first = &entry["roster"][0];
    assert!(
        first.get("id").and_then(|v| v.as_str()).is_some(),
        "roster row must carry an id: {first}"
    );
    assert!(
        first.get("source").and_then(|v| v.as_str()).is_some(),
        "roster row must say where the id came from: {first}"
    );
    assert!(
        first.get("lifecycle").is_some(),
        "roster row must carry lifecycle so a picker can mark a retired id: {first}"
    );
}

#[tokio::test]
async fn a_create_request_built_from_the_contract_type_is_accepted() {
    // The CLI used to send a flat `{name, type, api_key, base_url}` body and
    // got INVALID_PARAMS on every invocation it ever made. Building the request
    // by serialising `CreateParams` is what makes the wrong shape a compile
    // error; this checks the handler's deserialiser agrees with the encoder —
    // `alias`, `deserialize_with` and missing-default all live in that gap.
    let params = CreateParams {
        name: "my-relay".to_string(),
        config: ProviderConfigJson::new(vec!["model-a".into(), "model-b".into()]),
    };
    let wire = serde_json::to_value(&params).expect("contract type must serialise");

    let decoded: CreateParams =
        serde_json::from_value(wire).expect("the handler must accept what the contract encodes");
    assert_eq!(decoded.name, "my-relay");
    assert_eq!(decoded.config.models, vec!["model-a", "model-b"]);
}

#[tokio::test]
async fn a_models_refresh_sweep_speaks_only_the_contracts_vocabulary() {
    // No provider is configured, so this exercises the empty sweep and the
    // response envelope rather than the network.
    let config = Arc::new(RwLock::new(Config::default()));
    let request = JsonRpcRequest::with_id("providers.modelsRefresh", None, json!(1));
    let response = handle_models_refresh(request, config, test_vault()).await;

    let result = response.result.expect("sweep must answer");
    let parsed: ModelsRefreshResult =
        serde_json::from_value(result.clone()).expect("client must decode the sweep");
    assert!(parsed.providers.is_empty());
    assert_eq!(
        object_keys(&result),
        contract_keys(&ModelsRefreshResult::default()),
        "the sweep envelope must be exactly what the contract declares"
    );
}

#[tokio::test]
async fn a_provider_without_a_credential_gets_a_row_rather_than_silence() {
    // Skipping a bad record is usually right; doing it silently is what costs.
    // Asking to refresh one provider and getting an empty array back reads as
    // "nothing happened", which is indistinguishable from success.
    let mut config = Config::default();
    let mut cfg = ProviderConfig::test_config("gpt-4o");
    cfg.enabled = true;
    cfg.api_key = None;
    config.providers.insert("openai".to_string(), cfg);
    let config = Arc::new(RwLock::new(config));

    let request = JsonRpcRequest::with_id(
        "providers.modelsRefresh",
        Some(json!({ "provider": "openai" })),
        json!(1),
    );
    let response = handle_models_refresh(request, config, test_vault()).await;
    let parsed: ModelsRefreshResult = serde_json::from_value(response.result.expect("answer"))
        .expect("client must decode the sweep");

    let row = parsed
        .providers
        .iter()
        .find(|r| r.provider == "openai")
        .expect("the provider we asked about must appear in the answer");
    assert!(!row.ok);
    assert_eq!(row.kind, Some(DiscoveryFailureKind::MissingCredential));
}

/// Ask a `models_refresh` about one provider and read its row.
async fn refresh_one(config: Config, provider: &str) -> ModelsRefreshRow {
    let request = JsonRpcRequest::with_id(
        "providers.modelsRefresh",
        Some(json!({ "provider": provider })),
        json!(1),
    );
    let response =
        handle_models_refresh(request, Arc::new(RwLock::new(config)), test_vault()).await;
    let parsed: ModelsRefreshResult = serde_json::from_value(response.result.expect("answer"))
        .expect("client must decode the sweep");
    parsed
        .providers
        .into_iter()
        .find(|r| r.provider == provider)
        .unwrap_or_else(|| panic!("the sweep must answer about {provider}"))
}

#[tokio::test]
async fn naming_a_disabled_provider_still_gets_an_answer() {
    // The `enabled` filter is the blanket sweep's rule — it exists so a sweep
    // does not dial vendors nobody uses. Applied to a named target it turned
    // "go look at this one" into an empty array, which reads as "nothing
    // happened" and is the one thing the whole per-row design is against.
    let mut config = Config::default();
    let mut cfg = ProviderConfig::test_config("gpt-4o");
    cfg.enabled = false;
    cfg.api_key = None;
    config.providers.insert("openai".to_string(), cfg);

    let row = refresh_one(config, "openai").await;
    assert!(!row.ok);
    // No credential in this fixture, so that is the reason reported — the
    // point of the test is that a row exists at all.
    assert_eq!(row.kind, Some(DiscoveryFailureKind::MissingCredential));
}

#[tokio::test]
async fn a_disabled_provider_stays_out_of_the_blanket_sweep() {
    // The other half of the rule, so the fix above cannot quietly become "the
    // sweep dials everything".
    let mut config = Config::default();
    let mut cfg = ProviderConfig::test_config("gpt-4o");
    cfg.enabled = false;
    config.providers.insert("openai".to_string(), cfg);

    let request = JsonRpcRequest::with_id("providers.modelsRefresh", None, json!(1));
    let response =
        handle_models_refresh(request, Arc::new(RwLock::new(config)), test_vault()).await;
    let parsed: ModelsRefreshResult = serde_json::from_value(response.result.expect("answer"))
        .expect("client must decode the sweep");
    assert!(
        parsed.providers.is_empty(),
        "an un-narrowed sweep must skip disabled providers"
    );
}

#[tokio::test]
async fn naming_an_unconfigured_provider_says_so_rather_than_nothing() {
    // An unlinked preset falls out of the iteration before any of the
    // per-target reasoning can apply. The caller cannot tell that from a sweep
    // that ran and found nothing to say.
    let row = refresh_one(Config::default(), "openai").await;
    assert!(!row.ok);
    assert_eq!(row.kind, Some(DiscoveryFailureKind::MissingCredential));
    assert!(
        row.error.is_some_and(|e| e.contains("not configured")),
        "the reason must distinguish 'not linked' from 'no key'"
    );
}

#[tokio::test]
async fn a_provider_with_no_address_is_unsupported_not_missing_credential() {
    // Two different unprobeable states, and the difference is actionable:
    // "link it first" is something the operator can do, "this has no address"
    // is not. A custom provider with no base_url and no preset behind it is
    // the second kind.
    let mut config = Config::default();
    let mut cfg = ProviderConfig::test_config("some-model");
    cfg.enabled = true;
    cfg.base_url = None;
    cfg.api_key = Some("sk-test".to_string());
    config.providers.insert("my-relay".to_string(), cfg);

    let row = refresh_one(config, "my-relay").await;
    assert!(!row.ok);
    assert_eq!(row.kind, Some(DiscoveryFailureKind::Unsupported));
}
