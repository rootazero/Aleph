use crate::error::{AlephError, Result};
use crate::search::providers::base::{build_client, check_status, parse_json};
use crate::search::{SearchOptions, SearchProvider, SearchResult};
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

/// SearXNG search provider
///
/// SearXNG is a privacy-first, self-hosted metasearch engine
const NAME: &str = "searxng";

#[derive(Debug)]
pub struct SearxngProvider {
    base_url: String,
    client: Client,
}

#[derive(Deserialize)]
struct SearxngResponse {
    #[serde(default)]
    results: Vec<SearxngResult>,
    /// Engines that failed for this query — SearXNG returns an empty `results`
    /// array when every engine is suspended/CAPTCHA-blocked, which we surface
    /// as an error so the LLM doesn't keep trying new queries against a dead
    /// backend. Shape: `[["engine_name", "reason"], ...]`.
    #[serde(default)]
    unresponsive_engines: Vec<(String, String)>,
}

#[derive(Deserialize)]
struct SearxngResult {
    title: String,
    url: String,
    #[serde(default)]
    content: Option<String>,
}

impl SearxngProvider {
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        let base_url = base_url.into();
        if base_url.is_empty() {
            return Err(AlephError::invalid_config("SearXNG base URL is required"));
        }

        let trimmed = base_url.trim_end_matches('/').to_string();
        let scheme_lower = trimmed.to_lowercase();
        if !scheme_lower.starts_with("http://") && !scheme_lower.starts_with("https://") {
            return Err(AlephError::invalid_config(
                "SearXNG base URL must use http:// or https:// scheme",
            ));
        }

        Ok(Self {
            base_url: trimmed,
            client: build_client()?,
        })
    }
}

#[async_trait]
impl SearchProvider for SearxngProvider {
    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>> {
        let url = format!("{}/search", self.base_url);

        let mut params: Vec<(&str, String)> = vec![
            ("q", query.to_string()),
            ("format", "json".to_string()),
            ("count", options.validated_max_results().to_string()),
            ("safesearch", options.searxng_safesearch().to_string()),
        ];
        if let Some(lang) = options.language.as_deref() {
            params.push(("language", lang.to_string()));
        }
        if let Some(range) = options.searxng_time_range() {
            params.push(("time_range", range.to_string()));
        }

        let response = self
            .client
            .get(&url)
            .query(&params)
            .timeout(std::time::Duration::from_secs(options.validated_timeout()))
            .send()
            .await
            .map_err(|e| AlephError::network(e.to_string()))?;

        let response = check_status(response, NAME)?;
        let searxng_response: SearxngResponse = parse_json(response, NAME).await?;

        // SearXNG returns `200 OK` with `"results": []` even when every
        // backend engine is suspended/CAPTCHA-blocked. Silently returning
        // `Ok(vec![])` causes the calling LLM to assume "no results for
        // this query" and burn its iteration budget on new keywords against
        // a broken backend. Promote this to a typed error so the LLM gets
        // actionable signal ("the search engine itself is broken — switch
        // providers or stop").
        if searxng_response.results.is_empty() && !searxng_response.unresponsive_engines.is_empty()
        {
            let detail = searxng_response
                .unresponsive_engines
                .iter()
                .map(|(name, reason)| format!("{name}: {reason}"))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(AlephError::provider(format!(
                "SearXNG returned 0 results — all engines unresponsive ({detail}). \
                 Check the SearXNG instance — its engines are rate-limited / CAPTCHA-blocked."
            )));
        }

        let results = searxng_response
            .results
            .into_iter()
            .take(options.validated_max_results())
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.content.unwrap_or_default(),
                relevance_score: None,
                full_content: None,
                provider: Some(NAME.to_string()),
            })
            .collect();

        Ok(results)
    }

    fn name(&self) -> &str {
        NAME
    }

    fn is_available(&self) -> bool {
        !self.base_url.is_empty()
    }
}


/// Factory entry for the search provider registry.
///
/// Co-located with the concrete provider so adding a new search
/// backend is a single-file change (provider + factory) plus one
/// registration line in `ProviderFactoryRegistry::with_defaults`.
pub struct SearxngFactory;

impl crate::search::ProviderFactory for SearxngFactory {
    fn provider_type(&self) -> &'static str {
        NAME
    }
    fn build(
        &self,
        name: &str,
        backend: &crate::config::types::SearchBackendConfig,
    ) -> crate::error::Result<Option<crate::sync_primitives::Arc<dyn crate::search::SearchProvider>>> {
        let Some(base) = backend.base_url.as_deref().filter(|s| !s.is_empty()) else {
            log::warn!("search backend '{name}' ({}) skipped: base_url missing", NAME);
            return Ok(None);
        };
        match SearxngProvider::new(base.to_string()) {
            Ok(p) => Ok(Some(crate::sync_primitives::Arc::new(p))),
            Err(e) => {
                log::warn!("search backend '{name}' ({}) construct failed: {e}", NAME);
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_searxng_provider_creation() {
        let provider = SearxngProvider::new("http://localhost:8080".to_string()).unwrap();
        assert_eq!(provider.name(), "searxng");
        assert!(provider.is_available());
    }

    #[test]
    fn test_searxng_provider_rejects_empty_url() {
        let result = SearxngProvider::new("".to_string());
        assert!(result.is_err());
    }

    #[test]
    fn test_searxng_provider_trims_trailing_slash() {
        let provider = SearxngProvider::new("http://localhost:8080/".to_string()).unwrap();
        assert_eq!(provider.base_url, "http://localhost:8080");
    }

    #[test]
    fn test_searxng_provider_rejects_invalid_scheme() {
        let result = SearxngProvider::new("ftp://localhost:8080".to_string());
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("must use http:// or https://"));
    }

    #[test]
    fn test_searxng_provider_accepts_https() {
        let provider = SearxngProvider::new("https://searx.example.com".to_string()).unwrap();
        assert_eq!(provider.base_url, "https://searx.example.com");
    }

    /// Empty `results` with non-empty `unresponsive_engines` must surface as
    /// a provider error, not `Ok(vec![])`. Verifies the JSON shape we expect
    /// from SearXNG — `[["name","reason"], ...]` — round-trips through the
    /// `Vec<(String, String)>` deserializer.
    #[test]
    fn searxng_response_parses_unresponsive_engines() {
        let body = r#"{
            "query": "x",
            "number_of_results": 0,
            "results": [],
            "unresponsive_engines": [
                ["brave", "Suspended: too many requests"],
                ["duckduckgo", "CAPTCHA"]
            ]
        }"#;
        let parsed: SearxngResponse = serde_json::from_str(body).expect("parses");
        assert!(parsed.results.is_empty());
        assert_eq!(parsed.unresponsive_engines.len(), 2);
        assert_eq!(parsed.unresponsive_engines[0].0, "brave");
        assert_eq!(parsed.unresponsive_engines[1].1, "CAPTCHA");
    }

    /// When the body has no `unresponsive_engines` key at all (older SearXNG
    /// versions, or a genuinely empty result), `#[serde(default)]` must give
    /// us an empty Vec — otherwise the "treat 0+failures as error" path
    /// would falsely fire on healthy backends.
    #[test]
    fn searxng_response_missing_unresponsive_engines_defaults_empty() {
        let body = r#"{"query":"x","results":[]}"#;
        let parsed: SearxngResponse = serde_json::from_str(body).expect("parses");
        assert!(parsed.results.is_empty());
        assert!(parsed.unresponsive_engines.is_empty());
    }
}
