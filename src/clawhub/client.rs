//! `ClawHub` HTTP client — thin wrapper around clawhub.ai public API.
//!
//! All endpoints are public (no authentication required).
//! API contract verified from `OpenClaw` source code and clawhub CLI v0.7.0.

use std::path::PathBuf;
use std::time::Duration;

use futures::StreamExt;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use reqwest::Client;
use serde_json;
use tokio::io::AsyncWriteExt;
use tracing::{debug, warn};

/// Percent-encode set for URL path segments.
/// Keeps unreserved characters (`-`, `_`, `.`, `~`) unencoded per RFC 3986.
const PATH_SEGMENT_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

use crate::error::{AlephError, Result};

use super::types::{
    BrowseApiResponse, BrowseResponse, DetailApiResponse, SearchApiResponse, SkillDetail,
    SkillSearchResult, SortOrder,
};

const DEFAULT_REGISTRY: &str = "https://clawhub.ai";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Maximum download size (100 MiB) to prevent memory exhaustion.
const MAX_DOWNLOAD_BYTES: usize = 100 * 1024 * 1024;
/// Maximum size of a JSON catalog response (8 MiB) — search/browse/detail
/// responses are all KB-scale; anything larger is either a malformed server
/// or an amplification attack. Enforced before `resp.json()` so the body is
/// never fully materialized in memory.
const MAX_JSON_BYTES: usize = 8 * 1024 * 1024;

/// Percent-encode a slug for use in URL path segments.
/// Encodes everything except alphanumerics, `-`, `_`, `.`, `~`, and `/` (slug separator).
///
/// Empty and dot segments (`.` / `..`) are dropped so a crafted slug cannot
/// climb above the intended `/api/v1/skills/{slug}` prefix: the `url` crate
/// normalizes dot-segments, so leaving `..` literal would let
/// `get_skill("../../foo")` resolve to an unrelated endpoint on the registry
/// host. The consumer's `sanitize_skill_name` only guards the filesystem path,
/// not the URL path, so this is the URL-side containment.
fn encode_slug_path(slug: &str) -> String {
    slug.split('/')
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .map(|segment| utf8_percent_encode(segment, PATH_SEGMENT_ENCODE_SET).to_string())
        .collect::<Vec<_>>()
        .join("/")
}

/// HTTP client for `ClawHub` skill registry.
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
    pub fn new() -> Result<Self> {
        let registry = std::env::var("CLAWHUB_REGISTRY")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_REGISTRY.to_string());
        Self::with_registry(&registry)
    }

    /// Create a client pointing to a custom registry URL
    pub fn with_registry(url: &str) -> Result<Self> {
        let ua = format!("aleph/{}", env!("ALEPH_VERSION"));
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .user_agent(ua)
            .build()
            .map_err(|e| AlephError::network(format!("Failed to build HTTP client: {e}")))?;

        Ok(Self {
            base_url: url.trim_end_matches('/').to_string(),
            http,
        })
    }

    /// Get the registry base URL
    #[must_use]
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
            .map_err(|e| AlephError::network(format!("ClawHub {context} failed: {e}")))?;

        let resp = Self::check_status(resp, context).await?;

        // Cap the body before handing it to `resp.json()` so an oversized or
        // adversarial response can't materialize fully in memory. Honest
        // Content-Length is rejected early; the streaming path enforces the
        // same cap incrementally as a defense against missing/dishonest
        // Content-Length headers.
        if let Some(len) = resp.content_length() {
            if len > MAX_JSON_BYTES as u64 {
                return Err(AlephError::network(format!(
                    "ClawHub {context} response exceeds {MAX_JSON_BYTES} byte cap"
                )));
            }
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| AlephError::network(format!("ClawHub {context} read error: {e}")))?;
        if bytes.len() > MAX_JSON_BYTES {
            return Err(AlephError::network(format!(
                "ClawHub {context} response exceeds {MAX_JSON_BYTES} byte cap"
            )));
        }
        serde_json::from_slice(&bytes)
            .map_err(|e| AlephError::network(format!("ClawHub {context} parse error: {e}")))
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

        Ok(resp
            .results
            .into_iter()
            .map(SkillSearchResult::from)
            .collect())
    }

    /// Browse skills with sorting and pagination.
    ///
    /// The `/api/v1/skills` endpoint may return empty results. When it does,
    /// this returns an empty list, preserving `next_cursor` (and reporting
    /// `has_more = true`) when the API still advertises further pages so
    /// callers can continue paginating.
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

        // Fallback: browse endpoint returned empty. Preserve next_cursor if present
        // so callers can continue pagination.
        if api_resp.next_cursor.is_some() {
            warn!(
                cursor = api_resp.next_cursor,
                "Browse returned empty but has next_cursor; pagination may be broken"
            );
            return Ok(BrowseResponse {
                skills: Vec::new(),
                cursor: api_resp.next_cursor,
                has_more: true,
            });
        }
        debug!("Browse returned empty results");
        Ok(BrowseResponse {
            skills: Vec::new(),
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
            .map_err(|e| AlephError::network(format!("ClawHub download failed: {e}")))?;

        let resp = Self::check_status(resp, "download").await?;

        // Reject early when the server declares an oversized body, before
        // reading anything into memory.
        if let Some(len) = resp.content_length() {
            if len > MAX_DOWNLOAD_BYTES as u64 {
                return Err(AlephError::network(format!(
                    "ClawHub download exceeds maximum size ({len} > {MAX_DOWNLOAD_BYTES} bytes)"
                )));
            }
        }

        // Sanitize slug for filename: "owner/skill" → "owner-skill".
        // Keep only filename-safe characters so a crafted slug cannot create
        // subdirectories or use platform-reserved characters.
        let safe_slug: String = slug
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let safe_slug = if safe_slug.len() > 100 {
            safe_slug.chars().take(100).collect::<String>()
        } else {
            safe_slug
        };
        let temp_path = std::env::temp_dir().join(format!(
            "clawhub-{}-{}.zip",
            safe_slug,
            uuid::Uuid::new_v4()
        ));

        // Stream the body straight to the temp file and enforce the size cap
        // incrementally so a missing or dishonest Content-Length cannot exhaust
        // memory.
        let mut file = tokio::fs::File::create(&temp_path)
            .await
            .map_err(|e| AlephError::config(format!("Failed to create temp ZIP: {e}")))?;
        let mut stream = resp.bytes_stream();
        let mut downloaded = 0usize;
        let result: std::result::Result<(), AlephError> = async move {
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| {
                    AlephError::network(format!("ClawHub download read error: {e}"))
                })?;
                if downloaded + chunk.len() > MAX_DOWNLOAD_BYTES {
                    return Err(AlephError::network(format!(
                        "ClawHub download exceeds maximum size (> {MAX_DOWNLOAD_BYTES} bytes)"
                    )));
                }
                file.write_all(&chunk)
                    .await
                    .map_err(|e| AlephError::config(format!("Failed to write temp ZIP: {e}")))?;
                downloaded += chunk.len();
            }
            file.flush()
                .await
                .map_err(|e| AlephError::config(format!("Failed to flush temp ZIP: {e}")))?;
            Ok(())
        }
        .await;
        if let Err(e) = result {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(e);
        }

        Ok(temp_path)
    }

    /// Compare versions: returns true if `remote` is newer than `local`.
    /// Falls back to conservative `false` if semver parsing fails.
    pub fn is_newer_version(local: &str, remote: &str) -> bool {
        match (
            semver::Version::parse(local),
            semver::Version::parse(remote),
        ) {
            (Ok(l), Ok(r)) => r > l,
            _ => {
                warn!(
                    local,
                    remote, "Non-semver version strings, cannot determine if remote is newer"
                );
                false
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

        if status.is_redirection() {
            return Err(AlephError::network(format!(
                "ClawHub {context} rejected redirect response: HTTP {status}"
            )));
        }

        let err_msg = match status.as_u16() {
            404 => format!("Skill not found on ClawHub ({context})"),
            403 => "Skill blocked by ClawHub (malware detected)".to_string(),
            423 => "Skill is pending security review, try again later".to_string(),
            429 => {
                let retry_after = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("60");
                format!("ClawHub rate limit exceeded. Retry after {retry_after} seconds")
            }
            _ => {
                let body = resp
                    .bytes()
                    .await
                    .map(|b| {
                        // Invariant: end is `b.len().min(1024)`, so it is always within bounds.
                        let preview = b
                            .get(..b.len().min(1024))
                            .expect("invariant: slice end <= bytes len");
                        String::from_utf8_lossy(preview).into_owned()
                    })
                    .unwrap_or_default();
                let detail = if body.is_empty() {
                    String::new()
                } else {
                    format!(": {}", body.chars().take(200).collect::<String>())
                };
                format!("ClawHub API error: HTTP {status} ({context}){detail}")
            }
        };

        Err(AlephError::network(err_msg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn clawhub_redirect_is_rejected_without_second_hop() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let second_hop = MockServer::start().await;
        let registry = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/search"))
            .respond_with(ResponseTemplate::new(302).insert_header(
                "location",
                format!("{}/internal", second_hop.uri()).as_str(),
            ))
            .mount(&registry)
            .await;

        let client = ClawHubClient::with_registry(&registry.uri()).unwrap();
        let result = client.search("test", 1).await;
        let requests = second_hop
            .received_requests()
            .await
            .expect("wiremock must record requests");
        let error = result.expect_err("redirect must be rejected").to_string();

        assert!(error.contains("302"), "unexpected error: {error}");
        assert!(requests.is_empty(), "redirect target received a request");
    }

    #[test]
    fn test_is_newer_version_semver() {
        assert!(ClawHubClient::is_newer_version("1.0.0", "1.1.0"));
        assert!(ClawHubClient::is_newer_version("1.0.0", "2.0.0"));
        assert!(!ClawHubClient::is_newer_version("1.1.0", "1.0.0"));
        assert!(!ClawHubClient::is_newer_version("1.0.0", "1.0.0"));
    }

    #[test]
    fn test_is_newer_version_non_semver() {
        // Falls back to conservative false when semver parsing fails
        assert!(!ClawHubClient::is_newer_version("v1", "v2"));
        assert!(!ClawHubClient::is_newer_version("v1", "v1"));
    }

    #[test]
    fn test_default_client() {
        // Use with_registry directly to avoid env var dependency
        let client = ClawHubClient::with_registry("https://clawhub.ai").unwrap();
        assert_eq!(client.base_url, "https://clawhub.ai");
    }

    #[test]
    fn test_custom_registry() {
        let client = ClawHubClient::with_registry("https://my-clawhub.com/").unwrap();
        assert_eq!(client.base_url, "https://my-clawhub.com");
    }

    #[test]
    fn test_sort_order_api_str() {
        assert_eq!(SortOrder::Downloads.as_api_str(), "downloads");
        assert_eq!(SortOrder::Stars.as_api_str(), "stars");
        assert_eq!(SortOrder::Updated.as_api_str(), "updated");
        assert_eq!(SortOrder::Trending.as_api_str(), "trending");
    }

    #[test]
    fn test_encode_slug_path_basic() {
        assert_eq!(encode_slug_path("owner/skill"), "owner/skill");
    }

    #[test]
    fn test_encode_slug_path_with_spaces() {
        assert_eq!(encode_slug_path("owner/my skill"), "owner/my%20skill");
    }

    #[test]
    fn test_encode_slug_path_with_special_chars() {
        assert_eq!(
            encode_slug_path("owner/skill@name#v1.0"),
            "owner/skill%40name%23v1.0"
        );
    }

    #[test]
    fn test_encode_slug_path_filters_empty_segments() {
        assert_eq!(encode_slug_path("owner//skill"), "owner/skill");
        assert_eq!(encode_slug_path("/owner/skill/"), "owner/skill");
        assert_eq!(encode_slug_path("///"), "");
    }

    #[test]
    fn test_encode_slug_path_drops_dot_segments() {
        // `.`/`..` segments are dropped so a crafted slug cannot escape the
        // `/api/v1/skills/` prefix once the url crate normalizes dot-segments.
        assert_eq!(encode_slug_path("../../admin"), "admin");
        assert_eq!(encode_slug_path("owner/../secret"), "owner/secret");
        assert_eq!(encode_slug_path("./owner/./skill"), "owner/skill");
        assert_eq!(encode_slug_path(".."), "");
        // A literal dot inside a legitimate name is preserved (not a segment).
        assert_eq!(
            encode_slug_path("owner/node.js-helper"),
            "owner/node.js-helper"
        );
    }

    #[test]
    fn test_is_newer_version_prerelease() {
        assert!(ClawHubClient::is_newer_version("1.0.0-alpha", "1.0.0"));
        assert!(!ClawHubClient::is_newer_version("1.0.0", "1.0.0-alpha"));
        assert!(ClawHubClient::is_newer_version("1.0.0-beta", "1.0.0-rc1"));
    }

    #[test]
    fn test_is_newer_version_build_metadata() {
        // semver crate compares build metadata lexicographically
        assert!(ClawHubClient::is_newer_version(
            "1.0.0+build1",
            "1.0.0+build2"
        ));
        assert!(!ClawHubClient::is_newer_version(
            "1.0.0+build2",
            "1.0.0+build1"
        ));
    }

    #[test]
    fn test_is_newer_version_empty_strings() {
        assert!(!ClawHubClient::is_newer_version("", ""));
        assert!(!ClawHubClient::is_newer_version("", "v1"));
    }

    #[test]
    fn test_with_registry_trims_trailing_slash() {
        let client = ClawHubClient::with_registry("https://my-hub.com/").unwrap();
        assert_eq!(client.base_url, "https://my-hub.com");
    }

    #[test]
    fn test_with_registry_preserves_path() {
        let client = ClawHubClient::with_registry("https://my-hub.com/api").unwrap();
        assert_eq!(client.base_url, "https://my-hub.com/api");
    }
}
