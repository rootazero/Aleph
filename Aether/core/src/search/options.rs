/// Search options configuration
///
/// This module defines options passed to search providers

use serde::{Deserialize, Serialize};

/// Search options passed to providers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchOptions {
    /// Language code (ISO 639-1: "en", "zh", "ja", etc.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,

    /// Region code (ISO 3166-1 alpha-2: "US", "CN", "JP", etc.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,

    /// Date range filter ("day", "week", "month", "year")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_range: Option<String>,

    /// Enable safe search (adult content filtering)
    #[serde(default = "default_safe_search")]
    pub safe_search: bool,

    /// Maximum number of results (default: 5)
    #[serde(default = "default_max_results")]
    pub max_results: usize,

    /// Timeout in seconds (default: 10)
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,

    /// Include full page content (Tavily only)
    /// WARNING: Significantly increases latency and token usage
    #[serde(default)]
    pub include_full_content: bool,
}

fn default_safe_search() -> bool {
    true
}

fn default_max_results() -> usize {
    5
}

fn default_timeout() -> u64 {
    10
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            language: None,
            region: None,
            date_range: None,
            safe_search: default_safe_search(),
            max_results: default_max_results(),
            timeout_seconds: default_timeout(),
            include_full_content: false,
        }
    }
}

impl SearchOptions {
    /// Create default options with custom timeout
    pub fn default_with_timeout(timeout_seconds: u64) -> Self {
        Self {
            timeout_seconds,
            ..Default::default()
        }
    }
}

/// Quota information for rate-limited providers
#[derive(Debug, Clone)]
pub struct QuotaInfo {
    /// Remaining searches in current period
    pub remaining: Option<u32>,

    /// Total quota limit
    pub limit: Option<u32>,

    /// Reset timestamp (Unix timestamp)
    pub reset_at: Option<i64>,
}

impl QuotaInfo {
    pub fn unlimited() -> Self {
        Self {
            remaining: None,
            limit: None,
            reset_at: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_options_default() {
        let options = SearchOptions::default();

        assert_eq!(options.max_results, 5);
        assert_eq!(options.timeout_seconds, 10);
        assert!(options.safe_search);
        assert!(!options.include_full_content);
        assert!(options.language.is_none());
    }

    #[test]
    fn test_search_options_custom_timeout() {
        let options = SearchOptions::default_with_timeout(20);

        assert_eq!(options.timeout_seconds, 20);
        assert_eq!(options.max_results, 5);
    }

    #[test]
    fn test_search_options_customization() {
        let options = SearchOptions {
            language: Some("zh-CN".to_string()),
            region: Some("CN".to_string()),
            date_range: Some("week".to_string()),
            safe_search: true,
            max_results: 10,
            timeout_seconds: 15,
            include_full_content: true,
        };

        assert_eq!(options.language.unwrap(), "zh-CN");
        assert_eq!(options.max_results, 10);
        assert!(options.include_full_content);
    }

    #[test]
    fn test_quota_info_unlimited() {
        let quota = QuotaInfo::unlimited();

        assert!(quota.remaining.is_none());
        assert!(quota.limit.is_none());
        assert!(quota.reset_at.is_none());
    }
}
