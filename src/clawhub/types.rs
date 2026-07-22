//! `ClawHub` API types — request/response models for clawhub.ai REST API.

use serde::{Deserialize, Serialize};

/// Convert unix milliseconds timestamp to RFC 3339 string.
/// Returns empty string if the timestamp is invalid.
pub(crate) fn unix_ms_to_rfc3339(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let nanos = ((ms % 1000) * 1_000_000) as u32;
    chrono::DateTime::from_timestamp(secs, nanos)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
}

/// Sort order for browsing skills
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortOrder {
    Downloads,
    Stars,
    Updated,
    Trending,
}

impl SortOrder {
    #[must_use]
    pub const fn as_api_str(&self) -> &'static str {
        match self {
            Self::Downloads => "downloads",
            Self::Stars => "stars",
            Self::Updated => "updated",
            Self::Trending => "trending",
        }
    }
}

/// A skill from search or browse results (unified internal type).
///
/// This is the stable type used by builtin tools and gateway handlers.
/// Raw API responses are converted into this via `From` impls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSearchResult {
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub stars: u64,
    #[serde(default)]
    pub owner_handle: String,
    /// RFC 3339 timestamp when the skill was last updated (from API `updatedAt`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub updated_at: String,
}

/// Paginated browse response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowseResponse {
    pub skills: Vec<SkillSearchResult>,
    pub cursor: Option<String>,
    #[serde(default)]
    pub has_more: bool,
}

/// Skill detail (internal unified type, built from nested API response)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDetail {
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub owner: Option<OwnerInfo>,
    pub latest_version: Option<VersionInfo>,
    pub moderation: Option<ModerationInfo>,
}

/// Moderation info from security scans
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModerationInfo {
    #[serde(default)]
    pub verdict: String,
    #[serde(default, rename = "reasonCodes")]
    pub reason_codes: Vec<String>,
    #[serde(default)]
    pub summary: String,
    #[serde(default, rename = "isSuspicious")]
    pub is_suspicious_flag: bool,
    #[serde(default, rename = "isMalwareBlocked")]
    pub is_malware_blocked_flag: bool,
}

impl ModerationInfo {
    #[must_use]
    pub fn is_malware_blocked(&self) -> bool {
        self.is_malware_blocked_flag || self.verdict.eq_ignore_ascii_case("malware")
    }
}

/// Version info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    /// Version string (e.g. "1.0.0")
    #[serde(alias = "version")]
    pub number: String,
    #[serde(default)]
    pub changelog: String,
    #[serde(default)]
    pub license: String,
    /// Timestamp — may be RFC 3339 string or Unix ms number.
    /// Stored as string for display; converted from number if needed.
    #[serde(default)]
    pub published_at: String,
    #[serde(default)]
    pub files: Vec<String>,
}

/// Owner info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnerInfo {
    #[serde(default)]
    pub handle: String,
    #[serde(default, alias = "displayName")]
    pub display_name: String,
}

/// Local metadata for installed `ClawHub` skills
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClawHubMeta {
    pub slug: String,
    pub version: String,
    pub registry: String,
    /// RFC 3339 timestamp
    pub installed_at: String,
    #[serde(default)]
    pub owner: String,
}

// =============================================================================
// Raw API response types (for deserialization only)
// =============================================================================

/// Search API response: `{ results: [...] }`
#[derive(Debug, Clone, Deserialize)]
pub struct SearchApiResponse {
    #[serde(default)]
    pub results: Vec<SearchHit>,
}

/// A single search hit from the search API
#[derive(Debug, Clone, Deserialize)]
pub struct SearchHit {
    #[serde(default)]
    pub score: f64,
    #[serde(default)]
    pub slug: String,
    #[serde(default, alias = "displayName")]
    pub display_name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default, rename = "updatedAt")]
    pub updated_at: Option<u64>,
}

impl From<SearchHit> for SkillSearchResult {
    fn from(hit: SearchHit) -> Self {
        let updated_at = hit.updated_at.map(unix_ms_to_rfc3339).unwrap_or_default();
        Self {
            slug: hit.slug,
            name: hit.display_name,
            summary: hit.summary,
            tags: Vec::new(),
            downloads: 0,
            stars: 0,
            owner_handle: String::new(),
            updated_at,
        }
    }
}

/// Browse API response: `{ items: [...], nextCursor: ... }`
#[derive(Debug, Clone, Deserialize)]
pub struct BrowseApiResponse {
    #[serde(default)]
    pub items: Vec<BrowseSkill>,
    #[serde(default, rename = "nextCursor")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BrowseSkill {
    #[serde(default)]
    pub slug: String,
    #[serde(default, alias = "displayName")]
    pub display_name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub stats: Option<SkillStats>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SkillStats {
    #[serde(default)]
    pub downloads: u64,
    #[serde(default, rename = "installsAllTime")]
    pub installs_all_time: u64,
    #[serde(default, rename = "installsCurrent")]
    pub installs_current: u64,
    #[serde(default)]
    pub stars: u64,
    #[serde(default)]
    pub comments: u64,
    #[serde(default)]
    pub versions: u64,
}

impl From<BrowseSkill> for SkillSearchResult {
    fn from(s: BrowseSkill) -> Self {
        let (downloads, stars) = s.stats.map_or((0, 0), |s| (s.downloads, s.stars));
        Self {
            slug: s.slug,
            name: s.display_name,
            summary: s.summary,
            tags: Vec::new(),
            downloads,
            stars,
            owner_handle: String::new(),
            updated_at: String::new(),
        }
    }
}

/// Detail API response: `{ skill: {...}, latestVersion: {...}, owner: {...}, moderation: {...} }`
#[derive(Debug, Clone, Deserialize)]
pub struct DetailApiResponse {
    pub skill: DetailApiSkill,
    #[serde(default, rename = "latestVersion")]
    pub latest_version: Option<DetailApiVersion>,
    #[serde(default)]
    pub owner: Option<OwnerInfo>,
    #[serde(default)]
    pub moderation: Option<ModerationInfo>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DetailApiSkill {
    #[serde(default)]
    pub slug: String,
    #[serde(default, alias = "displayName")]
    pub display_name: String,
    #[serde(default)]
    pub summary: String,
    /// Tags is a map like `{"latest": "1.0.0"}` in the actual API
    #[serde(default)]
    pub tags: serde_json::Value,
    #[serde(default)]
    pub stats: Option<SkillStats>,
    #[serde(default, rename = "createdAt")]
    pub created_at: Option<u64>,
    #[serde(default, rename = "updatedAt")]
    pub updated_at: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DetailApiVersion {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub changelog: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default, rename = "createdAt")]
    pub created_at: Option<u64>,
}

impl From<DetailApiResponse> for SkillDetail {
    fn from(resp: DetailApiResponse) -> Self {
        // Extract tag names: if tags is a map, use the keys; if array, try strings
        let tags = match &resp.skill.tags {
            serde_json::Value::Object(map) => map.keys().cloned().collect(),
            serde_json::Value::Array(arr) => arr
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
            _ => Vec::new(),
        };

        let latest_version = resp.latest_version.map(|v| {
            let published_at = v.created_at.map(unix_ms_to_rfc3339).unwrap_or_default();
            VersionInfo {
                number: v.version,
                changelog: v.changelog.unwrap_or_default(),
                license: v.license.unwrap_or_default(),
                published_at,
                files: Vec::new(),
            }
        });

        Self {
            slug: resp.skill.slug,
            name: resp.skill.display_name,
            summary: resp.skill.summary,
            tags,
            owner: resp.owner,
            latest_version,
            moderation: resp.moderation,
        }
    }
}
