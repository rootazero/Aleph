//! Fetch configuration RPC handlers
//!
//! Provides RPC methods for managing fetch backend settings (URL→markdown providers),
//! parallel to `search_config`. Token storage uses the vault (`fetch:<name>`);
//! the Firecrawl fetch backend shares the `[search]` Firecrawl config and vault key.

use crate::config::types::FetchBackendConfig;
use crate::config::types::FetchConfigInternal;
use crate::config::Config;
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::security::SharedTokenManager;
use crate::sync_primitives::Arc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;
use tracing::error;

use super::normalize_optional_string;

// =============================================================================
// DTOs
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchBackendDto {
    pub name: String,
    pub provider_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    /// Inbound only (never echoed). Stored to vault on update.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default)]
    pub has_api_key: bool,
    #[serde(default)]
    pub verified: bool,
    /// True for providers that reuse the [search] config (firecrawl).
    #[serde(default)]
    pub shares_search: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchConfigDto {
    pub enabled: bool,
    pub default_provider: String,
    #[serde(default)]
    pub backends: Vec<FetchBackendDto>,
}

// =============================================================================
// Vault helpers
// =============================================================================

/// Primary vault key for a fetch backend (non-firecrawl).
fn vault_key(backend_name: &str) -> String {
    format!("fetch:{backend_name}")
}

/// Back-compat legacy vault key (crawl4ai was previously stored under web_fetch:).
fn legacy_vault_key(backend_name: &str) -> String {
    format!("web_fetch:{backend_name}")
}

/// Vault key for the firecrawl search/fetch shared credentials.
fn firecrawl_vault_key() -> &'static str {
    "search:firecrawl"
}

/// Resolve API key for a fetch backend. Checks `fetch:<name>` first, then
/// falls back to `web_fetch:<name>` (legacy crawl4ai key).
fn resolve_fetch_api_key(name: &str, vault: &SharedTokenManager) -> Option<String> {
    super::resolve_vault_secret(&vault_key(name), vault)
        .or_else(|| super::resolve_vault_secret(&legacy_vault_key(name), vault))
}

/// Resolve API key for the firecrawl backend (shared with search).
fn resolve_firecrawl_api_key(vault: &SharedTokenManager) -> Option<String> {
    super::resolve_vault_secret(firecrawl_vault_key(), vault)
}

/// Synthesize a firecrawl fetch backend DTO from the shared `[search]` config
/// (Decision A — firecrawl needs no `[fetch]` backend entry). Returns `None`
/// when search firecrawl is unconfigured (absent or empty base URL). `has_api_key`
/// reflects the shared `search:firecrawl` vault presence; the secret is never echoed.
fn synth_firecrawl_dto(
    search: Option<&crate::config::types::SearchConfigInternal>,
    has_api_key: bool,
) -> Option<FetchBackendDto> {
    let base_url = search?
        .backends
        .get("firecrawl")?
        .base_url
        .clone()
        .filter(|s| !s.is_empty())?;
    Some(FetchBackendDto {
        name: "firecrawl".to_string(),
        provider_type: "firecrawl".to_string(),
        base_url: Some(base_url),
        timeout_seconds: None,
        api_key: None,
        has_api_key,
        verified: false,
        shares_search: true,
    })
}

/// Resolve the firecrawl base URL from the shared `[search]` config (Decision A).
fn firecrawl_base_url_from_search(
    search: Option<&crate::config::types::SearchConfigInternal>,
) -> Option<String> {
    search?
        .backends
        .get("firecrawl")?
        .base_url
        .clone()
        .filter(|s| !s.is_empty())
}

// =============================================================================
// handle_get
// =============================================================================

/// Get fetch configuration
pub async fn handle_get(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    let cfg = config.read().await;

    let mut dto = if let Some(fetch) = &cfg.fetch {
        let backends: Vec<FetchBackendDto> = fetch
            .backends
            .iter()
            .map(|(name, backend)| {
                let (has_api_key, shares_search) = if name == "firecrawl" {
                    (resolve_firecrawl_api_key(&vault).is_some(), true)
                } else {
                    (resolve_fetch_api_key(name, &vault).is_some(), false)
                };
                tracing::debug!(
                    backend = %name,
                    has_key = has_api_key,
                    "fetch_config.get: resolved API key presence"
                );
                // Security: report presence only, never echo the secret.
                FetchBackendDto {
                    name: name.clone(),
                    provider_type: backend.provider_type.clone(),
                    base_url: backend.base_url.clone(),
                    timeout_seconds: backend.timeout_seconds,
                    api_key: None,
                    has_api_key,
                    verified: backend.verified,
                    shares_search,
                }
            })
            .collect();
        FetchConfigDto {
            enabled: fetch.enabled,
            default_provider: fetch.default_provider.clone(),
            backends,
        }
    } else {
        // No fetch config present — return a sensible empty default.
        FetchConfigDto {
            enabled: false,
            default_provider: String::new(),
            backends: Vec::new(),
        }
    };

    // Surface firecrawl availability from the shared [search] config so the Panel
    // can offer it as a default (Strategy V: no [fetch] backend entry is created).
    if !dto.backends.iter().any(|b| b.name == "firecrawl") {
        if let Some(fc) = synth_firecrawl_dto(
            cfg.search.as_ref(),
            resolve_firecrawl_api_key(&vault).is_some(),
        ) {
            dto.backends.push(fc);
        }
    }

    match serde_json::to_value(dto) {
        Ok(v) => JsonRpcResponse::success(request.id, v),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize config: {e}"),
        ),
    }
}

// =============================================================================
// handle_update
// =============================================================================

/// Update fetch configuration
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    let params = match request.params {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params".to_string())
        }
    };

    let dto: FetchConfigDto = match serde_json::from_value(params) {
        Ok(d) => d,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid params: {e}"),
            )
        }
    };

    {
        let mut cfg = config.write().await;

        // Create fetch config if it doesn't exist
        if cfg.fetch.is_none() {
            cfg.fetch = Some(FetchConfigInternal {
                enabled: false,
                default_provider: String::new(),
                fallback_providers: None,
                backends: std::collections::HashMap::new(),
            });
        }

        if let Some(fetch) = &mut cfg.fetch {
            fetch.enabled = dto.enabled;
            fetch.default_provider = dto.default_provider.clone();

            for backend_dto in &dto.backends {
                // Strategy V: firecrawl shares the [search] config and the
                // `search:firecrawl` vault key. It is never persisted as a
                // [fetch] backend entry, nor given a `fetch:` vault key. The
                // Panel already filters firecrawl out of the outbound payload;
                // this guard is server-side defense-in-depth for any future
                // client that does not.
                if backend_dto.name == "firecrawl" {
                    continue;
                }

                // Store API key in vault if provided
                if let Some(ref api_key) = normalize_optional_string(backend_dto.api_key.clone()) {
                    if let Err(e) = vault.store_secret(&vault_key(&backend_dto.name), api_key) {
                        error!(error = %e, "Failed to store fetch API key in vault");
                        return JsonRpcResponse::error(
                            request.id,
                            INTERNAL_ERROR,
                            format!("Failed to store API key: {e}"),
                        );
                    }
                }

                let entry = fetch
                    .backends
                    .entry(backend_dto.name.clone())
                    .or_insert_with(|| FetchBackendConfig {
                        provider_type: backend_dto.provider_type.clone(),
                        api_key: None,
                        base_url: None,
                        timeout_seconds: None,
                        verified: false,
                        enabled: true,
                    });

                entry.provider_type = backend_dto.provider_type.clone();
                // api_key stays None in config — vault is the source
                entry.api_key = None;
                entry.base_url = normalize_optional_string(backend_dto.base_url.clone());
                entry.timeout_seconds = backend_dto.timeout_seconds;
                entry.verified = false; // Config change resets verified
            }
        }

        if let Err(e) = cfg.save_incremental(&["fetch"]).map_err(|e| e.to_string()) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }
    }

    // Broadcast config change event
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("fetch".to_string()),
        value: serde_json::to_value(&dto).unwrap_or(Value::Null),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_gateway_event(&event);

    JsonRpcResponse::success(request.id, serde_json::json!({ "success": true }))
}

// =============================================================================
// handle_test
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchTestResult {
    pub success: bool,
    pub message: String,
}

/// Test a fetch backend connection
pub async fn handle_test(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        /// Backend name (used to persist verified=true on success)
        name: String,
        #[serde(default)]
        provider_type: Option<String>,
        #[serde(default)]
        api_key: Option<String>,
        #[serde(default)]
        base_url: Option<String>,
        #[serde(default)]
        timeout_seconds: Option<u64>,
    }

    let mut params: Params = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Determine provider type from config or fallback to name
    let provider_type = {
        let cfg = config.read().await;
        let from_config = cfg
            .fetch
            .as_ref()
            .and_then(|f| f.backends.get(&params.name))
            .map(|b| b.provider_type.clone());

        // Also pick up base_url from config if not in params
        if params.base_url.is_none() {
            params.base_url = cfg
                .fetch
                .as_ref()
                .and_then(|f| f.backends.get(&params.name))
                .and_then(|b| b.base_url.clone());
        }

        let resolved = from_config
            .or_else(|| params.provider_type.clone())
            .unwrap_or_else(|| params.name.clone());
        // Firecrawl shares the [search] base URL (Decision A); fall back to it
        // when the caller did not supply one.
        if resolved == "firecrawl" && params.base_url.is_none() {
            params.base_url = firecrawl_base_url_from_search(cfg.search.as_ref());
        }
        resolved
    };

    use crate::fetch::providers::{Crawl4aiFetchProvider, FirecrawlFetchProvider};
    use crate::fetch::FetchProvider;

    let test_result: FetchTestResult = match provider_type.as_str() {
        "crawl4ai" => {
            // Resolve token: inline param > fetch:<name> > web_fetch:<name> (back-compat)
            let resolved_key = params
                .api_key
                .clone()
                .or_else(|| resolve_fetch_api_key(&params.name, &vault));

            // Validate base_url upfront: the provider factory collapses both
            // "missing" and "bad scheme" into `None`, which used to surface
            // as a misleading "No base URL configured" for scheme typos.
            let base_url = normalize_optional_string(params.base_url.clone());
            match &base_url {
                None => FetchTestResult {
                    success: false,
                    message: "No base URL configured for crawl4ai".to_string(),
                },
                Some(url)
                    if !url.to_lowercase().starts_with("http://")
                        && !url.to_lowercase().starts_with("https://") =>
                {
                    FetchTestResult {
                        success: false,
                        message: format!(
                            "Base URL must start with http:// or https:// (got \"{url}\")"
                        ),
                    }
                }
                Some(_) => {
                    let backend_cfg = FetchBackendConfig {
                        provider_type: "crawl4ai".to_string(),
                        api_key: resolved_key,
                        base_url,
                        timeout_seconds: params.timeout_seconds,
                        verified: false,
                        enabled: true,
                    };

                    match Crawl4aiFetchProvider::from_backend(&backend_cfg) {
                        Some(provider) => match provider.fetch("https://example.com").await {
                            Ok(_) => FetchTestResult {
                                success: true,
                                message: "Connection successful".to_string(),
                            },
                            Err(e) => FetchTestResult {
                                success: false,
                                message: format!("Fetch failed: {e}"),
                            },
                        },
                        None => FetchTestResult {
                            success: false,
                            message: "No base URL configured for crawl4ai".to_string(),
                        },
                    }
                }
            }
        }
        "firecrawl" => {
            // Firecrawl shares search credentials
            let Some(api_key) = resolve_firecrawl_api_key(&vault) else {
                return JsonRpcResponse::success(
                    request.id,
                    serde_json::json!({"success": false, "message": "API key is required for Firecrawl (configure in Search settings)"}),
                );
            };

            let Some(base_url) = params.base_url.clone() else {
                return JsonRpcResponse::success(
                    request.id,
                    serde_json::json!({"success": false, "message": "Base URL is required for Firecrawl"}),
                );
            };

            match FirecrawlFetchProvider::new(base_url, api_key) {
                Ok(provider) => match provider.fetch("https://example.com").await {
                    Ok(_) => FetchTestResult {
                        success: true,
                        message: "Connection successful".to_string(),
                    },
                    Err(e) => FetchTestResult {
                        success: false,
                        message: format!("Fetch failed: {e}"),
                    },
                },
                Err(e) => FetchTestResult {
                    success: false,
                    message: format!("Failed to create provider: {e}"),
                },
            }
        }
        _ => FetchTestResult {
            success: false,
            message: format!("Unknown provider type: {provider_type}"),
        },
    };

    // Persist verified=true on success
    if test_result.success {
        let mut cfg = config.write().await;
        if let Some(fetch) = &mut cfg.fetch {
            if let Some(backend) = fetch.backends.get_mut(&params.name) {
                backend.verified = true;
                if let Err(e) = cfg.save_incremental(&["fetch"]) {
                    tracing::error!(error = %e, "Failed to save config after fetch test");
                }
            }
        }
    }

    match serde_json::to_value(test_result) {
        Ok(v) => JsonRpcResponse::success(request.id, v),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize result: {e}"),
        ),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_dto_never_serializes_token_and_round_trips() {
        let dto = FetchBackendDto {
            name: "crawl4ai".into(),
            provider_type: "crawl4ai".into(),
            base_url: Some("http://x:11235".into()),
            timeout_seconds: Some(60),
            api_key: None,
            has_api_key: true,
            verified: false,
            shares_search: false,
        };
        let v = serde_json::to_value(&dto).unwrap();
        assert_eq!(v["has_api_key"], true);
        assert!(v.get("api_key").is_none() || v["api_key"].is_null());
        let back: FetchBackendDto = serde_json::from_value(v).unwrap();
        assert_eq!(back.base_url.as_deref(), Some("http://x:11235"));
    }

    fn search_with_firecrawl(base_url: &str) -> crate::config::types::SearchConfigInternal {
        serde_json::from_value(serde_json::json!({
            "enabled": true,
            "default_provider": "firecrawl",
            "backends": { "firecrawl": { "provider_type": "firecrawl", "base_url": base_url } }
        }))
        .unwrap()
    }

    #[test]
    fn synth_firecrawl_dto_present_when_search_configured() {
        let search = search_with_firecrawl("https://api.firecrawl.dev");
        let dto = synth_firecrawl_dto(Some(&search), true).expect("firecrawl available");
        assert_eq!(dto.name, "firecrawl");
        assert_eq!(dto.provider_type, "firecrawl");
        assert!(dto.shares_search);
        assert!(dto.has_api_key);
        assert_eq!(dto.base_url.as_deref(), Some("https://api.firecrawl.dev"));
        assert!(dto.api_key.is_none(), "never echo a secret");
    }

    #[test]
    fn synth_firecrawl_dto_absent_without_search() {
        assert!(synth_firecrawl_dto(None, false).is_none());
        let empty: crate::config::types::SearchConfigInternal =
            serde_json::from_value(serde_json::json!({ "backends": {} })).unwrap();
        assert!(synth_firecrawl_dto(Some(&empty), true).is_none());
        let blank = search_with_firecrawl("");
        assert!(
            synth_firecrawl_dto(Some(&blank), true).is_none(),
            "empty base_url → unavailable"
        );
    }

    #[test]
    fn firecrawl_base_url_from_search_resolves() {
        let search = search_with_firecrawl("https://api.firecrawl.dev");
        assert_eq!(
            firecrawl_base_url_from_search(Some(&search)).as_deref(),
            Some("https://api.firecrawl.dev")
        );
        assert!(firecrawl_base_url_from_search(None).is_none());
    }

    fn test_vault(dir: &tempfile::TempDir) -> Arc<SharedTokenManager> {
        let store = Arc::new(crate::gateway::security::SecurityStore::in_memory().unwrap());
        let vault = Arc::new(SharedTokenManager::new(
            store,
            dir.path().join("test.vault").to_string_lossy().to_string(),
        ));
        let _ = vault.generate_token();
        vault
    }

    // A base_url without an http(s) scheme must produce a scheme error, not
    // the misleading "No base URL configured" (the provider factory collapses
    // both cases into `None`; the handler must distinguish them upfront).
    #[tokio::test]
    async fn handle_test_crawl4ai_rejects_schemeless_base_url_with_clear_message() {
        let dir = tempfile::tempdir().unwrap();
        let config = Arc::new(RwLock::new(Config::default()));
        let vault = test_vault(&dir);

        let params = serde_json::json!({
            "name": "crawl4ai",
            "provider_type": "crawl4ai",
            "base_url": "localhost:11235"
        });
        let request =
            JsonRpcRequest::with_id("fetch_config.test", Some(params), serde_json::json!(1));
        let response = handle_test(request, config, vault).await;
        let result: FetchTestResult =
            serde_json::from_value(response.result.expect("result")).unwrap();
        assert!(!result.success);
        assert!(
            result.message.contains("http://"),
            "message must explain the scheme requirement, got: {}",
            result.message
        );
    }

    // No base_url anywhere (params or config) → the original "not configured"
    // message. Whitespace-only input counts as missing.
    #[tokio::test]
    async fn handle_test_crawl4ai_reports_missing_base_url() {
        let dir = tempfile::tempdir().unwrap();
        let config = Arc::new(RwLock::new(Config::default()));
        let vault = test_vault(&dir);

        let params = serde_json::json!({
            "name": "crawl4ai",
            "provider_type": "crawl4ai",
            "base_url": "   "
        });
        let request =
            JsonRpcRequest::with_id("fetch_config.test", Some(params), serde_json::json!(1));
        let response = handle_test(request, config, vault).await;
        let result: FetchTestResult =
            serde_json::from_value(response.result.expect("result")).unwrap();
        assert!(!result.success);
        assert_eq!(result.message, "No base URL configured for crawl4ai");
    }

    // Strategy V defense-in-depth: even if a client includes a firecrawl entry
    // in the update payload, handle_update must NOT persist it as a [fetch]
    // backend nor write a fetch:firecrawl vault key (firecrawl shares [search]).
    // Other backends (crawl4ai) and default_provider still round-trip.
    #[tokio::test]
    async fn handle_update_never_persists_firecrawl_backend() {
        // Single-source ALEPH_HOME isolation: hold the crate-wide guard for the
        // whole test so no concurrently-running test's override (or dropped
        // tempdir) is observed during the config save. Replaces the former
        // `aleph_home_env` serial group, which did NOT exclude the tests that
        // guard ALEPH_HOME via ALEPH_HOME_TEST_GUARD.
        let _home_guard = crate::utils::paths::ALEPH_HOME_TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let prev = std::env::var("ALEPH_HOME").ok();
        // SAFETY: test holds the ALEPH_HOME guard; scoped env override, restored below.
        unsafe {
            std::env::set_var("ALEPH_HOME", dir.path());
        }

        let base = Config {
            fetch: Some(FetchConfigInternal {
                enabled: false,
                default_provider: String::new(),
                fallback_providers: None,
                backends: std::collections::HashMap::new(),
            }),
            ..Default::default()
        };
        let config = Arc::new(RwLock::new(base));
        let event_bus = Arc::new(GatewayEventBus::new());
        let store = Arc::new(crate::gateway::security::SecurityStore::in_memory().unwrap());
        let vault = Arc::new(SharedTokenManager::new(
            store,
            dir.path().join("test.vault").to_string_lossy().to_string(),
        ));
        let _ = vault.generate_token();

        // A hostile/legacy client sends firecrawl alongside crawl4ai.
        let params = serde_json::json!({
            "enabled": true,
            "default_provider": "firecrawl",
            "backends": [
                { "name": "crawl4ai", "provider_type": "crawl4ai", "base_url": "http://x:11235" },
                { "name": "firecrawl", "provider_type": "firecrawl", "base_url": "http://evil", "api_key": "leak-me" }
            ]
        });
        let request =
            JsonRpcRequest::with_id("fetch_config.update", Some(params), serde_json::json!(1));
        let response = handle_update(request, config.clone(), event_bus, vault.clone()).await;
        assert!(response.is_success(), "update should succeed: {response:?}");

        {
            let cfg = config.read().await;
            let fetch = cfg.fetch.as_ref().unwrap();
            assert_eq!(
                fetch.default_provider, "firecrawl",
                "default_provider round-trips even without a firecrawl backend entry"
            );
            assert!(
                fetch.backends.contains_key("crawl4ai"),
                "crawl4ai persisted"
            );
            assert!(
                !fetch.backends.contains_key("firecrawl"),
                "Strategy V: firecrawl must never be persisted as a [fetch] backend"
            );
        }
        // The inline api_key must have been dropped — no fetch:firecrawl key.
        assert!(
            resolve_fetch_api_key("firecrawl", &vault).is_none(),
            "no fetch:firecrawl vault key may be written"
        );

        match prev {
            // SAFETY: restoring previously-read env var while the ALEPH_HOME guard is held.
            Some(v) => unsafe { std::env::set_var("ALEPH_HOME", v) },
            // SAFETY: removing the env var we set above while the ALEPH_HOME guard is held.
            None => unsafe { std::env::remove_var("ALEPH_HOME") },
        }
    }
}
