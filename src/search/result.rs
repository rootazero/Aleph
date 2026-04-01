/// Search result data structure
///
/// This module defines the unified `SearchResult` struct that represents
/// search results from all providers (Tavily, Google, Bing, SearXNG, etc.)
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

    /// Publication date (Unix timestamp)
    /// Optional because not all providers return this
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_date: Option<i64>,

    /// Relevance score (0.0 - 1.0)
    /// Tavily provides this natively; others may compute it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relevance_score: Option<f32>,

    /// Source type (article, video, forum, etc.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_type: Option<String>,

    /// Full content (only for Tavily deep search)
    /// WARNING: Can be very large, use sparingly
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_content: Option<String>,

    /// Provider that returned this result
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

impl SearchResult {
    /// Create a basic search result (for testing/mocking)
    pub fn new(title: String, url: String, snippet: String) -> Self {
        Self {
            title,
            url,
            snippet,
            published_date: None,
            relevance_score: None,
            source_type: None,
            full_content: None,
            provider: None,
        }
    }

    /// Calculate content length (snippet + full_content)
    pub fn content_length(&self) -> usize {
        self.snippet.len() + self.full_content.as_ref().map(|c| c.len()).unwrap_or(0)
    }

    /// Check if result has full content
    pub fn has_full_content(&self) -> bool {
        self.full_content.is_some()
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
        assert!(result.published_date.is_none());
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

    #[test]
    fn test_search_result_with_optional_fields() {
        let result = SearchResult {
            title: "Test".to_string(),
            url: "https://test.com".to_string(),
            snippet: "Test snippet".to_string(),
            published_date: Some(1704067200),
            relevance_score: Some(0.95),
            source_type: Some("article".to_string()),
            full_content: None,
            provider: Some("tavily".to_string()),
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"title\":\"Test\""));
        assert!(json.contains("\"relevance_score\":0.95"));

        let parsed: SearchResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.title, "Test");
        assert_eq!(parsed.relevance_score, Some(0.95));
    }

    #[test]
    fn test_content_length() {
        let mut result = SearchResult::new(
            "Title".to_string(),
            "https://example.com".to_string(),
            "Short snippet".to_string(),
        );

        assert_eq!(result.content_length(), "Short snippet".len());

        result.full_content = Some("Full content here".to_string());
        assert_eq!(
            result.content_length(),
            "Short snippet".len() + "Full content here".len()
        );
    }

    #[test]
    fn test_has_full_content() {
        let mut result = SearchResult::new(
            "Title".to_string(),
            "https://example.com".to_string(),
            "Snippet".to_string(),
        );

        assert!(!result.has_full_content());

        result.full_content = Some("Content".to_string());
        assert!(result.has_full_content());
    }
}
