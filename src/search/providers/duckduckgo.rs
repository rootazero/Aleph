use crate::error::{AlephError, Result};
use crate::search::providers::base::{build_client, check_status};
use crate::search::{SearchOptions, SearchProvider, SearchResult};
use async_trait::async_trait;
use reqwest::Client;
use scraper::{Html, Selector};

/// `DuckDuckGo` HTML-scrape provider (`html.duckduckgo.com`)
///
/// Zero-account, no API-key, but parses DDG's HTML output instead of a
/// stable JSON API — when DDG redesigns the page this provider breaks.
/// Use as the last-resort fallback; prefer Brave/Tavily/Jina when keys
/// are available.
///
/// **Why GET, not POST**: DDG's anti-bot heuristics serve a 202-Accepted
/// challenge page for `POST /html/` from non-curl HTTP clients (reqwest's
/// TLS fingerprint trips it). `GET /html/?q=...` with a desktop UA
/// returns the real result page reliably.
const NAME: &str = "duckduckgo";
const ENDPOINT: &str = "https://html.duckduckgo.com/html/";
const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

#[derive(Debug)]
pub struct DuckDuckGoProvider {
    client: Client,
}

impl DuckDuckGoProvider {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: build_client()?,
        })
    }
}

impl Default for DuckDuckGoProvider {
    fn default() -> Self {
        Self {
            client: build_client().unwrap_or_else(|_| Client::new()),
        }
    }
}

#[async_trait]
impl SearchProvider for DuckDuckGoProvider {
    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>> {
        let mut params: Vec<(&str, String)> = vec![
            ("q", query.to_string()),
            ("kp", options.ddg_kp().to_string()),
        ];
        if let Some(region) = options.region.as_deref() {
            // DDG's `kl` expects a locale like `us-en` / `cn-zh`. If the
            // caller already supplied a hyphenated locale, pass it through;
            // otherwise pair the region with `language` when present.
            let kl = if region.contains('-') {
                region.to_lowercase()
            } else if let Some(lang) = options.language.as_deref() {
                format!("{}-{}", region.to_lowercase(), lang.to_lowercase())
            } else {
                region.to_lowercase()
            };
            params.push(("kl", kl));
        }
        if let Some(df) = options.ddg_df() {
            params.push(("df", df.to_string()));
        }

        let response = self
            .client
            .get(ENDPOINT)
            .query(&params)
            .header("User-Agent", USER_AGENT)
            .header("Accept", "text/html")
            .timeout(std::time::Duration::from_secs(options.validated_timeout()))
            .send()
            .await
            .map_err(|e| AlephError::network(e.to_string()))?;

        let response = check_status(response, NAME)?;
        let body = response
            .text()
            .await
            .map_err(|e| AlephError::provider(format!("Failed to read {NAME} body: {e}")))?;

        let results = parse_ddg_html(&body, options.validated_max_results());

        // Mirror the SearXNG dead-engine treatment: an empty HTML parse on
        // a 200 OK page almost always means DDG served a challenge / 0-result
        // page, which the LLM cannot distinguish from "no matches" unless we
        // surface it as an error and trip the consecutive-failure cap.
        if results.is_empty() {
            return Err(AlephError::provider(format!(
                "{NAME} returned 0 results — DDG may have served a challenge \
                 page or the HTML selectors are stale. Consider switching \
                 search providers."
            )));
        }

        Ok(results)
    }

    fn name(&self) -> &str {
        NAME
    }

    fn is_available(&self) -> bool {
        true
    }
}

/// Parse DDG's `/html/` result page.
///
/// Selector layout (verified against today's DDG output — keep this comment
/// up-to-date with what we actually scrape):
///
/// ```html
/// <div class="result results_links results_links_deep web-result">
///   <h2 class="result__title">
///     <a class="result__a" href="https://example.com/">Title</a>
///   </h2>
///   <a class="result__snippet" href="...">Snippet with <b>bold</b> hits</a>
/// </div>
/// ```
///
/// Returned hrefs may be:
/// - Direct (`https://example.com/`) — current DDG format
/// - Redirect (`//duckduckgo.com/l/?uddg=ENCODED&...`) — older DDG format
///   we still handle defensively by extracting the `uddg` query param.
///
/// Exposed `pub(crate)` so the `WebFetch` SERP fallback layer
/// ([`crate::search::WebFetchSerpFallback`]) can reuse this exact parser
/// instead of forking a sibling copy — keeps the DDG selector canon
/// single-sourced.
pub(crate) fn parse_ddg_html(body: &str, max_results: usize) -> Vec<SearchResult> {
    let doc = Html::parse_document(body);
    // Both `Selector::parse` calls are against compile-time-known strings
    // and will only fail if scraper's selector grammar changes — treat
    // failure as "no results" rather than panicking.
    let Ok(result_sel) = Selector::parse("div.result") else {
        return Vec::new();
    };
    let Ok(title_sel) = Selector::parse("a.result__a") else {
        return Vec::new();
    };
    let Ok(snippet_sel) = Selector::parse("a.result__snippet") else {
        return Vec::new();
    };

    let mut out: Vec<SearchResult> = Vec::new();
    for node in doc.select(&result_sel) {
        if out.len() >= max_results {
            break;
        }
        let Some(title_a) = node.select(&title_sel).next() else {
            continue;
        };
        let raw_href = title_a.value().attr("href").unwrap_or_default();
        let url = normalize_ddg_href(raw_href);
        if url.is_empty() {
            continue;
        }
        let title = title_a.text().collect::<String>().trim().to_string();
        let snippet = node
            .select(&snippet_sel)
            .next()
            .map(|n| n.text().collect::<String>().trim().to_string())
            .unwrap_or_default();

        out.push(SearchResult {
            title,
            url,
            snippet,
            relevance_score: None,
            full_content: None,
            provider: Some(NAME.to_string()),
        });
    }
    out
}

/// Parse DDG's `/lite/` result page (the no-JS endpoint at
/// `lite.duckduckgo.com`).
///
/// The Lite layout is a flat `<table>` with paired result rows: each result
/// contributes one `<a class="result-link">` (title + url) followed by a
/// sibling `<td class="result-snippet">` (snippet). Iterating rows in
/// document order pairs them up — DDG always emits them in matching order.
///
/// Layout (verified against today's Lite output):
///
/// ```html
/// <tr><td valign="top">1.</td><td>
///   <a rel="nofollow" href="https://example.com/" class="result-link">Title</a>
/// </td></tr>
/// <tr><td>&nbsp;</td><td class="result-snippet">Snippet text</td></tr>
/// <tr><td>&nbsp;</td><td><span class="link-text">example.com</span></td></tr>
/// ```
///
/// Exposed `pub(crate)` for the same reason as [`parse_ddg_html`].
pub(crate) fn parse_ddg_lite_html(body: &str, max_results: usize) -> Vec<SearchResult> {
    let doc = Html::parse_document(body);
    let Ok(link_sel) = Selector::parse("a.result-link") else {
        return Vec::new();
    };
    let Ok(snippet_sel) = Selector::parse("td.result-snippet") else {
        return Vec::new();
    };

    let titles: Vec<(String, String)> = doc
        .select(&link_sel)
        .filter_map(|a| {
            let url = normalize_ddg_href(a.value().attr("href").unwrap_or_default());
            if url.is_empty() {
                return None;
            }
            let title = a.text().collect::<String>().trim().to_string();
            Some((title, url))
        })
        .collect();

    let snippets: Vec<String> = doc
        .select(&snippet_sel)
        .map(|td| td.text().collect::<String>().trim().to_string())
        .collect();

    titles
        .into_iter()
        .zip(snippets.into_iter().chain(std::iter::repeat(String::new())))
        .take(max_results)
        .map(|((title, url), snippet)| SearchResult {
            title,
            url,
            snippet,
            relevance_score: None,
            full_content: None,
            provider: Some(NAME.to_string()),
        })
        .collect()
}

/// Extract the real URL from a DDG href.
///
/// - Direct `https://...` → returned as-is
/// - Protocol-relative redirect `//duckduckgo.com/l/?uddg=ENCODED` →
///   percent-decode the `uddg` param
/// - Anything else → empty string (skip)
fn normalize_ddg_href(href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }
    // Defensive: handle the old uddg redirect form even though current DDG
    // doesn't use it — costs nothing and survives a future DDG flip-back.
    if let Some(qs_start) = href.find('?') {
        for pair in href[qs_start + 1..].split('&') {
            if let Some(v) = pair.strip_prefix("uddg=") {
                return percent_decode(v);
            }
        }
    }
    String::new()
}

fn percent_decode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Factory entry for the search provider registry.
///
/// Co-located with the concrete provider so adding a new search
/// backend is a single-file change (provider + factory) plus one
/// registration line in `ProviderFactoryRegistry::with_defaults`.
pub struct DuckDuckGoFactory;

impl crate::search::ProviderFactory for DuckDuckGoFactory {
    fn provider_type(&self) -> &'static str {
        NAME
    }
    fn build(
        &self,
        name: &str,
        _backend: &crate::config::types::SearchBackendConfig,
    ) -> crate::error::Result<Option<crate::sync_primitives::Arc<dyn crate::search::SearchProvider>>>
    {
        match DuckDuckGoProvider::new() {
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
    fn ddg_provider_constructs() {
        let provider = DuckDuckGoProvider::new().unwrap();
        assert_eq!(provider.name(), "duckduckgo");
        assert!(provider.is_available());
    }

    /// Real fragment of today's DDG HTML output, verbatim. If DDG changes
    /// the selectors this test fails — that's the early-warning we want.
    #[test]
    fn parse_ddg_html_extracts_results() {
        let html = r#"
        <html><body>
        <div class="result results_links results_links_deep web-result ">
          <div class="links_main links_deep result__body">
            <h2 class="result__title">
              <a rel="nofollow" class="result__a" href="https://rust-lang.org/">Rust Programming Language</a>
            </h2>
            <a class="result__snippet" href="https://rust-lang.org/">A <b>language</b> empowering everyone.</a>
          </div>
        </div>
        <div class="result results_links results_links_deep web-result ">
          <div class="links_main links_deep result__body">
            <h2 class="result__title">
              <a rel="nofollow" class="result__a" href="https://en.wikipedia.org/wiki/Rust_(programming_language)">Rust (programming language) - Wikipedia</a>
            </h2>
            <a class="result__snippet" href="https://en.wikipedia.org/wiki/Rust_(programming_language)"><b>Rust</b> is a general-purpose <b>programming</b> language.</a>
          </div>
        </div>
        </body></html>
        "#;
        let results = parse_ddg_html(html, 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].url, "https://rust-lang.org/");
        assert_eq!(results[0].snippet, "A language empowering everyone.");
        assert_eq!(results[0].provider.as_deref(), Some("duckduckgo"));
        assert_eq!(results[1].title, "Rust (programming language) - Wikipedia");
    }

    /// max_results honoured (caller cap, not just provider response size).
    #[test]
    fn parse_ddg_html_respects_max_results() {
        let mut html = String::from("<html><body>");
        for i in 0..20 {
            html.push_str(&format!(
                r#"<div class="result"><h2 class="result__title">
                <a class="result__a" href="https://e{i}.test/">T{i}</a></h2>
                <a class="result__snippet">S{i}</a></div>"#
            ));
        }
        html.push_str("</body></html>");
        let results = parse_ddg_html(&html, 5);
        assert_eq!(results.len(), 5);
        assert_eq!(results[4].title, "T4");
    }

    /// Empty body / malformed page → empty Vec, no panic.
    #[test]
    fn parse_ddg_html_empty_when_no_results() {
        assert!(parse_ddg_html("<html><body>No matches found.</body></html>", 10).is_empty());
        assert!(parse_ddg_html("", 10).is_empty());
    }

    /// uddg redirect URLs (legacy DDG format) decode correctly.
    #[test]
    fn normalize_ddg_href_decodes_uddg() {
        assert_eq!(
            normalize_ddg_href("//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fpath&rut=1"),
            "https://example.com/path"
        );
    }

    /// Direct hrefs pass through unchanged.
    #[test]
    fn normalize_ddg_href_passes_direct_url() {
        assert_eq!(
            normalize_ddg_href("https://rust-lang.org/"),
            "https://rust-lang.org/"
        );
        assert_eq!(
            normalize_ddg_href("http://example.com"),
            "http://example.com"
        );
    }

    /// Unrecognised href shapes (javascript:, mailto:, fragments) are dropped.
    #[test]
    fn normalize_ddg_href_drops_unknown_shapes() {
        assert_eq!(normalize_ddg_href("javascript:void(0)"), "");
        assert_eq!(normalize_ddg_href("#fragment"), "");
        assert_eq!(normalize_ddg_href(""), "");
    }

    /// Real fragment of today's DDG Lite output, verbatim. Used both by
    /// this provider's own tests and by the fallback layer's tests.
    #[test]
    fn parse_ddg_lite_html_pairs_link_and_snippet() {
        let html = r#"
        <html><body>
        <table>
          <tr><td valign="top">1.</td><td>
            <a rel="nofollow" href="https://rust-lang.org/" class="result-link">Rust Programming Language</a>
          </td></tr>
          <tr><td>&nbsp;</td><td class="result-snippet">A language empowering everyone to build reliable and efficient software.</td></tr>
          <tr><td>&nbsp;</td><td><span class="link-text">rust-lang.org</span></td></tr>
          <tr><td valign="top">2.</td><td>
            <a rel="nofollow" href="https://en.wikipedia.org/wiki/Rust_(programming_language)" class="result-link">Rust (programming language) - Wikipedia</a>
          </td></tr>
          <tr><td>&nbsp;</td><td class="result-snippet">Rust is a general-purpose programming language.</td></tr>
        </table>
        </body></html>
        "#;
        let results = parse_ddg_lite_html(html, 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].url, "https://rust-lang.org/");
        assert!(results[0].snippet.contains("empowering everyone"));
        assert_eq!(results[0].provider.as_deref(), Some("duckduckgo"));
        assert_eq!(results[1].title, "Rust (programming language) - Wikipedia");
    }

    /// max_results honoured on Lite endpoint too.
    #[test]
    fn parse_ddg_lite_html_respects_max_results() {
        let mut html = String::from("<html><body><table>");
        for i in 0..15 {
            html.push_str(&format!(
                r#"<tr><td>{i}.</td><td>
                <a class="result-link" href="https://e{i}.test/">T{i}</a></td></tr>
                <tr><td></td><td class="result-snippet">S{i}</td></tr>"#
            ));
        }
        html.push_str("</table></body></html>");
        let results = parse_ddg_lite_html(&html, 4);
        assert_eq!(results.len(), 4);
        assert_eq!(results[3].title, "T3");
    }

    /// Empty / malformed Lite page → empty Vec.
    #[test]
    fn parse_ddg_lite_html_empty_when_no_results() {
        assert!(parse_ddg_lite_html("<html><body>No matches.</body></html>", 10).is_empty());
        assert!(parse_ddg_lite_html("", 10).is_empty());
    }

    /// Lite endpoint with more titles than snippets — extra titles get empty
    /// snippets rather than being dropped (defensive against DDG layout drift).
    #[test]
    fn parse_ddg_lite_html_pads_missing_snippets() {
        let html = r#"
        <table>
          <tr><td><a class="result-link" href="https://a.test/">A</a></td></tr>
          <tr><td><a class="result-link" href="https://b.test/">B</a></td></tr>
          <tr><td class="result-snippet">snippet for A only</td></tr>
        </table>
        "#;
        let results = parse_ddg_lite_html(html, 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].snippet, "snippet for A only");
        assert_eq!(
            results[1].snippet, "",
            "missing snippet must not drop the title row"
        );
    }
}
