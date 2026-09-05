//! Web fetch policies
//!
//! Configurable parameters for web content fetching including
//! content limits, timeouts, and user agent settings.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Policy for web fetch behavior
///
/// Controls content size limits, request timeouts, and HTTP client settings
/// for web scraping operations.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
    /// Default: "Aleph/1.0"
    #[serde(default = "default_user_agent")]
    pub user_agent: String,

    /// Request timeout in seconds
    /// Default: 30
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,

    /// Whether to use Readability algorithm for content extraction
    /// When false, falls back to CSS selector-based extraction
    /// Default: true
    #[serde(default = "default_enable_readability")]
    pub enable_readability: bool,

    /// Whether PDF responses take the lopdf text-extraction pipeline
    /// (per-page `[page N]` markers, honest error when there is no text
    /// layer). When false, PDFs fall through to the HTML path, which
    /// mangles binary content into lossy UTF-8 — kept only as an escape
    /// hatch.
    /// Default: true
    #[serde(default = "default_pdf_extract")]
    pub pdf_extract: bool,

    /// Whether YouTube URLs take the yt-dlp transcript pipeline (subtitles
    /// cleaned from VTT, scrolling-overlap deduplicated). Soft failures
    /// (yt-dlp not installed, no subtitles) fall back to the generic HTTP
    /// path. Default: true
    #[serde(default = "default_youtube_transcript")]
    pub youtube_transcript: bool,

    /// Legacy crawl4ai backend config. Read only by `Config::migrate_fetch`,
    /// which folds it into `[fetch].backends.crawl4ai` at load. Note that
    /// `[fetch]` providers are currently NOT wired into `web_fetch` at
    /// runtime (BT-D-R4-22: the SSRF DNS pin cannot be enforced on a
    /// provider-side crawl); a configured backend triggers a one-time startup
    /// warning and the built-in pinned fetch is used instead.
    #[serde(default)]
    pub crawl4ai: Crawl4aiConfig,
}

impl Default for WebFetchPolicy {
    fn default() -> Self {
        Self {
            max_content_length: default_max_content_length(),
            min_content_length: default_min_content_length(),
            user_agent: default_user_agent(),
            timeout_seconds: default_timeout_seconds(),
            enable_readability: default_enable_readability(),
            pdf_extract: default_pdf_extract(),
            youtube_transcript: default_youtube_transcript(),
            crawl4ai: Crawl4aiConfig::default(),
        }
    }
}

const fn default_max_content_length() -> u64 {
    10000
}

const fn default_min_content_length() -> u64 {
    100
}

fn default_user_agent() -> String {
    "Aleph/1.0".to_string()
}

const fn default_timeout_seconds() -> u64 {
    30
}

const fn default_enable_readability() -> bool {
    true
}

const fn default_pdf_extract() -> bool {
    true
}

const fn default_youtube_transcript() -> bool {
    true
}

const fn default_crawl4ai_timeout() -> u64 {
    60
}

/// Legacy crawl4ai backend configuration (`[policies.web_fetch.crawl4ai]`).
///
/// Superseded by `[fetch].backends.crawl4ai`; `Config::migrate_fetch` folds
/// this section into `[fetch]` at load. Fetch providers are not wired into
/// `web_fetch` at runtime (BT-D-R4-22) — see the `fetch` module docs. The
/// struct is still constructed programmatically by
/// `crate::fetch::providers::crawl4ai` for the connection-test RPC.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Crawl4aiConfig {
    /// Whether the crawl4ai backend is active. Default: false.
    #[serde(default)]
    pub enabled: bool,

    /// Base URL of the crawl4ai server, e.g. "http://10.10.10.3:11235".
    #[serde(default)]
    pub base_url: String,

    /// Request timeout in seconds. crawl4ai drives a headless browser, so it
    /// is slower than a plain HTTP GET. Default: 60.
    #[serde(default = "default_crawl4ai_timeout")]
    pub timeout_seconds: u64,

    /// Runtime-only bearer token. Never persisted to config.toml —
    /// populated programmatically from the vault-resolved
    /// `FetchBackendConfig.api_key` when the connection-test RPC builds a
    /// backend (mirrors `SearchBackendConfig::api_key`).
    #[serde(default, skip_serializing)]
    #[schemars(skip)]
    pub token: Option<String>,
}

impl Default for Crawl4aiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: String::new(),
            timeout_seconds: default_crawl4ai_timeout(),
            token: None,
        }
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
        assert_eq!(policy.user_agent, "Aleph/1.0");
        assert_eq!(policy.timeout_seconds, 30);
        assert!(policy.enable_readability);
        assert!(policy.pdf_extract);
        assert!(policy.youtube_transcript);
    }

    #[test]
    fn pdf_extract_parses_explicit_false() {
        let toml = r#"
            pdf_extract = false
        "#;
        let policy: WebFetchPolicy = toml::from_str(toml).unwrap();
        assert!(!policy.pdf_extract);
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

    #[test]
    fn crawl4ai_defaults_are_off_with_60s_timeout() {
        let cfg = Crawl4aiConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.base_url.is_empty());
        assert_eq!(cfg.timeout_seconds, 60);
        assert!(cfg.token.is_none());
    }

    #[test]
    fn web_fetch_policy_without_crawl4ai_section_uses_defaults() {
        // A pre-existing config with no [crawl4ai] table must still parse and
        // leave the backend disabled (back-compat / zero regression).
        let toml = r#"
            max_content_length = 20000
        "#;
        let policy: WebFetchPolicy = toml::from_str(toml).unwrap();
        assert!(!policy.crawl4ai.enabled);
        assert_eq!(policy.crawl4ai.timeout_seconds, 60);
    }

    #[test]
    fn crawl4ai_section_parses_enabled_base_url_timeout() {
        let toml = r#"
            [crawl4ai]
            enabled = true
            base_url = "http://10.10.10.3:11235"
            timeout_seconds = 45
        "#;
        let policy: WebFetchPolicy = toml::from_str(toml).unwrap();
        assert!(policy.crawl4ai.enabled);
        assert_eq!(policy.crawl4ai.base_url, "http://10.10.10.3:11235");
        assert_eq!(policy.crawl4ai.timeout_seconds, 45);
        // token never comes from TOML
        assert!(policy.crawl4ai.token.is_none());
    }

    #[test]
    fn crawl4ai_token_is_never_serialized() {
        // Runtime-only vault field: a token set in memory must NOT round-trip
        // into serialized config (mirrors SearchBackendConfig::api_key).
        let cfg = Crawl4aiConfig {
            enabled: true,
            base_url: "http://x".into(),
            timeout_seconds: 60,
            token: Some("secret".into()),
        };
        let json = serde_json::to_value(&cfg).unwrap();
        assert!(
            json.get("token").is_none(),
            "token must be skip_serializing"
        );
    }
}
