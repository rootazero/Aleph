use crate::error::{AlephError, Result};
use crate::search::providers::base::{build_client, parse_json, retain_usable, send};
use crate::search::{SearchOptions, SearchProvider, SearchResult};
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;

/// Jina AI search provider (`s.jina.ai`)
///
/// Free credits on signup (1M tokens). Returns LLM-ready snippets that are
/// already de-deduplicated and ranked. The endpoint requires `Authorization:
/// Bearer <api_key>` and a couple of opt-in headers to keep payloads small
/// (we don't need full-page markdown for ranking, just snippets).
const NAME: &str = "jina";
const ENDPOINT: &str = "https://s.jina.ai/";

#[derive(Debug)]
pub struct JinaProvider {
    api_key: String,
    client: Client,
}

#[derive(Deserialize)]
struct JinaResponse {
    /// `data` is `null` on error envelopes — keep it Option to avoid hard
    /// failure on Jina's quota / auth error JSON.
    #[serde(default)]
    data: Option<Vec<JinaResult>>,
    /// Jina's envelope carries an HTTP-shaped status code (`429`, `401`, ...)
    /// even on a 200 OK transport response. Structured, unlike `message`, so
    /// it is the field the error classification reads.
    #[serde(default)]
    code: Option<u16>,
    #[serde(default)]
    message: Option<String>,
}

/// Every field is optional on the wire.
///
/// Not politeness: serde does not degrade field by field, so a single item a
/// vendor returned with a `null` title used to make the **whole** document
/// fail to deserialize — the backend reported a parse error and the chain
/// moved on as if it were down. `base::retain_usable` decides afterwards what
/// is usable (a url), which is one filter instead of one per provider.
#[derive(Deserialize)]
struct JinaResult {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

impl JinaProvider {
    pub fn new(api_key: impl Into<String>) -> Result<Self> {
        let api_key = api_key.into();
        if api_key.is_empty() {
            return Err(AlephError::invalid_config("Jina API key is required"));
        }
        Ok(Self {
            api_key,
            client: build_client()?,
        })
    }
}

#[async_trait]
impl SearchProvider for JinaProvider {
    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>> {
        // s.jina.ai takes the query as the path segment — let reqwest's
        // query() handle percent-encoding via a single ?q= parameter is also
        // accepted by the API and avoids manual encoding.
        let secret = Some(self.api_key.as_str());
        let response = send(
            self.client
                .get(ENDPOINT)
                .header("Accept", "application/json")
                .header("Authorization", format!("Bearer {}", self.api_key))
                // `no-content` skips full-page markdown extraction — we only
                // need title/url/snippet for LLM ranking and the no-content
                // mode is dramatically cheaper on Jina credits.
                .header("X-Respond-With", "no-content")
                .query(&[("q", query)])
                .timeout(std::time::Duration::from_secs(options.validated_timeout())),
            NAME,
            secret,
        )
        .await?;
        let jina_response: JinaResponse = parse_json(response, NAME, secret).await?;

        let data = jina_response.data.unwrap_or_default();
        if data.is_empty() {
            // Surface Jina's own error message when present — quota /
            // rate-limit envelopes return `data: null` with a `message`
            // field. Treating these as `Ok(vec![])` would silently waste
            // LLM iterations exactly like the SearXNG dead-engine case.
            if let Some(msg) = jina_response.message {
                // The envelope's `code` is structured data, so the kind can
                // come from it rather than from sniffing the free-text
                // message: a quota error reads as quota, not as a generic
                // transient failure that "retry later" undersells (a free
                // tier's lever is the plan, not the clock).
                return Err(match jina_response.code {
                    Some(429) => {
                        AlephError::rate_limit(format!("Jina returned 0 results — {msg}"))
                    }
                    Some(401 | 403) => {
                        AlephError::authentication(NAME, format!("Jina returned 0 results — {msg}"))
                    }
                    _ => AlephError::provider(format!("Jina returned 0 results — {msg}")),
                });
            }
            return Ok(Vec::new());
        }

        let results = data
            .into_iter()
            .take(options.validated_max_results())
            .map(|r| SearchResult {
                title: r.title.unwrap_or_default(),
                url: r.url.unwrap_or_default(),
                snippet: r.description.unwrap_or_default(),
                relevance_score: None,
                full_content: None,
                published_date: None,
                provider: Some(NAME.to_string()),
            })
            .collect();

        Ok(retain_usable(NAME, results))
    }

    fn name(&self) -> &str {
        NAME
    }

    fn is_available(&self) -> bool {
        !self.api_key.is_empty()
    }
}

/// Factory entry for the search provider registry.
///
/// Co-located with the concrete provider so adding a new search
/// backend is a single-file change (provider + factory) plus one
/// registration line in `ProviderFactoryRegistry::with_defaults`.
pub struct JinaFactory;

impl crate::search::ProviderFactory for JinaFactory {
    fn provider_type(&self) -> &'static str {
        NAME
    }
    fn build(
        &self,
        name: &str,
        backend: &crate::config::types::SearchBackendConfig,
        // No operator-supplied upstream URL on this provider — its endpoint is
        // hardcoded, so there is nothing for the SSRF switch to admit.
        _allow_private_network: bool,
    ) -> crate::error::Result<Option<crate::sync_primitives::Arc<dyn crate::search::SearchProvider>>>
    {
        let Some(key) = backend.api_key.as_deref().filter(|s| !s.is_empty()) else {
            log::warn!("search backend '{name}' ({NAME}) skipped: no api_key in vault");
            return Ok(None);
        };
        match JinaProvider::new(key.to_string()) {
            Ok(p) => Ok(Some(crate::sync_primitives::Arc::new(p))),
            Err(e) => {
                log::warn!("search backend '{name}' ({NAME}) construct failed: {e}");
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jina_provider_creation_requires_key() {
        let provider = JinaProvider::new("jina_test_key").unwrap();
        assert_eq!(provider.name(), "jina");
        assert!(provider.is_available());
    }

    #[test]
    fn jina_provider_rejects_empty_key() {
        assert!(JinaProvider::new("").is_err());
    }

    /// Success envelope round-trips through the structs with the fields we
    /// actually care about (title/url/description) and ignores everything
    /// else Jina might add later.
    #[test]
    fn jina_response_parses_success_envelope() {
        let body = r#"{
            "code": 200,
            "status": 20000,
            "data": [
                {"title": "Foo", "url": "https://foo.test", "description": "snippet here", "extra": "ignored"},
                {"title": "Bar", "url": "https://bar.test"}
            ]
        }"#;
        let parsed: JinaResponse = serde_json::from_str(body).expect("parses");
        let data = parsed.data.expect("data present");
        assert_eq!(data.len(), 2);
        assert_eq!(data[0].title.as_deref(), Some("Foo"));
        assert_eq!(data[0].description.as_deref(), Some("snippet here"));
        assert!(data[1].description.is_none());
    }

    /// Quota / rate-limit error envelope: `data: null` plus a `message`.
    /// We need both `None` data AND access to the message so the caller
    /// can promote it to a typed error.
    #[test]
    fn jina_response_parses_error_envelope() {
        let body = r#"{
            "data": null,
            "code": 429,
            "name": "RateLimitError",
            "status": 42900,
            "message": "Rate limit exceeded for your free tier"
        }"#;
        let parsed: JinaResponse = serde_json::from_str(body).expect("parses");
        assert!(parsed.data.is_none());
        assert_eq!(parsed.code, Some(429));
        assert_eq!(
            parsed.message.as_deref(),
            Some("Rate limit exceeded for your free tier")
        );
    }
}
