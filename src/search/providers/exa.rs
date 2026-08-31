use crate::error::{AlephError, Result};
use crate::search::providers::base::{build_client, parse_json, retain_usable, send};
use crate::search::{SearchCapabilities, SearchOptions, SearchProvider, SearchResult};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

/// Exa.ai (formerly Metaphor) search provider
///
/// Exa provides semantic search capabilities
const NAME: &str = "exa";

#[derive(Debug)]
pub struct ExaProvider {
    api_key: String,
    client: Client,
}

#[derive(Serialize)]
struct ExaRequest {
    query: String,
    #[serde(rename = "numResults")]
    num_results: usize,
    contents: ExaContents,
    #[serde(rename = "includeDomains", skip_serializing_if = "Vec::is_empty")]
    include_domains: Vec<String>,
    #[serde(rename = "excludeDomains", skip_serializing_if = "Vec::is_empty")]
    exclude_domains: Vec<String>,
}

#[derive(Serialize)]
struct ExaContents {
    text: ExaText,
}

/// Exa's `contents.text` is `oneOf { boolean, object }` — the object form
/// takes `maxCharacters` (documented as an integer in `1..=10000`).
///
/// Untagged so each variant serialises as the wire spells it: `true`, or
/// `{"maxCharacters": N}`.
#[derive(Serialize)]
#[serde(untagged)]
enum ExaText {
    /// Always constructed as `true`. `false` would mean *no* text, and
    /// `contents.text` is Exa's only text source — there is no snippet field
    /// to fall back to, so a result without it is a title and a url.
    Whole(bool),
    /// Return at most this much of the page. The saving is Exa's: without it
    /// Exa retrieves (and bills for) a whole page per result while the caller
    /// keeps a paragraph.
    Capped {
        #[serde(rename = "maxCharacters")]
        max_characters: usize,
    },
}

/// The largest `maxCharacters` Exa documents.
///
/// Clamped rather than trusted: the value arrives from a caller's snippet
/// budget, and a budget above this ceiling would turn a request that works
/// into a 400 — the one failure mode worth spending a `clamp` to rule out,
/// because it would take the backend out on every search rather than degrade
/// one answer.
const MAX_CHARACTERS_CEILING: usize = 10_000;

#[derive(Deserialize)]
struct ExaResponse {
    #[serde(default)]
    results: Vec<ExaResult>,
}

/// Every field is optional on the wire.
///
/// Not politeness: serde does not degrade field by field, so a single item a
/// vendor returned with a `null` title used to make the **whole** document
/// fail to deserialize — the backend reported a parse error and the chain
/// moved on as if it were down. `base::retain_usable` decides afterwards what
/// is usable (a url), which is one filter instead of one per provider.
#[derive(Deserialize)]
struct ExaResult {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    text: Option<String>,
    /// ISO-8601, as Exa spells it. Absent for pages it has no date for.
    #[serde(default, rename = "publishedDate")]
    published_date: Option<String>,
}

impl ExaProvider {
    pub fn new(api_key: impl Into<String>) -> Result<Self> {
        let api_key = api_key.into();
        if api_key.is_empty() {
            return Err(AlephError::invalid_config("Exa API key is required"));
        }

        Ok(Self {
            api_key,
            client: build_client()?,
        })
    }

    /// Build the request body. Split out of `search` so the wire shape can be
    /// asserted without an HTTP round trip — the parameter names are a
    /// contract with Exa and "it looked right" is how the fill_form /
    /// wait_for key mismatches in the browser layer shipped.
    fn build_request(query: &str, options: &SearchOptions) -> ExaRequest {
        ExaRequest {
            query: query.to_string(),
            num_results: options.validated_max_results(),
            contents: ExaContents {
                text: Self::text_request(options),
            },
            include_domains: options.include_domains.clone(),
            exclude_domains: options.exclude_domains.clone(),
        }
    }

    /// How much page text to ask Exa for.
    ///
    /// Exa has no snippet field: `contents.text` is a whole page body, and it
    /// used to be requested in full on every search. A caller that did not ask
    /// for `full_content` keeps one paragraph of that and discards the rest —
    /// paid for, transferred, and thrown away, once per result.
    fn text_request(options: &SearchOptions) -> ExaText {
        // Asked for bodies: ask for the whole body. The caller's snippet
        // budget is about snippets, and the body budget on the other side
        // (20 000 chars) is above what this field accepts anyway, so capping
        // here could only shorten something the caller explicitly wanted.
        if options.include_full_content {
            return ExaText::Whole(true);
        }
        options.snippet_budget_chars.map_or(
            // No budget declared — every caller but the tool face. Unchanged
            // behaviour: a caller that has not said what it keeps has not
            // given us the right to shorten the answer.
            ExaText::Whole(true),
            |budget| ExaText::Capped {
                max_characters: budget.clamp(1, MAX_CHARACTERS_CEILING),
            },
        )
    }
}

#[async_trait]
impl SearchProvider for ExaProvider {
    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>> {
        let request_body = Self::build_request(query, options);

        let secret = Some(self.api_key.as_str());
        let response = send(
            self.client
                .post("https://api.exa.ai/search")
                .header("x-api-key", &self.api_key)
                .json(&request_body)
                .timeout(std::time::Duration::from_secs(options.validated_timeout())),
            NAME,
            secret,
        )
        .await?;
        let exa_response: ExaResponse = parse_json(response, NAME, secret).await?;
        Ok(retain_usable(
            NAME,
            Self::map_response(exa_response, options),
        ))
    }

    fn name(&self) -> &str {
        NAME
    }

    fn is_available(&self) -> bool {
        !self.api_key.is_empty()
    }

    fn capabilities(&self, _options: &SearchOptions) -> SearchCapabilities {
        SearchCapabilities {
            domain_filter: true, // includeDomains / excludeDomains
            recency: false,      // ExaRequest has no freshness field
            // `contents.text` is always requested (it is Exa's only text
            // source) and reaches `full_content` when the caller asks for
            // bodies — uncapped in that case, which is what makes this bit
            // true rather than "true but truncated". The bit was `false`
            // while the body was being dropped
            // on the floor, which hid Exa from every `full_content` request
            // it could in fact have answered.
            full_content: true,
        }
    }
}

impl ExaProvider {
    /// Map a parsed response onto `SearchResult`s. Split out of `search` so
    /// the content routing can be asserted without an HTTP round trip — it is
    /// the half that was wrong, and it was wrong in a way every test passed.
    fn map_response(response: ExaResponse, options: &SearchOptions) -> Vec<SearchResult> {
        response
            .results
            .into_iter()
            .take(options.validated_max_results())
            .map(|r| {
                // `contents.text` is a page **body**, not a summary — Exa has
                // no separate snippet field. It used to land in `snippet`
                // wholesale, so every result over the tool face's snippet
                // bound tripped the clamp and the answer carried a note
                // telling the reader to `web_fetch` the url for the full page
                // — a page this call had already downloaded and thrown away.
                // Routed by what was asked for, mirroring `firecrawl.rs`.
                let text = r.text.unwrap_or_default();
                let full_content = options.include_full_content.then(|| text.clone());
                SearchResult {
                    title: r.title.unwrap_or_default(),
                    url: r.url.unwrap_or_default(),
                    snippet: text,
                    relevance_score: None,
                    full_content,
                    published_date: r.published_date,
                    provider: Some(NAME.to_string()),
                }
            })
            .collect()
    }
}

/// Factory entry for the search provider registry.
///
/// Co-located with the concrete provider so adding a new search
/// backend is a single-file change (provider + factory) plus one
/// registration line in `ProviderFactoryRegistry::with_defaults`.
pub struct ExaFactory;

impl crate::search::ProviderFactory for ExaFactory {
    fn provider_type(&self) -> &'static str {
        NAME
    }
    fn build(
        &self,
        name: &str,
        backend: &crate::config::types::SearchBackendConfig,
    ) -> crate::error::Result<Option<crate::sync_primitives::Arc<dyn crate::search::SearchProvider>>>
    {
        let Some(key) = backend.api_key.as_deref().filter(|s| !s.is_empty()) else {
            log::warn!("search backend '{name}' ({NAME}) skipped: no api_key in vault");
            return Ok(None);
        };
        match ExaProvider::new(key.to_string()) {
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
    fn test_exa_provider_creation() {
        let provider = ExaProvider::new("exa_test_key".to_string()).unwrap();
        assert_eq!(provider.name(), "exa");
        assert!(provider.is_available());
    }

    #[test]
    fn test_exa_provider_rejects_empty_key() {
        let result = ExaProvider::new("".to_string());
        assert!(result.is_err());
    }

    #[test]
    fn domain_lists_reach_the_exa_request_body() {
        let o = SearchOptions {
            include_domains: vec!["github.com".into()],
            exclude_domains: vec!["pinterest.com".into()],
            ..Default::default()
        };
        let body = ExaProvider::build_request("q", &o);
        let v = serde_json::to_value(&body).unwrap();
        assert_eq!(v["includeDomains"], serde_json::json!(["github.com"]));
        assert_eq!(v["excludeDomains"], serde_json::json!(["pinterest.com"]));
    }

    fn one_page(body: &str) -> ExaResponse {
        ExaResponse {
            results: vec![ExaResult {
                title: Some("t".into()),
                url: Some("https://example.com/p".into()),
                text: Some(body.into()),
                published_date: None,
            }],
        }
    }

    /// Exa's `contents.text` is a page body. It used to be assigned to
    /// `snippet` and nothing else, so the tool face clamped every result to
    /// its snippet bound and told the reader to `web_fetch` the url — for a
    /// page this call had already downloaded and discarded. Asked for bodies,
    /// the body is now the body.
    #[test]
    fn a_page_body_reaches_full_content_when_the_caller_asked_for_bodies() {
        let body = "x".repeat(5_000);
        let opts = SearchOptions {
            include_full_content: true,
            ..Default::default()
        };
        let mapped = ExaProvider::map_response(one_page(&body), &opts);
        assert_eq!(mapped.len(), 1);
        assert_eq!(
            mapped[0].full_content.as_deref(),
            Some(body.as_str()),
            "the body Exa sent has to arrive as a body"
        );
    }

    /// Not asked for, the body must not be smuggled through: `full_content`
    /// is the field the tool face budgets, and a provider that fills it
    /// unrequested spends a caller's context on something nobody asked for.
    /// Firecrawl already had this discipline (`scrape_options` only when
    /// requested); Exa now matches it.
    #[test]
    fn no_body_is_returned_when_the_caller_did_not_ask_for_one() {
        let mapped = ExaProvider::map_response(one_page("body"), &SearchOptions::default());
        assert!(mapped[0].full_content.is_none());
        assert_eq!(mapped[0].snippet, "body");
    }

    /// A result Exa returned without a url is dropped, and one without a
    /// title is not: a url is a result's identity, a title is decoration.
    #[test]
    fn a_result_without_a_url_does_not_survive_but_one_without_a_title_does() {
        let response = ExaResponse {
            results: vec![
                ExaResult {
                    title: None,
                    url: Some("https://kept.test".into()),
                    text: None,
                    published_date: None,
                },
                ExaResult {
                    title: Some("no url".into()),
                    url: None,
                    text: None,
                    published_date: None,
                },
            ],
        };
        let mapped = ExaProvider::map_response(response, &SearchOptions::default());
        let kept = crate::search::providers::base::retain_usable(NAME, mapped);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].url, "https://kept.test");
        assert_eq!(kept[0].title, "");
    }

    fn text_field(options: &SearchOptions) -> serde_json::Value {
        serde_json::to_value(ExaProvider::build_request("q", options)).unwrap()["contents"]["text"]
            .clone()
    }

    /// The saving. Without a declared budget Exa returns a whole page per
    /// result and the tool face keeps 600 characters of it; the rest was
    /// Exa's bill and our latency.
    #[test]
    fn a_declared_snippet_budget_becomes_a_server_side_cap() {
        let capped = text_field(&SearchOptions {
            snippet_budget_chars: Some(601),
            ..Default::default()
        });
        assert_eq!(capped, serde_json::json!({ "maxCharacters": 601 }));
    }

    /// A caller that asked for page bodies must still get them whole: the
    /// snippet budget describes a snippet, and capping the body here would
    /// silently shorten the thing `full_content` exists to deliver.
    #[test]
    fn asking_for_bodies_still_asks_exa_for_the_whole_page() {
        let body_request = text_field(&SearchOptions {
            include_full_content: true,
            snippet_budget_chars: Some(601),
            ..Default::default()
        });
        assert_eq!(body_request, serde_json::json!(true));
    }

    /// Every caller but the tool face declares no budget, and for them the
    /// request has to be byte-identical to what it was before this field
    /// existed — a default that silently shortened answers would be a change
    /// nobody asked for wearing an opt-in's clothes.
    #[test]
    fn no_declared_budget_is_the_request_that_shipped_before() {
        assert_eq!(
            text_field(&SearchOptions::default()),
            serde_json::json!(true)
        );
    }

    /// `maxCharacters` is documented as `1..=10000`. A caller's budget is not
    /// obliged to know that, and an out-of-range value would not shorten an
    /// answer — it would 400 the whole backend on every search.
    #[test]
    fn a_budget_outside_what_exa_accepts_is_clamped_not_forwarded() {
        assert_eq!(
            text_field(&SearchOptions {
                snippet_budget_chars: Some(50_000),
                ..Default::default()
            }),
            serde_json::json!({ "maxCharacters": MAX_CHARACTERS_CEILING })
        );
        assert_eq!(
            text_field(&SearchOptions {
                snippet_budget_chars: Some(0),
                ..Default::default()
            }),
            serde_json::json!({ "maxCharacters": 1 }),
            "zero is not a legal value for this field either"
        );
    }

    #[test]
    fn empty_domain_lists_are_omitted_entirely() {
        let body = ExaProvider::build_request("q", &SearchOptions::default());
        let v = serde_json::to_value(&body).unwrap();
        assert!(v.get("includeDomains").is_none(), "{v}");
        assert!(v.get("excludeDomains").is_none(), "{v}");
    }
}
