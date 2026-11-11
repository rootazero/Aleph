use crate::config::types::FetchBackendConfig;
use crate::config::Crawl4aiConfig;
use crate::error::{AlephError, Result};
use crate::fetch::FetchProvider;
use async_trait::async_trait;

const NAME: &str = "crawl4ai";

/// Fetch provider backed by the existing crawl4ai HTTP client.
pub struct Crawl4aiFetchProvider {
    inner: crate::builtin_tools::crawl4ai::Crawl4aiBackend,
}

impl Crawl4aiFetchProvider {
    /// Build from a `[fetch].backends.crawl4ai` entry. `None` when the entry
    /// is unusable (disabled, or missing/invalid base_url).
    pub fn from_backend(b: &FetchBackendConfig) -> Option<Self> {
        if !b.enabled {
            return None;
        }
        let cfg = Crawl4aiConfig {
            enabled: true,
            base_url: b.base_url.clone().unwrap_or_default(),
            timeout_seconds: b.timeout_seconds.unwrap_or(60),
            token: b.api_key.clone(),
        };
        crate::builtin_tools::crawl4ai::Crawl4aiBackend::from_config(&cfg)
            .map(|inner| Self { inner })
    }
}

#[async_trait]
impl FetchProvider for Crawl4aiFetchProvider {
    async fn fetch(&self, url: &str) -> Result<String> {
        // SSRF contract: the caller is responsible for validating `url`
        // against the operator's SsrfPolicy. The only production caller is
        // the connection-test RPC (`fetch_config.test`), which passes a
        // hardcoded https://example.com. See `FetchProvider::fetch`.
        self.inner.fetch_markdown(url).await.map_err(|e| {
            let message = format!("crawl4ai: {e}");
            match e {
                crate::builtin_tools::error::ToolError::Network(_) => AlephError::network(message),
                _ => AlephError::provider(message),
            }
        })
    }
    fn name(&self) -> &str {
        NAME
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn transport_errors_are_network_but_upstream_errors_are_provider_errors() {
        use std::time::Duration;
        use wiremock::{matchers::any, Mock, MockServer, ResponseTemplate};

        let cases = [
            (ResponseTemplate::new(503), false),
            (
                ResponseTemplate::new(200).set_body_string("not JSON"),
                false,
            ),
            (
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"success": false})),
                false,
            ),
            (
                ResponseTemplate::new(200).set_delay(Duration::from_secs(2)),
                true,
            ),
        ];
        for (response, network) in cases {
            let server = MockServer::start().await;
            Mock::given(any())
                .respond_with(response)
                .mount(&server)
                .await;
            let backend = FetchBackendConfig {
                provider_type: "crawl4ai".into(),
                api_key: Some("super-secret-token".into()),
                base_url: Some(format!("{}/connection/super-secret-token", server.uri())),
                timeout_seconds: Some(1),
                verified: false,
                enabled: true,
            };
            let provider = Crawl4aiFetchProvider::from_backend(&backend).unwrap();
            let err = provider.fetch("https://example.com").await.unwrap_err();
            if network {
                assert!(matches!(&err, AlephError::NetworkError { .. }), "{err}");
            } else {
                assert!(matches!(&err, AlephError::ProviderError { .. }), "{err}");
            }
            let message = err.to_string();
            assert!(message.contains("crawl4ai:"), "{message}");
            assert!(!message.contains("super-secret-token"), "{message}");
        }
    }

    #[test]
    fn factory_builds_from_backend_config_with_base_url() {
        let backend = crate::config::types::FetchBackendConfig {
            provider_type: "crawl4ai".into(),
            api_key: Some("tok".into()),
            base_url: Some("http://10.0.0.1:11235".into()),
            timeout_seconds: Some(45),
            verified: false,
            enabled: true,
        };
        let p = Crawl4aiFetchProvider::from_backend(&backend);
        assert!(p.is_some());
        assert_eq!(p.unwrap().name(), "crawl4ai");
    }

    #[test]
    fn factory_returns_none_without_base_url() {
        let backend = crate::config::types::FetchBackendConfig {
            provider_type: "crawl4ai".into(),
            api_key: None,
            base_url: None,
            timeout_seconds: None,
            verified: false,
            enabled: true,
        };
        assert!(Crawl4aiFetchProvider::from_backend(&backend).is_none());
    }

    #[test]
    fn factory_returns_none_when_disabled() {
        let backend = crate::config::types::FetchBackendConfig {
            provider_type: "crawl4ai".into(),
            api_key: Some("tok".into()),
            base_url: Some("http://10.0.0.1:11235".into()),
            timeout_seconds: Some(45),
            verified: false,
            enabled: false,
        };
        assert!(Crawl4aiFetchProvider::from_backend(&backend).is_none());
    }
}
