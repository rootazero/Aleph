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
    /// Build from a `[fetch].backends.crawl4ai` entry. `None` when the entry is
    /// unusable (no/invalid base_url) — caller then skips this provider.
    pub fn from_backend(b: &FetchBackendConfig) -> Option<Self> {
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
        self.inner
            .fetch_markdown(url)
            .await
            .map_err(|e| AlephError::provider(format!("crawl4ai: {e}")))
    }
    fn name(&self) -> &str {
        NAME
    }
    fn is_available(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_builds_from_backend_config_with_base_url() {
        let backend = crate::config::types::FetchBackendConfig {
            provider_type: "crawl4ai".into(),
            api_key: Some("tok".into()),
            base_url: Some("http://10.0.0.1:11235".into()),
            timeout_seconds: Some(45),
            verified: false,
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
        };
        assert!(Crawl4aiFetchProvider::from_backend(&backend).is_none());
    }
}
