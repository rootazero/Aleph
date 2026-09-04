//! Search configuration RPC handlers
//!
//! Provides RPC methods for managing search settings.

mod delete;
// `pub(crate)` so `search::handle`'s live rebuild resolves backend keys by
// the one definition of the vault-key format (`dto::vault_key`) rather than
// a second spelling of "search:<name>".
pub(crate) mod dto;
mod get;
mod test;
mod update;

pub use delete::handle_delete_backend;
pub use dto::{SearchBackendDto, SearchConfigDto};
pub use get::handle_get;
pub use test::{handle_test, SearchTestResult};
pub use update::handle_update;

#[cfg(test)]
mod tests {
    use super::dto::vault_key;
    use super::*;
    use crate::config::Config;
    use crate::gateway::event_bus::GatewayEventBus;
    use crate::gateway::protocol::JsonRpcRequest;
    use crate::gateway::security::{SecurityStore, SharedTokenManager};
    use crate::sync_primitives::Arc;
    use tokio::sync::RwLock;

    fn update_dto_params(backends: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "enabled": true,
            "default_provider": "tavily",
            "max_results": 5,
            "timeout_seconds": 10,
            "pii_enabled": false,
            "pii_scrub_email": false,
            "pii_scrub_phone": false,
            "pii_scrub_ssn": false,
            "pii_scrub_credit_card": false,
            "backends": backends,
        })
    }

    // Security (3def857c6): get reports per-backend `has_api_key` from the vault
    // but never echoes the secret back to the Panel.
    #[tokio::test]
    async fn test_handle_get_reports_has_api_key_without_echoing_secret() {
        let base = Config {
            search: Some(crate::config::types::SearchConfigInternal {
                enabled: true,
                default_provider: "brave".to_string(),
                fallback_providers: None,
                max_results: 5,
                timeout_seconds: 10,
                backends: std::collections::HashMap::from([(
                    "brave".to_string(),
                    crate::config::types::SearchBackendConfig {
                        provider_type: "brave".to_string(),
                        api_key: None,
                        base_url: None,
                        engine_id: None,
                        engines: None,
                        min_request_interval_ms: None,
                        verified: true,
                    },
                )]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let config = Arc::new(RwLock::new(base));
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let vault = Arc::new(SharedTokenManager::new(
            store,
            "/tmp/test_search_haskey.vault",
        ));
        let _ = vault.generate_token();
        vault
            .store_secret(&vault_key("brave"), "super-secret-key")
            .unwrap();

        let request = JsonRpcRequest::with_id("search_config.get", None, serde_json::json!(1));
        let response = handle_get(request, config, vault).await;
        assert!(response.is_success());

        let result = response.result.unwrap();
        let backend = &result["backends"][0];
        assert_eq!(backend["name"].as_str(), Some("brave"));
        assert_eq!(backend["has_api_key"].as_bool(), Some(true));
        assert!(
            backend.get("api_key").is_none(),
            "stored secret must never be echoed back"
        );
        assert!(!result.to_string().contains("super-secret-key"));
    }

    // The SearXNG `engines` pin must survive the RPC boundary in both
    // directions (panel update -> gateway, gateway get -> panel).
    #[test]
    fn search_backend_dto_round_trips_engines() {
        let dto = SearchBackendDto {
            name: "searxng".to_string(),
            api_key: None,
            base_url: Some("http://searxng:8080".to_string()),
            engine_id: None,
            engines: Some("bing".to_string()),
            has_api_key: false,
            verified: false,
        };
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["engines"], "bing");
        let back: SearchBackendDto = serde_json::from_value(json).unwrap();
        assert_eq!(back.engines.as_deref(), Some("bing"));
    }

    // Regression (2026-07-02 incident): saving a searxng backend with the
    // base_url cleared must be rejected, not persisted. The config loader
    // hard-fails on a searxng backend without base_url (validate.rs), so
    // accepting the write bricks every subsequent config load.
    #[tokio::test]
    async fn handle_update_rejects_searxng_without_base_url_and_preserves_entry() {
        // Hold the crate-wide ALEPH_HOME guard for the whole test (replaces the
        // former `aleph_home_env` serial group, which raced the mutex-guarded
        // tests). Single source of ALEPH_HOME mutual exclusion across the crate.
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
            search: Some(crate::config::types::SearchConfigInternal {
                enabled: true,
                default_provider: "tavily".to_string(),
                fallback_providers: None,
                max_results: 5,
                timeout_seconds: 10,
                backends: std::collections::HashMap::from([
                    (
                        "searxng".to_string(),
                        crate::config::types::SearchBackendConfig {
                            provider_type: "searxng".to_string(),
                            api_key: None,
                            base_url: Some("http://10.0.0.1:8008".to_string()),
                            engine_id: None,
                            engines: Some("bing".to_string()),
                            min_request_interval_ms: Some(2000),
                            verified: true,
                        },
                    ),
                    (
                        "tavily".to_string(),
                        crate::config::types::SearchBackendConfig {
                            provider_type: "tavily".to_string(),
                            api_key: None,
                            base_url: Some("https://api.tavily.com".to_string()),
                            engine_id: None,
                            engines: None,
                            min_request_interval_ms: None,
                            verified: true,
                        },
                    ),
                ]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let config = Arc::new(RwLock::new(base));
        let event_bus = Arc::new(GatewayEventBus::new());
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let vault = Arc::new(SharedTokenManager::new(
            store,
            dir.path().join("test.vault").to_string_lossy().to_string(),
        ));
        let _ = vault.generate_token();

        // The incident payload: the Panel echoes searxng with its fields
        // cleared (user emptied the form, or a stale card upserts blindly).
        let params = update_dto_params(serde_json::json!([
            { "name": "tavily", "base_url": "https://api.tavily.com" },
            { "name": "searxng" }
        ]));
        let request =
            JsonRpcRequest::with_id("search_config.update", Some(params), serde_json::json!(1));
        let response = handle_update(request, config.clone(), event_bus, vault).await;
        assert!(
            !response.is_success(),
            "clearing searxng base_url must be rejected: {response:?}"
        );
        let msg = &response.error.as_ref().unwrap().message;
        assert!(
            msg.contains("base_url"),
            "error names the missing field: {msg}"
        );

        // The in-memory entry is untouched — nothing was clobbered before the
        // rejection, so a follow-up get still shows the working config.
        {
            let cfg = config.read().await;
            let search = cfg.search.as_ref().unwrap();
            let sx = &search.backends["searxng"];
            assert_eq!(sx.base_url.as_deref(), Some("http://10.0.0.1:8008"));
            assert_eq!(sx.engines.as_deref(), Some("bing"));
            assert!(sx.verified);
        }

        match prev {
            // SAFETY: restoring previously-read env var while the ALEPH_HOME guard is held.
            Some(v) => unsafe { std::env::set_var("ALEPH_HOME", v) },
            // SAFETY: removing the env var we set above while the ALEPH_HOME guard is held.
            None => unsafe { std::env::remove_var("ALEPH_HOME") },
        }
    }

    // Sibling structural prerequisite: a google backend without engine_id is
    // equally unloadable — reject its creation instead of persisting it.
    #[tokio::test]
    async fn handle_update_rejects_google_without_engine_id() {
        // Same crate-wide ALEPH_HOME guard as the sibling test above.
        let _home_guard = crate::utils::paths::ALEPH_HOME_TEST_GUARD
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let prev = std::env::var("ALEPH_HOME").ok();
        // SAFETY: see above: scoped ALEPH_HOME override under guard, restored below.
        unsafe {
            std::env::set_var("ALEPH_HOME", dir.path());
        }

        let config = Arc::new(RwLock::new(Config::default()));
        let event_bus = Arc::new(GatewayEventBus::new());
        let store = Arc::new(SecurityStore::in_memory().unwrap());
        let vault = Arc::new(SharedTokenManager::new(
            store,
            dir.path().join("test.vault").to_string_lossy().to_string(),
        ));
        let _ = vault.generate_token();

        let params = update_dto_params(serde_json::json!([
            { "name": "google", "base_url": "https://www.googleapis.com/customsearch/v1" }
        ]));
        let request =
            JsonRpcRequest::with_id("search_config.update", Some(params), serde_json::json!(1));
        let response = handle_update(request, config.clone(), event_bus, vault).await;
        assert!(
            !response.is_success(),
            "google without engine_id must be rejected: {response:?}"
        );
        let msg = &response.error.as_ref().unwrap().message;
        assert!(
            msg.contains("engine_id"),
            "error names the missing field: {msg}"
        );

        // Rejection happened before any mutation — no search section was
        // created as a side effect.
        assert!(config.read().await.search.is_none());

        match prev {
            // SAFETY: restoring previously-read env var while the ALEPH_HOME guard is held.
            Some(v) => unsafe { std::env::set_var("ALEPH_HOME", v) },
            // SAFETY: removing the env var we set above while the ALEPH_HOME guard is held.
            None => unsafe { std::env::remove_var("ALEPH_HOME") },
        }
    }
}
