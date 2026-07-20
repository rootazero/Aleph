# ClawHub Integration Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable Aleph to search, browse, install, and update skills from ClawHub (clawhub.ai) via LLM tools and Panel UI.

**Architecture:** Thin HTTP client (`src/clawhub/`) shared by a single builtin tool (`clawhub` with action enum) and gateway RPC handlers. Skill parser extended to support `openclaw` metadata namespace.

**Tech Stack:** Rust, reqwest, serde, semver (new dep), zip/chrono/uuid (existing deps), Leptos (Panel WASM)

**Spec:** `docs/superpowers/specs/2026-03-18-clawhub-integration-design.md`

**Spec deviations:** The spec defines 3 separate tools; this plan uses a single tool with action enum (consistent with `CronManageTool` pattern). The spec injects `ClawHubClient` via `BuiltinToolConfig`; this plan creates the client internally for simplicity (no config wiring needed). Both are conscious simplifications for v1.

---

## File Structure

| Action | Path | Responsibility |
|--------|------|---------------|
| **NEW** | `src/clawhub/mod.rs` | Module entry, re-exports |
| **NEW** | `src/clawhub/client.rs` | HTTP client for ClawHub API |
| **NEW** | `src/clawhub/types.rs` | Request/response types |
| **NEW** | `src/builtin_tools/clawhub.rs` | Single builtin tool with action enum |
| **NEW** | `src/gateway/handlers/clawhub.rs` | 4 RPC handlers for Panel |
| **MOD** | `src/lib.rs:109` | Add `pub mod clawhub;` |
| **MOD** | `src/builtin_tools/mod.rs:70,128` | Add module + re-exports |
| **MOD** | `src/executor/builtin_registry/definitions.rs` | Register tool |
| **MOD** | `src/executor/builtin_registry/groups.rs` | Add to tool group |
| **MOD** | `src/executor/builtin_registry/registry.rs` | Tool field + execute match |
| **MOD** | `src/gateway/handlers/mod.rs:84,268` | Add module + register handlers |
| **MOD** | `src/tools/markdown_skill/spec.rs` | Add `OpenClawMetadata` (parser.rs needs no changes — `serde(default)` handles it) |
| **MOD** | `Cargo.toml` | Add `semver` dependency (zip/chrono/uuid already present) |

---

## Chunk 1: ClawHub Types & HTTP Client

### Task 1: ClawHub response types

**Files:**
- Create: `src/clawhub/types.rs`
- Create: `src/clawhub/mod.rs`
- Modify: `src/lib.rs:109`

- [ ] **Step 1: Create `src/clawhub/types.rs`**

```rust
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
```

- [ ] **Step 2: Create `src/clawhub/mod.rs`**

```rust
//! ClawHub integration — HTTP client and types for clawhub.ai skill registry.

pub mod client;
pub mod types;

pub use client::ClawHubClient;
pub use types::*;
```

- [ ] **Step 3: Add `pub mod clawhub;` to `src/lib.rs`**

Insert after the `pub mod cron;` line (~line 109):

```rust
pub mod clawhub;
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -20`
Expected: Warning about unused `client` module (not yet created), no errors.

- [ ] **Step 5: Commit**

```bash
git add src/clawhub/ src/lib.rs
git commit -m "clawhub: add API response types and module structure"
```

---

### Task 2: ClawHub HTTP client

**Files:**
- Create: `src/clawhub/client.rs`
- Modify: `Cargo.toml` (add `semver` dependency)

- [ ] **Step 1: Add `semver` to `Cargo.toml`**

In `[dependencies]` section, add:

```toml
semver = "1"
```

Note: `zip`, `chrono`, and `uuid` are already in `Cargo.toml`. Only `semver` needs to be added.

- [ ] **Step 2: Create `src/clawhub/client.rs`**

```rust
//! ClawHub HTTP client — thin wrapper around clawhub.ai public API.
//!
//! All endpoints are public (no authentication required).
//! API contract verified from OpenClaw source code and clawhub CLI v0.7.0.

use std::path::PathBuf;
use std::time::Duration;

use reqwest::Client;
use tracing::{debug, warn};

use crate::error::{AlephError, Result};

use super::types::*;

const DEFAULT_REGISTRY: &str = "https://clawhub.ai";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

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
    /// Create a new client with default registry (clawhub.ai)
    pub fn new() -> Self {
        Self::with_registry(DEFAULT_REGISTRY)
    }

    /// Create a client pointing to a custom registry URL
    pub fn with_registry(url: &str) -> Self {
        let http = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent("aleph/0.1")
            .build()
            .unwrap_or_default();

        Self {
            base_url: url.trim_end_matches('/').to_string(),
            http,
        }
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

        let resp = Self::check_status(resp, context)?;

        resp.json()
            .await
            .map_err(|e| AlephError::network(format!("ClawHub {} parse error: {}", context, e)))
    }

    /// Search skills by keyword
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SkillSearchResult>> {
        let url = format!("{}/api/v1/search", self.base_url);
        debug!(query, limit, "ClawHub search");

        let hits: Vec<SearchHit> = self
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

        Ok(hits.into_iter().map(SkillSearchResult::from).collect())
    }

    /// Browse skills with sorting and pagination
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

        let api_resp: BrowseApiResponse =
            self.get_json(&url, &params, "browse").await?;

        Ok(BrowseResponse {
            skills: api_resp
                .skills
                .into_iter()
                .map(SkillSearchResult::from)
                .collect(),
            cursor: api_resp.cursor,
            has_more: api_resp.has_more,
        })
    }

    /// Get skill detail by slug
    pub async fn get_skill(&self, slug: &str) -> Result<SkillDetail> {
        let url = format!("{}/api/v1/skills/{}", self.base_url, slug);
        debug!(slug, "ClawHub get_skill");
        self.get_json(&url, &[], "get_skill").await
    }

    /// Get version list for a skill
    pub async fn get_versions(&self, slug: &str) -> Result<Vec<VersionInfo>> {
        let url = format!("{}/api/v1/skills/{}/versions", self.base_url, slug);
        debug!(slug, "ClawHub get_versions");
        let data: VersionsResponse = self.get_json(&url, &[], "get_versions").await?;
        Ok(data.versions)
    }

    /// Download skill ZIP to a temporary file. Returns path to the temp ZIP.
    pub async fn download(
        &self,
        slug: &str,
        version: Option<&str>,
    ) -> Result<PathBuf> {
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

        let resp = Self::check_status(resp, "download")?;

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| AlephError::network(format!("ClawHub download read error: {}", e)))?;

        let temp_path = std::env::temp_dir()
            .join(format!("clawhub-{}-{}.zip", slug, uuid::Uuid::new_v4()));

        std::fs::write(&temp_path, &bytes).map_err(|e| {
            AlephError::config(format!("Failed to write temp ZIP: {}", e))
        })?;

        Ok(temp_path)
    }

    /// Compare versions: returns true if `remote` is newer than `local`.
    /// Falls back to string inequality if semver parsing fails.
    pub fn is_newer_version(local: &str, remote: &str) -> bool {
        match (semver::Version::parse(local), semver::Version::parse(remote)) {
            (Ok(l), Ok(r)) => r > l,
            _ => {
                warn!(
                    local,
                    remote,
                    "Non-semver version strings, falling back to string compare"
                );
                local != remote
            }
        }
    }

    /// Check HTTP response status. Consumes and returns the response on success.
    fn check_status(resp: reqwest::Response, context: &str) -> Result<reqwest::Response> {
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
            _ => format!("ClawHub API error: HTTP {} ({})", status, context),
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
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -20`
Expected: PASS (no errors)

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib clawhub 2>&1 | tail -20`
Expected: All tests pass

- [ ] **Step 5: Commit**

```bash
git add src/clawhub/client.rs Cargo.toml
git commit -m "clawhub: add HTTP client with search, browse, download, version compare"
```

---

## Chunk 2: Builtin Tool & Registration

### Task 3: ClawHub builtin tool (single tool with action enum)

**Files:**
- Create: `src/builtin_tools/clawhub.rs`

- [ ] **Step 1: Create `src/builtin_tools/clawhub.rs`**

```rust
//! ClawHub tool — search, install, and update skills from clawhub.ai.
//!
//! Single tool with action enum (consistent with CronManageTool pattern).
//! Uses ClawHubClient for all API calls. Installs to ~/.aleph/skills/{slug}/.

use std::io::Read;
use std::path::PathBuf;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::clawhub::{ClawHubClient, ClawHubMeta, SkillSearchResult, SortOrder};
use crate::error::{AlephError, Result};
use crate::skills::Skill;
use crate::tools::AlephTool;

// =============================================================================
// Args
// =============================================================================

/// Action to perform on ClawHub
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClawHubAction {
    /// Search for skills by keyword
    Search,
    /// Browse popular/trending skills
    Browse,
    /// Install a skill by slug
    Install,
    /// Update an installed skill to latest version
    Update,
}

/// Arguments for the clawhub tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ClawHubArgs {
    /// Action to perform
    pub action: ClawHubAction,

    // ── Search/Browse fields ──────────────────────────────────────
    /// Search query (required for search)
    #[serde(default)]
    pub query: Option<String>,

    /// Sort order for browse (default: downloads)
    #[serde(default)]
    pub sort: Option<String>,

    /// Max results to return (default: 10)
    #[serde(default = "default_limit")]
    pub limit: usize,

    /// Pagination cursor for browse
    #[serde(default)]
    pub cursor: Option<String>,

    // ── Install/Update fields ─────────────────────────────────────
    /// Skill slug (required for install/update)
    #[serde(default)]
    pub slug: Option<String>,

    /// Specific version to install (default: latest)
    #[serde(default)]
    pub version: Option<String>,
}

fn default_limit() -> usize {
    10
}

// =============================================================================
// Output
// =============================================================================

/// Output from clawhub tool
#[derive(Debug, Clone, Serialize)]
pub struct ClawHubOutput {
    /// Human-readable status message
    pub message: String,
    /// Search/browse results
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<SkillSearchResult>>,
    /// Pagination cursor for next page
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Whether more results are available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_more: Option<bool>,
    /// Installed/updated skill slug
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_slug: Option<String>,
    /// Installed/updated version
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_version: Option<String>,
    /// Warning (e.g., suspicious skill)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool for searching, installing, and updating skills from ClawHub.
#[derive(Clone)]
pub struct ClawHubTool {
    client: ClawHubClient,
}

impl ClawHubTool {
    pub fn new() -> Self {
        Self {
            client: ClawHubClient::new(),
        }
    }

    pub fn with_client(client: ClawHubClient) -> Self {
        Self { client }
    }

    /// Get the skills directory path
    fn skills_dir() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".aleph")
            .join("skills")
    }

    /// Read .clawhub.json metadata from an installed skill
    fn read_meta(slug: &str) -> Option<ClawHubMeta> {
        let meta_path = Self::skills_dir().join(slug).join(".clawhub.json");
        std::fs::read_to_string(&meta_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
    }

    /// Write .clawhub.json metadata for an installed skill
    fn write_meta(slug: &str, meta: &ClawHubMeta) -> Result<()> {
        let meta_path = Self::skills_dir().join(slug).join(".clawhub.json");
        let json = serde_json::to_string_pretty(meta)
            .map_err(|e| AlephError::config(format!("Failed to serialize meta: {}", e)))?;
        std::fs::write(&meta_path, json)
            .map_err(|e| AlephError::config(format!("Failed to write .clawhub.json: {}", e)))
    }

    /// Install a skill from a downloaded ZIP
    fn install_from_zip(
        &self,
        slug: &str,
        zip_path: &PathBuf,
        version: &str,
        owner: &str,
    ) -> Result<PathBuf> {
        let skills_dir = Self::skills_dir();
        std::fs::create_dir_all(&skills_dir).map_err(|e| {
            AlephError::config(format!("Failed to create skills dir: {}", e))
        })?;

        let target_dir = skills_dir.join(slug);
        let backup_dir = skills_dir.join(format!("{}.bak", slug));

        // Backup existing if present
        if target_dir.exists() {
            if backup_dir.exists() {
                let _ = std::fs::remove_dir_all(&backup_dir);
            }
            std::fs::rename(&target_dir, &backup_dir).map_err(|e| {
                AlephError::config(format!("Failed to backup existing skill: {}", e))
            })?;
        }

        // Extract ZIP
        let file = std::fs::File::open(zip_path).map_err(|e| {
            AlephError::config(format!("Failed to open ZIP: {}", e))
        })?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| {
            // Restore backup on failure
            if backup_dir.exists() {
                let _ = std::fs::rename(&backup_dir, &target_dir);
            }
            AlephError::config(format!("Failed to read ZIP: {}", e))
        })?;

        // Create target directory
        std::fs::create_dir_all(&target_dir).map_err(|e| {
            if backup_dir.exists() {
                let _ = std::fs::rename(&backup_dir, &target_dir);
            }
            AlephError::config(format!("Failed to create skill dir: {}", e))
        })?;

        // Extract all files, stripping the top-level directory
        let mut found_skill_md = false;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| {
                AlephError::config(format!("Failed to read ZIP entry: {}", e))
            })?;

            let entry_path = entry.name().to_string();

            // Strip first path component (e.g., "skill-name-1.0.0/SKILL.md" -> "SKILL.md")
            let relative = entry_path
                .split('/')
                .skip(1)
                .collect::<Vec<_>>()
                .join("/");

            if relative.is_empty() {
                continue;
            }

            let out_path = target_dir.join(&relative);

            if entry.is_dir() {
                std::fs::create_dir_all(&out_path).ok();
            } else {
                if let Some(parent) = out_path.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                let mut content = Vec::new();
                entry.read_to_end(&mut content).map_err(|e| {
                    AlephError::config(format!("Failed to read ZIP file {}: {}", relative, e))
                })?;
                std::fs::write(&out_path, &content).map_err(|e| {
                    AlephError::config(format!("Failed to write {}: {}", relative, e))
                })?;

                if relative == "SKILL.md" {
                    found_skill_md = true;
                }
            }
        }

        // Validate SKILL.md exists and parses
        if !found_skill_md {
            let _ = std::fs::remove_dir_all(&target_dir);
            if backup_dir.exists() {
                let _ = std::fs::rename(&backup_dir, &target_dir);
            }
            return Err(AlephError::config(format!(
                "Invalid skill package: no SKILL.md found for '{}'",
                slug
            )));
        }

        let skill_md_path = target_dir.join("SKILL.md");
        let content = std::fs::read_to_string(&skill_md_path).map_err(|e| {
            AlephError::config(format!("Failed to read SKILL.md: {}", e))
        })?;

        if let Err(e) = Skill::parse(slug, &content) {
            let _ = std::fs::remove_dir_all(&target_dir);
            if backup_dir.exists() {
                let _ = std::fs::rename(&backup_dir, &target_dir);
            }
            return Err(AlephError::config(format!(
                "Invalid SKILL.md in '{}': {}",
                slug, e
            )));
        }

        // Write metadata
        let meta = ClawHubMeta {
            slug: slug.to_string(),
            version: version.to_string(),
            registry: self.client.base_url().to_string(),
            installed_at: chrono::Utc::now().to_rfc3339(),
            owner: owner.to_string(),
        };
        Self::write_meta(slug, &meta)?;

        // Remove backup on success
        if backup_dir.exists() {
            let _ = std::fs::remove_dir_all(&backup_dir);
        }

        info!(slug, version, "ClawHub skill installed");
        Ok(target_dir)
    }
}

impl Default for ClawHubTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AlephTool for ClawHubTool {
    const NAME: &'static str = "clawhub";
    const DESCRIPTION: &'static str =
        "Search, browse, install, and update skills from ClawHub (clawhub.ai) skill registry. \
         Use this when the user wants to find new skills, install a skill by name, \
         or update an installed ClawHub skill.";

    type Args = ClawHubArgs;
    type Output = ClawHubOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            r#"clawhub(action="search", query="github pr review")"#.to_string(),
            r#"clawhub(action="browse", sort="downloads", limit=10)"#.to_string(),
            r#"clawhub(action="install", slug="sonoscli")"#.to_string(),
            r#"clawhub(action="install", slug="sonoscli", version="1.2.0")"#.to_string(),
            r#"clawhub(action="update", slug="sonoscli")"#.to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match args.action {
            ClawHubAction::Search => {
                let query = args.query.ok_or_else(|| {
                    AlephError::tool("clawhub search: 'query' is required")
                })?;

                let results = self.client.search(&query, args.limit).await?;
                let count = results.len();

                Ok(ClawHubOutput {
                    message: format!("找到 {} 个匹配的技能", count),
                    skills: Some(results),
                    cursor: None,
                    has_more: None,
                    installed_slug: None,
                    installed_version: None,
                    warning: None,
                })
            }

            ClawHubAction::Browse => {
                let sort = args
                    .sort
                    .as_deref()
                    .map(|s| match s {
                        "stars" => SortOrder::Stars,
                        "updated" => SortOrder::Updated,
                        "trending" => SortOrder::Trending,
                        _ => SortOrder::Downloads,
                    })
                    .unwrap_or(SortOrder::Downloads);

                let resp = self
                    .client
                    .browse(sort, args.limit, args.cursor.as_deref())
                    .await?;

                let count = resp.skills.len();

                Ok(ClawHubOutput {
                    message: format!("获取了 {} 个技能", count),
                    skills: Some(resp.skills),
                    cursor: resp.cursor,
                    has_more: Some(resp.has_more),
                    installed_slug: None,
                    installed_version: None,
                    warning: None,
                })
            }

            ClawHubAction::Install => {
                let slug = args.slug.ok_or_else(|| {
                    AlephError::tool("clawhub install: 'slug' is required")
                })?;

                // Check moderation status
                let detail = self.client.get_skill(&slug).await?;

                let mut warning = None;
                if let Some(ref moderation) = detail.moderation {
                    if moderation.is_malware_blocked() {
                        return Err(AlephError::tool(format!(
                            "Skill '{}' is blocked by ClawHub: malware detected. Installation refused.",
                            slug
                        )));
                    }
                    if moderation.is_suspicious() {
                        warning = Some(format!(
                            "Warning: Skill '{}' is flagged as suspicious by ClawHub security scan. Proceed with caution.",
                            slug
                        ));
                        warn!(slug = %slug, "Installing suspicious ClawHub skill");
                    }
                }

                let version = args
                    .version
                    .or_else(|| detail.latest_version.as_ref().map(|v| v.number.clone()))
                    .unwrap_or_else(|| "latest".to_string());

                let owner = detail
                    .owner
                    .as_ref()
                    .map(|o| o.handle.clone())
                    .unwrap_or_default();

                // Download
                let zip_path = self
                    .client
                    .download(&slug, Some(&version))
                    .await?;

                // Install from ZIP
                let result =
                    self.install_from_zip(&slug, &zip_path, &version, &owner);

                // Clean up temp file
                let _ = std::fs::remove_file(&zip_path);

                let _target_dir = result?;

                // TODO: Emit EventBus event to refresh local skills list in Panel
                // event_bus.emit(Event::SkillsChanged).await;

                Ok(ClawHubOutput {
                    message: format!(
                        "技能 '{}' (v{}) 已安装到 ~/.aleph/skills/{}/",
                        slug, version, slug
                    ),
                    skills: None,
                    cursor: None,
                    has_more: None,
                    installed_slug: Some(slug),
                    installed_version: Some(version),
                    warning,
                })
            }

            ClawHubAction::Update => {
                let slug = args.slug.ok_or_else(|| {
                    AlephError::tool("clawhub update: 'slug' is required")
                })?;

                // Read local metadata
                let meta = Self::read_meta(&slug).ok_or_else(|| {
                    AlephError::tool(format!(
                        "Skill '{}' is not installed from ClawHub (no .clawhub.json found)",
                        slug
                    ))
                })?;

                // Get remote versions
                let versions = self.client.get_versions(&slug).await?;
                let latest = versions.first().ok_or_else(|| {
                    AlephError::tool(format!("No versions found for skill '{}'", slug))
                })?;

                if !ClawHubClient::is_newer_version(&meta.version, &latest.number) {
                    return Ok(ClawHubOutput {
                        message: format!(
                            "技能 '{}' 已是最新版本 (v{})",
                            slug, meta.version
                        ),
                        skills: None,
                        cursor: None,
                        has_more: None,
                        installed_slug: Some(slug),
                        installed_version: Some(meta.version),
                        warning: None,
                    });
                }

                // Download and install new version
                let zip_path = self
                    .client
                    .download(&slug, Some(&latest.number))
                    .await?;

                let result = self.install_from_zip(
                    &slug,
                    &zip_path,
                    &latest.number,
                    &meta.owner,
                );

                let _ = std::fs::remove_file(&zip_path);
                result?;

                Ok(ClawHubOutput {
                    message: format!(
                        "技能 '{}' 已更新: v{} → v{}",
                        slug, meta.version, latest.number
                    ),
                    skills: None,
                    cursor: None,
                    has_more: None,
                    installed_slug: Some(slug),
                    installed_version: Some(latest.number.clone()),
                    warning: None,
                })
            }
        }
    }
}
```

**Note:** The `client.base_url()` method needs to be exposed. Add this to `client.rs`:

```rust
/// Get the registry base URL
pub fn base_url(&self) -> &str {
    &self.base_url
}
```

- [ ] **Step 2: Add module declaration to `src/builtin_tools/mod.rs`**

After the line `pub mod cron_manage;` (~line 70), add:

```rust
pub mod clawhub;
```

After the `pub use cron_manage::...` line (~line 128), add:

```rust
pub use clawhub::{ClawHubAction, ClawHubArgs, ClawHubOutput, ClawHubTool};
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -20`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/builtin_tools/clawhub.rs src/builtin_tools/mod.rs
git commit -m "clawhub: add builtin tool with search/browse/install/update actions"
```

---

### Task 4: Register tool in executor

**Files:**
- Modify: `src/executor/builtin_registry/definitions.rs`
- Modify: `src/executor/builtin_registry/groups.rs`
- Modify: `src/executor/builtin_registry/registry.rs`

- [ ] **Step 1: Add to `definitions.rs` — BUILTIN_TOOL_DEFINITIONS array**

Before the closing `];` of `BUILTIN_TOOL_DEFINITIONS`, add:

```rust
    BuiltinToolDefinition {
        name: "clawhub",
        description: "Search, browse, install, and update skills from ClawHub registry",
        requires_config: false,
    },
```

- [ ] **Step 2: Add to `definitions.rs` — `create_tool_boxed()` match**

Add a match arm:

```rust
        "clawhub" => Some(Box::new(crate::builtin_tools::clawhub::ClawHubTool::new())),
```

- [ ] **Step 3: Add to `groups.rs` — TOOL_GROUPS**

Add `"clawhub"` to the `"agent_mgmt"` group's tools array:

```rust
            "cron_manage",
            "clawhub",
```

- [ ] **Step 4: Add to `registry.rs` — struct field**

After the `acp_switch_tool` field, add:

```rust
    /// ClawHub tool instance
    pub(crate) clawhub_tool: crate::builtin_tools::clawhub::ClawHubTool,
```

- [ ] **Step 5: Add to `registry.rs` — `with_config()` initialization**

In the `with_config()` method, initialize the field:

```rust
            clawhub_tool: crate::builtin_tools::clawhub::ClawHubTool::new(),
```

- [ ] **Step 6: Add to `registry.rs` — `execute_tool()` match**

Before the final `_ =>` arm, add:

```rust
            "clawhub" => Box::pin(async move { self.clawhub_tool.call_json(arguments).await }),
```

- [ ] **Step 7: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -20`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add src/executor/builtin_registry/
git commit -m "clawhub: register tool in executor builtin registry"
```

---

## Chunk 3: Gateway RPC Handlers

### Task 5: ClawHub RPC handlers for Panel

**Files:**
- Create: `src/gateway/handlers/clawhub.rs`
- Modify: `src/gateway/handlers/mod.rs`

- [ ] **Step 1: Create `src/gateway/handlers/clawhub.rs`**

```rust
//! ClawHub RPC Handlers
//!
//! Handlers for Panel UI: search, browse, install, detail.
//! Uses shared ClawHubClient instance.

use serde::Deserialize;
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use super::parse_params;
use crate::clawhub::ClawHubClient;
use crate::builtin_tools::clawhub::ClawHubTool;

// Shared client and tool — lazily initialized, lives for the process lifetime
fn get_client() -> &'static ClawHubClient {
    use std::sync::OnceLock;
    static CLIENT: OnceLock<ClawHubClient> = OnceLock::new();
    CLIENT.get_or_init(ClawHubClient::new)
}

fn get_tool() -> &'static ClawHubTool {
    use std::sync::OnceLock;
    static TOOL: OnceLock<ClawHubTool> = OnceLock::new();
    TOOL.get_or_init(ClawHubTool::new)
}

// ============================================================================
// Search
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    pub query: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    20
}

pub async fn handle_search(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: SearchParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let client = get_client();
    match client.search(&params.query, params.limit).await {
        Ok(results) => JsonRpcResponse::success(request.id, json!({ "skills": results })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("ClawHub search failed: {}", e),
        ),
    }
}

// ============================================================================
// Browse
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct BrowseParams {
    #[serde(default = "default_sort")]
    pub sort: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
    pub cursor: Option<String>,
}

fn default_sort() -> String {
    "downloads".to_string()
}

pub async fn handle_browse(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: BrowseParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let sort = match params.sort.as_str() {
        "stars" => crate::clawhub::SortOrder::Stars,
        "updated" => crate::clawhub::SortOrder::Updated,
        "trending" => crate::clawhub::SortOrder::Trending,
        _ => crate::clawhub::SortOrder::Downloads,
    };

    let client = get_client();
    match client
        .browse(sort, params.limit, params.cursor.as_deref())
        .await
    {
        Ok(resp) => JsonRpcResponse::success(
            request.id,
            json!({
                "skills": resp.skills,
                "cursor": resp.cursor,
                "hasMore": resp.has_more,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("ClawHub browse failed: {}", e),
        ),
    }
}

// ============================================================================
// Detail
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct DetailParams {
    pub slug: String,
}

pub async fn handle_detail(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: DetailParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let client = get_client();
    match client.get_skill(&params.slug).await {
        Ok(detail) => JsonRpcResponse::success(request.id, json!({ "skill": detail })),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("ClawHub detail failed: {}", e),
        ),
    }
}

// ============================================================================
// Install
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct InstallParams {
    pub slug: String,
    pub version: Option<String>,
}

pub async fn handle_install(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: InstallParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Use shared tool instance for consistency
    let tool = get_tool();
    let args = crate::builtin_tools::clawhub::ClawHubArgs {
        action: crate::builtin_tools::clawhub::ClawHubAction::Install,
        query: None,
        sort: None,
        limit: 10,
        cursor: None,
        slug: Some(params.slug),
        version: params.version,
    };

    match crate::tools::AlephTool::call(&tool, args).await {
        Ok(output) => {
            let mut result = json!({
                "message": output.message,
            });
            if let Some(slug) = output.installed_slug {
                result["slug"] = json!(slug);
            }
            if let Some(version) = output.installed_version {
                result["version"] = json!(version);
            }
            if let Some(warning) = output.warning {
                result["warning"] = json!(warning);
            }
            JsonRpcResponse::success(request.id, result)
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("ClawHub install failed: {}", e),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_params() {
        let json = json!({"query": "github"});
        let params: SearchParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.query, "github");
        assert_eq!(params.limit, 20);
    }

    #[test]
    fn test_browse_params_defaults() {
        let json = json!({});
        let params: BrowseParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.sort, "downloads");
        assert_eq!(params.limit, 20);
        assert!(params.cursor.is_none());
    }

    #[test]
    fn test_install_params() {
        let json = json!({"slug": "sonoscli", "version": "1.2.0"});
        let params: InstallParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.slug, "sonoscli");
        assert_eq!(params.version, Some("1.2.0".to_string()));
    }
}
```

- [ ] **Step 2: Register in `src/gateway/handlers/mod.rs`**

Add module declaration after `pub mod cron;` (~line 84):

```rust
pub mod clawhub;
```

Add handler registrations after the skills handlers block (~line 268):

```rust
        // ClawHub handlers
        registry.register("clawhub.search", clawhub::handle_search);
        registry.register("clawhub.browse", clawhub::handle_browse);
        registry.register("clawhub.detail", clawhub::handle_detail);
        registry.register("clawhub.install", clawhub::handle_install);
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p alephcore 2>&1 | head -20`
Expected: PASS

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib clawhub 2>&1 | tail -20`
Expected: All tests pass

- [ ] **Step 5: Commit**

```bash
git add src/gateway/handlers/clawhub.rs src/gateway/handlers/mod.rs
git commit -m "clawhub: add gateway RPC handlers for Panel UI"
```

---

## Chunk 4: Skill Format Compatibility

### Task 6: Add OpenClaw metadata namespace to skill parser

**Files:**
- Modify: `src/tools/markdown_skill/spec.rs`

- [ ] **Step 1: Add `OpenClawMetadata` struct to `spec.rs`**

After the existing `AlephExtensions` struct, add:

```rust
/// OpenClaw metadata namespace — compatible with ClawHub skill format.
///
/// Allows SKILL.md files from ClawHub to work natively in Aleph.
/// Both `aleph` and `openclaw` namespaces can coexist.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenClawMetadata {
    #[serde(default)]
    pub emoji: Option<String>,
    #[serde(default, rename = "primaryEnv")]
    pub primary_env: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub os: Option<Vec<String>>,
    #[serde(default)]
    pub always: Option<bool>,
    #[serde(default)]
    pub install: Option<Vec<OpenClawInstallSpec>>,
}

/// OpenClaw install specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenClawInstallSpec {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub formula: Option<String>,
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub bins: Option<Vec<String>>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub os: Option<Vec<String>>,
}
```

- [ ] **Step 2: Add `openclaw` field to `SkillMetadata`**

In the `SkillMetadata` struct, add after the `aleph` field:

```rust
    /// OpenClaw metadata namespace (ClawHub compatibility)
    #[serde(default)]
    pub openclaw: Option<OpenClawMetadata>,
```

- [ ] **Step 3: Verify compilation and existing tests pass**

Run: `cargo test -p alephcore --lib markdown_skill 2>&1 | tail -20`
Expected: Existing tests still pass (serde(default) ensures backward compatibility)

- [ ] **Step 4: Commit**

```bash
git add src/tools/markdown_skill/spec.rs
git commit -m "clawhub: add OpenClaw metadata namespace to skill parser"
```

---

## Chunk 5: Integration Test & Final Wiring

### Task 7: Full integration check

- [ ] **Step 1: Run full compilation**

Run: `cargo check -p alephcore 2>&1 | tail -30`
Expected: PASS with no errors

- [ ] **Step 2: Run all core tests**

Run: `cargo test -p alephcore --lib 2>&1 | tail -30`
Expected: All tests pass (pre-existing `markdown_skill::loader` failures excluded)

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings 2>&1 | tail -30`
Expected: No new warnings

- [ ] **Step 4: Final commit if any fixes needed**

```bash
git add -A
git commit -m "clawhub: fix clippy warnings and integration issues"
```

---

## Chunk 6: Panel UI (ClawHub Tab) — FOLLOW-UP

> **Note:** This chunk is a **separate follow-up** after core backend (Chunks 1-5) is complete and verified with a running Aleph instance. It requires examining the existing Panel Leptos component structure in detail and following its specific patterns. The RPC handlers from Chunk 3 provide the backend.

### Task 8: Panel ClawHub tab

**Files:**
- Create: `apps/panel/src/clawhub/` (component directory)
- Modify: Panel routing/tabs to add ClawHub tab

This task requires examining the existing Panel component structure and following its patterns. Key requirements:

1. New "ClawHub" tab alongside existing Skills tab
2. `SearchBar` component with 300ms debounce → `clawhub.search` RPC
3. `SkillGrid` component with `SkillCard` children
4. Each `SkillCard`: name, summary, tags, downloads/stars counts, install button
5. Install button states: Not installed / Installing (spinner) / Installed (greyed)
6. Cross-reference local skills list to detect installed status
7. Pagination via "Load more" button using cursor
8. Error states: network error banner, empty search results, malware badge

**Implementation approach:** Follow existing Panel tab patterns. The Panel already has a skills list — add a parallel ClawHub tab using the same layout conventions.

- [ ] **Step 1: Examine existing Panel skills tab structure**
- [ ] **Step 2: Create ClawHub tab component**
- [ ] **Step 3: Add RPC client calls for clawhub.* methods**
- [ ] **Step 4: Implement search with debounce**
- [ ] **Step 5: Implement skill grid with install flow**
- [ ] **Step 6: Add error states**
- [ ] **Step 7: Build and verify**

Run: `just build` (or equivalent Panel build command)

- [ ] **Step 8: Commit**

```bash
git add apps/panel/
git commit -m "panel: add ClawHub skill marketplace tab"
```
