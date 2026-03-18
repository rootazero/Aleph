//! ClawHub API types — request/response models for clawhub.ai REST API.

use serde::{Deserialize, Serialize};

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
    pub fn as_api_str(&self) -> &'static str {
        match self {
            Self::Downloads => "downloads",
            Self::Stars => "stars",
            Self::Updated => "updated",
            Self::Trending => "trending",
        }
    }
}

/// A skill from search or browse results
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
}

/// Paginated browse response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowseResponse {
    pub skills: Vec<SkillSearchResult>,
    pub cursor: Option<String>,
    #[serde(default)]
    pub has_more: bool,
}

/// Skill detail
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
    #[serde(rename = "latestVersion")]
    pub latest_version: Option<VersionInfo>,
    #[serde(rename = "moderationInfo")]
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
}

impl ModerationInfo {
    pub fn is_malware_blocked(&self) -> bool {
        self.verdict == "malware"
    }

    pub fn is_suspicious(&self) -> bool {
        self.verdict == "suspicious"
    }
}

/// Version info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    #[serde(alias = "number")]
    pub number: String,
    #[serde(default)]
    pub changelog: String,
    /// RFC 3339 timestamp
    #[serde(default, rename = "publishedAt")]
    pub published_at: String,
    #[serde(default)]
    pub files: Vec<String>,
}

/// Versions list response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionsResponse {
    pub versions: Vec<VersionInfo>,
}

/// Owner info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnerInfo {
    #[serde(default)]
    pub handle: String,
    #[serde(default, rename = "displayName")]
    pub display_name: String,
}

/// Local metadata for installed ClawHub skills
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

/// Search API raw response (array of search hits)
#[derive(Debug, Clone, Deserialize)]
pub struct SearchHit {
    pub skill: SearchHitSkill,
    #[serde(default, rename = "ownerHandle")]
    pub owner_handle: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchHitSkill {
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub stats: Option<SkillStats>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SkillStats {
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub stars: u64,
}

impl From<SearchHit> for SkillSearchResult {
    fn from(hit: SearchHit) -> Self {
        let (downloads, stars) = hit
            .skill
            .stats
            .map(|s| (s.downloads, s.stars))
            .unwrap_or((0, 0));
        Self {
            slug: hit.skill.slug,
            name: hit.skill.name,
            summary: hit.skill.summary,
            tags: hit.skill.tags,
            downloads,
            stars,
            owner_handle: hit.owner_handle,
        }
    }
}

/// Browse API raw response
#[derive(Debug, Clone, Deserialize)]
pub struct BrowseApiResponse {
    pub skills: Vec<BrowseSkill>,
    pub cursor: Option<String>,
    #[serde(default, rename = "hasMore")]
    pub has_more: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BrowseSkill {
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub stats: Option<SkillStats>,
    #[serde(default, rename = "ownerHandle")]
    pub owner_handle: String,
}

impl From<BrowseSkill> for SkillSearchResult {
    fn from(s: BrowseSkill) -> Self {
        let (downloads, stars) = s.stats.map(|s| (s.downloads, s.stars)).unwrap_or((0, 0));
        Self {
            slug: s.slug,
            name: s.name,
            summary: s.summary,
            tags: s.tags,
            downloads,
            stars,
            owner_handle: s.owner_handle,
        }
    }
}
