/// Search result data structure
///
/// This module defines the unified `SearchResult` struct that represents
/// search results from all providers (Tavily, Google, Bing, `SearXNG`, etc.)
use serde::{Deserialize, Serialize};

/// Search result entry returned by all providers
///
/// This struct provides a unified interface for search results from
/// different providers (Google, Bing, Tavily, etc.)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResult {
    /// Result title
    pub title: String,

    /// Source URL
    pub url: String,

    /// Snippet/summary of the content
    pub snippet: String,

    /// Relevance score (0.0 - 1.0)
    /// Tavily provides this natively; others may compute it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relevance_score: Option<f32>,

    /// Full content (only for Tavily deep search)
    /// WARNING: Can be very large, use sparingly
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_content: Option<String>,

    /// When the page says it was published, in whatever spelling the backend
    /// sent (RFC 2822 from Tavily, ISO-8601 from Exa). Not normalised: a
    /// reader can tell 2019 from last week in either, and parsing every
    /// vendor's dialect to re-emit one of them buys nothing a caller asked
    /// for.
    ///
    /// `None` means **this backend did not say** — never "this result has no
    /// date". The distinction is the whole point of the field: a backend that
    /// reports no dates makes every result look equally undated, and a caller
    /// asking about recent work needs to know which of the two they are
    /// looking at. Backends that send nothing leave it absent rather than
    /// having a date invented for them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_date: Option<String>,

    /// Provider that returned this result
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

impl SearchResult {
    /// Create a basic search result (for testing/mocking)
    pub fn new(
        title: impl Into<String>,
        url: impl Into<String>,
        snippet: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            url: url.into(),
            snippet: snippet.into(),
            relevance_score: None,
            full_content: None,
            published_date: None,
            provider: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_result_new() {
        let result = SearchResult::new(
            "Test Title".to_string(),
            "https://example.com".to_string(),
            "Test snippet".to_string(),
        );

        assert_eq!(result.title, "Test Title");
        assert_eq!(result.url, "https://example.com");
        assert_eq!(result.snippet, "Test snippet");
        assert!(result.provider.is_none());
    }

    #[test]
    fn test_search_result_serialization() {
        let result = SearchResult::new(
            "Test".to_string(),
            "https://test.com".to_string(),
            "Snippet".to_string(),
        );

        let json = serde_json::to_string(&result).unwrap();
        let deserialized: SearchResult = serde_json::from_str(&json).unwrap();

        assert_eq!(result, deserialized);
    }

    /// A field this struct once carried and no longer does (`source_type`)
    /// must still parse — we don't `deny_unknown_fields`, and that is the
    /// wire-level back-compat this test pins.
    ///
    /// `published_date` is back, so it is no longer one of them: it was cut in
    /// `8297b1fce` for having zero producers, and returns here with two
    /// (Tavily and Exa) and a renderer on the tool face. It is typed as the
    /// string the vendors send; the numeric epoch this test used to pass was
    /// never a shape anything emitted, since every provider left the field
    /// `None` and `skip_serializing_if` meant it never reached the wire at
    /// all.
    #[test]
    fn test_search_result_ignores_legacy_fields() {
        let legacy_json = r#"{
            "title": "Test",
            "url": "https://test.com",
            "snippet": "Snippet",
            "published_date": "Mon, 01 Jan 2024 00:00:00 GMT",
            "source_type": "article",
            "relevance_score": 0.5,
            "provider": "tavily"
        }"#;

        let parsed: SearchResult = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(parsed.title, "Test");
        assert_eq!(parsed.relevance_score, Some(0.5));
        assert_eq!(parsed.provider.as_deref(), Some("tavily"));
        assert_eq!(
            parsed.published_date.as_deref(),
            Some("Mon, 01 Jan 2024 00:00:00 GMT")
        );
    }

    #[test]
    fn test_search_result_with_optional_fields() {
        let result = SearchResult {
            title: "Test".to_string(),
            url: "https://test.com".to_string(),
            snippet: "Test snippet".to_string(),
            relevance_score: Some(0.95),
            full_content: None,
            published_date: None,
            provider: Some("tavily".to_string()),
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"title\":\"Test\""));
        assert!(json.contains("\"relevance_score\":0.95"));

        let parsed: SearchResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.title, "Test");
        assert_eq!(parsed.relevance_score, Some(0.95));
    }
}
