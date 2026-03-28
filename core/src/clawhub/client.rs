//! ClawHub HTTP client — thin wrapper around clawhub.ai public API.
//!
//! All endpoints are public (no authentication required).
//! API contract verified from OpenClaw source code and clawhub CLI v0.7.0.

use std::path::PathBuf;
use std::time::Duration;

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use reqwest::Client;
use tracing::{debug, warn};

use crate::error::{AlephError, Result};

use super::types::*;

const DEFAULT_REGISTRY: &str = "https://clawhub.ai";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Percent-encode a slug for use in URL path segments.
/// Encodes everything except alphanumerics, `-`, `_`, `.`, and `/` (slug separator).
fn encode_slug_path(slug: &str) -> String {
    slug.split('/')
        .map(|segment| utf8_percent_encode(segment, NON_ALPHANUMERIC).to_string())
        .collect::<Vec<_>>()
        .join("/")
}

/// HTTP client for ClawHub skill registry.
///
/// All methods are read-only (search, browse, download).
/// No authentication required for public API endpoints.
#[derive(Clone)]
pub struct ClawHubClient {
    base_url: String,
    http: Client,
}

impl ClawHubClient {
    /// Create a new client, honoring `CLAWHUB_REGISTRY` env var if set.
    pub fn new() -> Self {
        let registry = std::env::var("CLAWHUB_REGISTRY")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_REGISTRY.to_string());
        Self::with_registry(&registry)
    }

    /// Create a client pointing to a custom registry URL
    pub fn with_registry(url: &str) -> Self {
        let ua = format!("aleph/{}", env!("ALEPH_VERSION"));
        let http = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(ua)
            .build()
            .unwrap_or_default();

        Self {
            base_url: url.trim_end_matches('/').to_string(),
            http,
        }
    }

    /// Get the registry base URL
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Send a GET request, check status, and parse JSON response
    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        query: &[(&str, String)],
        context: &str,
    ) -> Result<T> {
        let resp = self
            .http
            .get(url)
            .query(query)
            .send()
            .await
            .map_err(|e| AlephError::network(format!("ClawHub {} failed: {}", context, e)))?;

        let resp = Self::check_status(resp, context).await?;

        resp.json()
            .await
            .map_err(|e| AlephError::network(format!("ClawHub {} parse error: {}", context, e)))
    }

    /// Search skills by keyword
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SkillSearchResult>> {
        let url = format!("{}/api/v1/search", self.base_url);
        debug!(query, limit, "ClawHub search");

        let resp: SearchApiResponse = self
            .get_json(
                &url,
                &[
                    ("q", query.to_string()),
                    ("limit", limit.to_string()),
                    ("nonSuspiciousOnly", "true".to_string()),
                ],
                "search",
            )
            .await?;

        Ok(resp.results.into_iter().map(SkillSearchResult::from).collect())
    }

    /// Browse skills with sorting and pagination.
    ///
    /// The `/api/v1/skills` endpoint may return empty results.
    /// When that happens, we fall back to search with a broad query.
    pub async fn browse(
        &self,
        sort: SortOrder,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<BrowseResponse> {
        let url = format!("{}/api/v1/skills", self.base_url);
        debug!(?sort, limit, ?cursor, "ClawHub browse");

        let mut params = vec![
            ("sort", sort.as_api_str().to_string()),
            ("limit", limit.to_string()),
            ("nonSuspiciousOnly", "true".to_string()),
        ];
        if let Some(c) = cursor {
            params.push(("cursor", c.to_string()));
        }

        let api_resp: BrowseApiResponse = self.get_json(&url, &params, "browse").await?;

        // If the browse endpoint returned results, use them
        if !api_resp.items.is_empty() {
            let has_more = api_resp.next_cursor.is_some();
            return Ok(BrowseResponse {
                skills: api_resp
                    .items
                    .into_iter()
                    .map(SkillSearchResult::from)
                    .collect(),
                cursor: api_resp.next_cursor,
                has_more,
            });
        }

        // Fallback: browse endpoint returned empty, use search instead
        debug!("Browse returned empty, falling back to search");
        let results = self.search("", limit).await?;
        Ok(BrowseResponse {
            skills: results,
            cursor: None,
            has_more: false,
        })
    }

    /// Get skill detail by slug.
    ///
    /// The API returns a nested response `{ skill, latestVersion, owner, moderation }`.
    /// We flatten it into our internal `SkillDetail` type.
    pub async fn get_skill(&self, slug: &str) -> Result<SkillDetail> {
        let url = format!("{}/api/v1/skills/{}", self.base_url, encode_slug_path(slug));
        debug!(slug, "ClawHub get_skill");
        let raw: DetailApiResponse = self.get_json(&url, &[], "get_skill").await?;
        Ok(SkillDetail::from(raw))
    }

    /// Get version list for a skill.
    ///
    /// The API returns `{ items: [{version, createdAt, changelog}], nextCursor }`.
    pub async fn get_versions(&self, slug: &str) -> Result<Vec<VersionInfo>> {
        let url = format!(
            "{}/api/v1/skills/{}/versions",
            self.base_url,
            encode_slug_path(slug)
        );
        debug!(slug, "ClawHub get_versions");
        let data: VersionsResponse = self.get_json(&url, &[], "get_versions").await?;
        Ok(data.items.into_iter().map(VersionInfo::from).collect())
    }

    /// Download skill ZIP to a temporary file. Returns path to the temp ZIP.
    pub async fn download(&self, slug: &str, version: Option<&str>) -> Result<PathBuf> {
        let url = format!("{}/api/v1/download", self.base_url);
        debug!(slug, ?version, "ClawHub download");

        let mut params = vec![("slug", slug.to_string())];
        if let Some(v) = version {
            params.push(("version", v.to_string()));
        }

        let resp = self
            .http
            .get(&url)
            .query(&params)
            .send()
            .await
            .map_err(|e| AlephError::network(format!("ClawHub download failed: {}", e)))?;

        let resp = Self::check_status(resp, "download").await?;

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| AlephError::network(format!("ClawHub download read error: {}", e)))?;

        // Sanitize slug for filename: "owner/skill" → "owner-skill"
        let safe_slug = slug.replace('/', "-");
        let temp_path =
            std::env::temp_dir().join(format!("clawhub-{}-{}.zip", safe_slug, uuid::Uuid::new_v4()));

        std::fs::write(&temp_path, &bytes)
            .map_err(|e| AlephError::config(format!("Failed to write temp ZIP: {}", e)))?;

        Ok(temp_path)
    }

    /// Compare versions: returns true if `remote` is newer than `local`.
    /// Falls back to string inequality if semver parsing fails.
    pub fn is_newer_version(local: &str, remote: &str) -> bool {
        match (
            semver::Version::parse(local),
            semver::Version::parse(remote),
        ) {
            (Ok(l), Ok(r)) => r > l,
            _ => {
                warn!(
                    local,
                    remote, "Non-semver version strings, falling back to string compare"
                );
                local != remote
            }
        }
    }

    /// Check HTTP response status. Consumes and returns the response on success.
    /// For known error codes, returns a descriptive message; for others, reads the body.
    async fn check_status(resp: reqwest::Response, context: &str) -> Result<reqwest::Response> {
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }

        let err_msg = match status.as_u16() {
            404 => format!("Skill not found on ClawHub ({})", context),
            403 => "Skill blocked by ClawHub (malware detected)".to_string(),
            423 => "Skill is pending security review, try again later".to_string(),
            429 => {
                let retry_after = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("60");
                format!(
                    "ClawHub rate limit exceeded. Retry after {} seconds",
                    retry_after
                )
            }
            _ => {
                let body = resp
                    .text()
                    .await
                    .unwrap_or_default();
                let detail = if body.is_empty() {
                    String::new()
                } else {
                    format!(": {}", body.chars().take(200).collect::<String>())
                };
                format!("ClawHub API error: HTTP {} ({}){}", status, context, detail)
            }
        };

        Err(AlephError::network(err_msg))
    }
}

impl Default for ClawHubClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_newer_version_semver() {
        assert!(ClawHubClient::is_newer_version("1.0.0", "1.1.0"));
        assert!(ClawHubClient::is_newer_version("1.0.0", "2.0.0"));
        assert!(!ClawHubClient::is_newer_version("1.1.0", "1.0.0"));
        assert!(!ClawHubClient::is_newer_version("1.0.0", "1.0.0"));
    }

    #[test]
    fn test_is_newer_version_non_semver() {
        // Falls back to string inequality
        assert!(ClawHubClient::is_newer_version("v1", "v2"));
        assert!(!ClawHubClient::is_newer_version("v1", "v1"));
    }

    #[test]
    fn test_default_client() {
        let client = ClawHubClient::new();
        assert_eq!(client.base_url, "https://clawhub.ai");
    }

    #[test]
    fn test_custom_registry() {
        let client = ClawHubClient::with_registry("https://my-clawhub.com/");
        assert_eq!(client.base_url, "https://my-clawhub.com");
    }

    #[test]
    fn test_sort_order_api_str() {
        assert_eq!(SortOrder::Downloads.as_api_str(), "downloads");
        assert_eq!(SortOrder::Stars.as_api_str(), "stars");
        assert_eq!(SortOrder::Updated.as_api_str(), "updated");
        assert_eq!(SortOrder::Trending.as_api_str(), "trending");
    }
}
