//! ClawHub tool — search, browse, install, and update skills from ClawHub registry.
//!
//! Wraps the `ClawHubClient` HTTP client as an LLM-callable builtin tool,
//! letting users manage ClawHub skills through natural language.

use std::io::Read as IoRead;
use std::path::PathBuf;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::clawhub::client::ClawHubClient;
use crate::clawhub::types::{BrowseResponse, ClawHubMeta, SkillSearchResult, SortOrder};
use crate::error::{AlephError, Result};
use crate::tools::AlephTool;

// =============================================================================
// Args
// =============================================================================

/// Action to perform on the ClawHub registry
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClawHubAction {
    /// Search skills by keyword
    Search,
    /// Browse skills with sorting and pagination
    Browse,
    /// Install a skill by slug (optionally at a specific version)
    Install,
    /// Check for and apply updates to an installed skill
    Update,
}

/// Sort order for browsing (mirrors ClawHub API)
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClawHubSortOrder {
    Downloads,
    Stars,
    Updated,
    Trending,
}

impl From<ClawHubSortOrder> for SortOrder {
    fn from(s: ClawHubSortOrder) -> Self {
        match s {
            ClawHubSortOrder::Downloads => SortOrder::Downloads,
            ClawHubSortOrder::Stars => SortOrder::Stars,
            ClawHubSortOrder::Updated => SortOrder::Updated,
            ClawHubSortOrder::Trending => SortOrder::Trending,
        }
    }
}

/// Arguments for the clawhub tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ClawHubArgs {
    /// Action to perform
    pub action: ClawHubAction,

    /// Search query (required for search)
    #[serde(default)]
    pub query: Option<String>,

    /// Sort order for browse (default: downloads)
    #[serde(default)]
    pub sort: Option<ClawHubSortOrder>,

    /// Maximum number of results (default: 20)
    #[serde(default)]
    pub limit: Option<usize>,

    /// Pagination cursor (for browse)
    #[serde(default)]
    pub cursor: Option<String>,

    /// Skill slug (required for install/update, e.g. "owner/skill-name")
    #[serde(default)]
    pub slug: Option<String>,

    /// Specific version to install (optional, defaults to latest)
    #[serde(default)]
    pub version: Option<String>,
}

// =============================================================================
// Output
// =============================================================================

/// Output from clawhub tool
#[derive(Debug, Clone, Serialize)]
pub struct ClawHubOutput {
    /// Human-readable status message
    pub message: String,
    /// Search/browse results (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<SkillSearchResult>>,
    /// Browse pagination info
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_more: Option<bool>,
    /// Installed skill slug (for install/update)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_slug: Option<String>,
    /// Installed version
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_version: Option<String>,
}

// =============================================================================
// Tool
// =============================================================================

/// Tool for searching, browsing, installing, and updating skills from ClawHub.
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

    /// Skills installation directory: ~/.aleph/skills/
    fn skills_dir() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".aleph")
            .join("skills")
    }

    /// Install a skill from a downloaded ZIP file.
    ///
    /// Extracts the ZIP, validates SKILL.md, writes `.clawhub.json` metadata.
    /// Returns the installed version string.
    fn install_from_zip(
        zip_path: &std::path::Path,
        slug: &str,
        version: &str,
        registry_url: &str,
    ) -> Result<String> {
        let file = std::fs::File::open(zip_path).map_err(|e| {
            AlephError::tool(format!("Failed to open ZIP file: {}", e))
        })?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| {
            AlephError::tool(format!("Failed to read ZIP archive: {}", e))
        })?;

        // Determine the skill directory name from slug (e.g. "owner/skill-name" -> "skill-name")
        let skill_name = slug.split('/').next_back().unwrap_or(slug);
        let dest_dir = Self::skills_dir().join(skill_name);

        // Create destination directory
        std::fs::create_dir_all(&dest_dir).map_err(|e| {
            AlephError::tool(format!("Failed to create skill directory: {}", e))
        })?;

        let mut found_skill_md = false;

        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| {
                AlephError::tool(format!("Failed to read ZIP entry: {}", e))
            })?;

            let entry_name = entry.name().to_string();

            // Skip directories and hidden files
            if entry.is_dir() || entry_name.starts_with('.') || entry_name.contains("/.") {
                continue;
            }

            // Strip the top-level directory prefix if present (common in ZIP archives)
            let relative_path = if let Some(pos) = entry_name.find('/') {
                &entry_name[pos + 1..]
            } else {
                &entry_name
            };

            if relative_path.is_empty() {
                continue;
            }

            let out_path = dest_dir.join(relative_path);

            // Create parent directories
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    AlephError::tool(format!("Failed to create directory: {}", e))
                })?;
            }

            // Read and write the file
            let mut content = Vec::new();
            entry.read_to_end(&mut content).map_err(|e| {
                AlephError::tool(format!("Failed to read ZIP entry content: {}", e))
            })?;

            // Validate SKILL.md if found
            if relative_path == "SKILL.md" || relative_path.ends_with("/SKILL.md") {
                let text = String::from_utf8_lossy(&content);
                crate::skill::parse_skill_content(
                    &text,
                    crate::domain::skill::SkillSource::Global,
                )
                .map_err(|e| AlephError::tool(format!("Invalid SKILL.md in package: {}", e)))?;
                found_skill_md = true;
            }

            std::fs::write(&out_path, &content).map_err(|e| {
                AlephError::tool(format!("Failed to write file: {}", e))
            })?;
        }

        if !found_skill_md {
            // Clean up the directory if no SKILL.md found
            let _ = std::fs::remove_dir_all(&dest_dir);
            return Err(AlephError::tool(
                "Package does not contain a valid SKILL.md file",
            ));
        }

        // Write .clawhub.json metadata
        let meta = ClawHubMeta {
            slug: slug.to_string(),
            version: version.to_string(),
            registry: registry_url.to_string(),
            installed_at: chrono::Utc::now().to_rfc3339(),
            owner: slug.split('/').next().unwrap_or("").to_string(),
        };
        let meta_path = dest_dir.join(".clawhub.json");
        let meta_json = serde_json::to_string_pretty(&meta).map_err(|e| {
            AlephError::tool(format!("Failed to serialize metadata: {}", e))
        })?;
        std::fs::write(&meta_path, meta_json).map_err(|e| {
            AlephError::tool(format!("Failed to write .clawhub.json: {}", e))
        })?;

        // Clean up temp ZIP
        let _ = std::fs::remove_file(zip_path);

        info!(slug, version, dir = %dest_dir.display(), "ClawHub skill installed");
        Ok(version.to_string())
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
        "Search, browse, install, and update skills from ClawHub registry. \
         Use this when the user wants to find new skills, install a skill from ClawHub, \
         or check for updates to installed skills.";

    type Args = ClawHubArgs;
    type Output = ClawHubOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            r#"clawhub(action="search", query="web scraping")"#.to_string(),
            r#"clawhub(action="browse", sort="trending", limit=10)"#.to_string(),
            r#"clawhub(action="install", slug="owner/skill-name")"#.to_string(),
            r#"clawhub(action="update", slug="owner/skill-name")"#.to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        match args.action {
            ClawHubAction::Search => {
                let query = args.query.ok_or_else(|| {
                    AlephError::tool("clawhub search: 'query' is required")
                })?;
                let limit = args.limit.unwrap_or(20);

                let results = self.client.search(&query, limit).await?;
                let count = results.len();

                Ok(ClawHubOutput {
                    message: format!("Found {} skills matching '{}'", count, query),
                    skills: Some(results),
                    cursor: None,
                    has_more: None,
                    installed_slug: None,
                    installed_version: None,
                })
            }

            ClawHubAction::Browse => {
                let sort: SortOrder = args
                    .sort
                    .unwrap_or(ClawHubSortOrder::Downloads)
                    .into();
                let limit = args.limit.unwrap_or(20);
                let cursor = args.cursor.as_deref();

                let resp: BrowseResponse = self.client.browse(sort, limit, cursor).await?;
                let count = resp.skills.len();

                Ok(ClawHubOutput {
                    message: format!("Showing {} skills", count),
                    skills: Some(resp.skills),
                    cursor: resp.cursor,
                    has_more: Some(resp.has_more),
                    installed_slug: None,
                    installed_version: None,
                })
            }

            ClawHubAction::Install => {
                let slug = args.slug.ok_or_else(|| {
                    AlephError::tool("clawhub install: 'slug' is required")
                })?;
                let version_arg = args.version.as_deref();

                // Download the ZIP
                let zip_path = self.client.download(&slug, version_arg).await?;

                // Determine actual version — use provided or fetch latest
                let version = if let Some(v) = version_arg {
                    v.to_string()
                } else {
                    let detail = self.client.get_skill(&slug).await?;
                    detail
                        .latest_version
                        .map(|v| v.number)
                        .unwrap_or_else(|| "unknown".to_string())
                };

                let installed_version = Self::install_from_zip(
                    &zip_path,
                    &slug,
                    &version,
                    self.client.base_url(),
                )?;

                Ok(ClawHubOutput {
                    message: format!("Skill '{}' v{} installed successfully", slug, installed_version),
                    skills: None,
                    cursor: None,
                    has_more: None,
                    installed_slug: Some(slug),
                    installed_version: Some(installed_version),
                })
            }

            ClawHubAction::Update => {
                let slug = args.slug.ok_or_else(|| {
                    AlephError::tool("clawhub update: 'slug' is required")
                })?;

                // Check installed version
                let skill_name = slug.split('/').next_back().unwrap_or(&slug);
                let meta_path = Self::skills_dir().join(skill_name).join(".clawhub.json");

                let local_meta: ClawHubMeta = if meta_path.exists() {
                    let content = std::fs::read_to_string(&meta_path).map_err(|e| {
                        AlephError::tool(format!("Failed to read .clawhub.json: {}", e))
                    })?;
                    serde_json::from_str(&content).map_err(|e| {
                        AlephError::tool(format!("Failed to parse .clawhub.json: {}", e))
                    })?
                } else {
                    return Err(AlephError::tool(format!(
                        "Skill '{}' is not installed from ClawHub (no .clawhub.json found)",
                        slug
                    )));
                };

                // Fetch remote latest version
                let detail = self.client.get_skill(&slug).await?;
                let remote_version = detail
                    .latest_version
                    .map(|v| v.number)
                    .unwrap_or_else(|| "unknown".to_string());

                if !ClawHubClient::is_newer_version(&local_meta.version, &remote_version) {
                    return Ok(ClawHubOutput {
                        message: format!(
                            "Skill '{}' is already up to date (v{})",
                            slug, local_meta.version
                        ),
                        skills: None,
                        cursor: None,
                        has_more: None,
                        installed_slug: Some(slug),
                        installed_version: Some(local_meta.version),
                    });
                }

                // Download and install the new version
                let zip_path = self.client.download(&slug, Some(&remote_version)).await?;
                let installed_version = Self::install_from_zip(
                    &zip_path,
                    &slug,
                    &remote_version,
                    self.client.base_url(),
                )?;

                Ok(ClawHubOutput {
                    message: format!(
                        "Skill '{}' updated from v{} to v{}",
                        slug, local_meta.version, installed_version
                    ),
                    skills: None,
                    cursor: None,
                    has_more: None,
                    installed_slug: Some(slug),
                    installed_version: Some(installed_version),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clawhub_tool_new() {
        let tool = ClawHubTool::new();
        assert_eq!(tool.client.base_url(), "https://clawhub.ai");
    }

    #[test]
    fn test_clawhub_tool_default() {
        let tool = ClawHubTool::default();
        assert_eq!(tool.client.base_url(), "https://clawhub.ai");
    }

    #[test]
    fn test_skills_dir() {
        let dir = ClawHubTool::skills_dir();
        assert!(dir.ends_with(".aleph/skills"));
    }

    #[test]
    fn test_clawhub_args_deserialize() {
        let json = r#"{"action":"search","query":"web scraping","limit":10}"#;
        let args: ClawHubArgs = serde_json::from_str(json).unwrap();
        assert!(matches!(args.action, ClawHubAction::Search));
        assert_eq!(args.query.as_deref(), Some("web scraping"));
        assert_eq!(args.limit, Some(10));
    }

    #[test]
    fn test_clawhub_output_serialize() {
        let output = ClawHubOutput {
            message: "Found 5 skills".to_string(),
            skills: None,
            cursor: None,
            has_more: None,
            installed_slug: None,
            installed_version: None,
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("Found 5 skills"));
        // Optional None fields should be skipped
        assert!(!json.contains("cursor"));
    }

    #[test]
    fn test_sort_order_conversion() {
        let sort: SortOrder = ClawHubSortOrder::Trending.into();
        assert_eq!(sort.as_api_str(), "trending");
    }
}
