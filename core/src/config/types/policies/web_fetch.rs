//! Web fetch policies
//!
//! Configurable parameters for web content fetching including
//! content limits, timeouts, and user agent settings.

use serde::{Deserialize, Serialize};

/// Policy for web fetch behavior
///
/// Controls content size limits, request timeouts, and HTTP client settings
/// for web scraping operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebFetchPolicy {
    /// Maximum content length in characters to return
    /// Default: 10000
    #[serde(default = "default_max_content_length")]
    pub max_content_length: u64,

    /// Minimum content length to accept a selector match
    /// Default: 100
    #[serde(default = "default_min_content_length")]
    pub min_content_length: u64,

    /// User-Agent header value for HTTP requests
    /// Default: "Aether/1.0"
    #[serde(default = "default_user_agent")]
    pub user_agent: String,

    /// Request timeout in seconds
    /// Default: 30
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,

    /// Whether to follow HTTP redirects
    /// Default: true
    #[serde(default = "default_follow_redirects")]
    pub follow_redirects: bool,

    /// Maximum number of redirects to follow
    /// Default: 10
    #[serde(default = "default_max_redirects")]
    pub max_redirects: u64,

    /// CSS selectors for content extraction in priority order
    /// Default: ["article", "main", ".content", ".post-content", "#content", "body"]
    #[serde(default = "default_content_selectors")]
    pub content_selectors: Vec<String>,
}

impl Default for WebFetchPolicy {
    fn default() -> Self {
        Self {
            max_content_length: default_max_content_length(),
            min_content_length: default_min_content_length(),
            user_agent: default_user_agent(),
            timeout_seconds: default_timeout_seconds(),
            follow_redirects: default_follow_redirects(),
            max_redirects: default_max_redirects(),
            content_selectors: default_content_selectors(),
        }
    }
}

fn default_max_content_length() -> u64 {
    10000
}

fn default_min_content_length() -> u64 {
    100
}

fn default_user_agent() -> String {
    "Aether/1.0".to_string()
}

fn default_timeout_seconds() -> u64 {
    30
}

fn default_follow_redirects() -> bool {
    true
}

fn default_max_redirects() -> u64 {
    10
}

fn default_content_selectors() -> Vec<String> {
    vec![
        "article".to_string(),
        "main".to_string(),
        ".content".to_string(),
        ".post-content".to_string(),
        "#content".to_string(),
        "body".to_string(),
    ]
}

impl WebFetchPolicy {
    /// Get timeout as std::time::Duration
    pub fn timeout_duration(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.timeout_seconds)
    }

    /// Check if content length is within acceptable range
    pub fn is_content_acceptable(&self, length: usize) -> bool {
        let len = length as u64;
        len >= self.min_content_length && len <= self.max_content_length
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values() {
        let policy = WebFetchPolicy::default();
        assert_eq!(policy.max_content_length, 10000);
        assert_eq!(policy.min_content_length, 100);
        assert_eq!(policy.user_agent, "Aether/1.0");
        assert_eq!(policy.timeout_seconds, 30);
        assert!(policy.follow_redirects);
    }

    #[test]
    fn test_content_selectors() {
        let policy = WebFetchPolicy::default();
        assert!(policy.content_selectors.contains(&"article".to_string()));
        assert!(policy.content_selectors.contains(&"main".to_string()));
        assert!(policy.content_selectors.contains(&"body".to_string()));
    }

    #[test]
    fn test_content_acceptable() {
        let policy = WebFetchPolicy::default();
        assert!(!policy.is_content_acceptable(50)); // Too short
        assert!(policy.is_content_acceptable(500)); // OK
        assert!(policy.is_content_acceptable(10000)); // At max
        assert!(!policy.is_content_acceptable(10001)); // Too long
    }

    #[test]
    fn test_partial_deserialization() {
        let toml = r#"
            max_content_length = 20000
            user_agent = "CustomBot/2.0"
        "#;
        let policy: WebFetchPolicy = toml::from_str(toml).unwrap();
        assert_eq!(policy.max_content_length, 20000);
        assert_eq!(policy.user_agent, "CustomBot/2.0");
        // Defaults for unspecified
        assert_eq!(policy.min_content_length, 100);
        assert_eq!(policy.timeout_seconds, 30);
    }
}
