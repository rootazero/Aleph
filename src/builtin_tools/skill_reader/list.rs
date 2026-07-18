//! `ListSkillsTool` — List available skills (Level 1 metadata).

use std::fs;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use super::super::{notify_tool_result, notify_tool_start};
use super::list_skill_files;
use crate::error::Result;
use crate::tools::AlephTool;

/// Arguments for `list_skills` tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ListSkillsArgs {
    /// Optional: filter by keyword in name or description
    #[serde(default)]
    pub filter: Option<String>,
}

/// Skill summary for listing
#[derive(Debug, Clone, Serialize)]
pub struct SkillSummary {
    /// Skill ID (directory name)
    pub id: String,

    /// Human-readable name
    pub name: String,

    /// Brief description
    pub description: String,

    /// Absolute path to the skill directory.
    ///
    /// Deliberately elided from the listing (`None` → skipped on serialize): a
    /// discovery listing that handed the model the absolute path of *every*
    /// skill up front taught it to `cat <path>/SKILL.md` instead of calling
    /// `skill_read(id)`. The model reads a skill by its `id`; the resolved path
    /// is returned by `skill_read` only when a specific skill is actually read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,

    /// Files available in this skill
    pub files: Vec<String>,

    /// Source location type (project or global)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Output from `list_skills` tool
#[derive(Debug, Clone, Serialize)]
pub struct ListSkillsOutput {
    /// Whether the operation succeeded
    pub success: bool,

    /// Total number of skills found
    pub count: usize,

    /// List of available skills
    pub skills: Vec<SkillSummary>,
}

/// Skill listing tool
///
/// Lists all available skills with their metadata.
/// Useful for discovering what skills are installed.
///
/// Supports multi-location discovery:
/// - Project level: .aleph/skills/, .claude/skills/
/// - Global level: ~/.aleph/skills, ~/.claude/skills
pub struct ListSkillsTool {
    /// All skills directories (for multi-location discovery)
    skills_dirs: Vec<PathBuf>,
}

impl ListSkillsTool {
    /// Create a new `ListSkillsTool` with a single directory (backwards compatible)
    #[must_use]
    pub fn new(skills_dir: PathBuf) -> Self {
        Self {
            skills_dirs: vec![skills_dir],
        }
    }

    /// Create a `ListSkillsTool` with multiple directories
    #[must_use]
    pub const fn with_directories(skills_dirs: Vec<PathBuf>) -> Self {
        Self { skills_dirs }
    }

    /// Create a `ListSkillsTool` with auto-discovery
    #[must_use]
    pub fn with_auto_discover(project_dir: Option<&Path>) -> Self {
        let skills_dirs =
            crate::utils::paths::get_all_skills_dirs(project_dir).unwrap_or_else(|_| vec![]);

        if skills_dirs.is_empty() {
            // Fallback to default directory
            let default_dir = crate::utils::paths::get_skills_dir()
                .unwrap_or_else(|_| PathBuf::from("~/.aleph/skills"));
            Self {
                skills_dirs: vec![default_dir],
            }
        } else {
            Self { skills_dirs }
        }
    }

    /// Determine source type based on path.
    ///
    /// "global" = installed under `~/.aleph/` or `~/.claude/` (skills, extensions, plugins).
    /// Everything else is "project" (workspace-local).
    fn get_source_type(&self, skill_dir: &Path) -> String {
        if let Ok(home) = crate::utils::paths::get_home_dir() {
            let aleph_root = home.join(".aleph");
            let claude_root = home.join(".claude");
            if skill_dir.starts_with(&aleph_root) || skill_dir.starts_with(&claude_root) {
                return "global".to_string();
            }
        }
        "project".to_string()
    }

    /// Parse skill frontmatter to extract metadata
    fn parse_skill_metadata(&self, skill_dir: &Path) -> Option<SkillSummary> {
        let skill_md = skill_dir.join("SKILL.md");
        if !skill_md.exists() {
            return None;
        }

        let content = fs::read_to_string(&skill_md).ok()?;
        let id = skill_dir.file_name()?.to_str()?.to_string();

        // Parse frontmatter using v2 parser
        let manifest =
            crate::skill::parse_skill_content(&content, crate::domain::skill::SkillSource::Global)
                .ok()?;

        // List files
        let files = list_skill_files(skill_dir);

        // Determine source
        let source = self.get_source_type(skill_dir);

        Some(SkillSummary {
            id,
            name: manifest.name().to_string(),
            description: manifest.description().to_string(),
            // Elided from the listing on purpose — see `SkillSummary::location`.
            // The model reads by `id`; the path is only surfaced by `skill_read`.
            location: None,
            files,
            source: Some(source),
        })
    }

    /// Execute the `list_skills` operation (internal implementation)
    async fn call_impl(
        &self,
        args: ListSkillsArgs,
    ) -> std::result::Result<ListSkillsOutput, super::super::error::ToolError> {
        let args_summary = match &args.filter {
            Some(f) => format!("Listing skills (filter: {f})"),
            None => "Listing all skills".to_string(),
        };
        notify_tool_start(Self::NAME, &args_summary);

        let mut skills = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();

        // Scan all skills directories
        for skills_dir in &self.skills_dirs {
            if !skills_dir.exists() {
                debug!(
                    skills_dir = %skills_dir.display(),
                    "Skills directory does not exist"
                );
                continue;
            }

            if let Ok(entries) = fs::read_dir(skills_dir) {
                for entry in entries.flatten() {
                    if let Ok(file_type) = entry.file_type() {
                        if file_type.is_dir() {
                            let skill_dir = entry.path();

                            // Skip hidden directories
                            if let Some(name) = skill_dir.file_name() {
                                if name.to_str().is_some_and(|n| n.starts_with('.')) {
                                    continue;
                                }
                            }

                            // Try to parse skill metadata
                            if let Some(summary) = self.parse_skill_metadata(&skill_dir) {
                                // Skip if already seen (first occurrence wins)
                                if seen_ids.contains(&summary.id) {
                                    debug!(
                                        skill_id = %summary.id,
                                        "Skill already discovered, skipping duplicate"
                                    );
                                    continue;
                                }

                                // Apply filter if specified
                                if let Some(ref filter) = args.filter {
                                    let filter_lower = filter.to_lowercase();
                                    let matches = summary.id.to_lowercase().contains(&filter_lower)
                                        || summary.name.to_lowercase().contains(&filter_lower)
                                        || summary
                                            .description
                                            .to_lowercase()
                                            .contains(&filter_lower);

                                    if !matches {
                                        continue;
                                    }
                                }

                                seen_ids.insert(summary.id.clone());
                                skills.push(summary);
                            }
                        }
                    }
                }
            }
        }

        // Sort by source (project first), then by ID
        skills.sort_by(|a, b| {
            let a_source = a.source.as_deref().unwrap_or("global");
            let b_source = b.source.as_deref().unwrap_or("global");
            match (a_source, b_source) {
                ("project", "global") => std::cmp::Ordering::Less,
                ("global", "project") => std::cmp::Ordering::Greater,
                _ => a.id.cmp(&b.id),
            }
        });

        let count = skills.len();
        let result_msg = format!("Found {count} skills");
        notify_tool_result(Self::NAME, &result_msg, true);

        info!(count = count, "Listed skills");

        Ok(ListSkillsOutput {
            success: true,
            count,
            skills,
        })
    }
}

impl Default for ListSkillsTool {
    fn default() -> Self {
        Self::with_auto_discover(None)
    }
}

impl Clone for ListSkillsTool {
    fn clone(&self) -> Self {
        Self {
            skills_dirs: self.skills_dirs.clone(),
        }
    }
}

/// Implementation of `AlephTool` trait for `ListSkillsTool`
#[async_trait]
impl AlephTool for ListSkillsTool {
    const NAME: &'static str = "skill_list";
    const DESCRIPTION: &'static str = r#"List all available skills installed on the system.

Use this tool to discover what skills are available before using skill.read.
Each skill has an ID, name, and description. Read a skill by its ID with
skill.read(skill_id) — the id is the handle, do not cat SKILL.md.

Skills are discovered from multiple locations:
- Project level: .aleph/skills/, .claude/skills/ (traverse up to git root)
- Global level: ~/.aleph/skills, ~/.claude/skills

After finding a relevant skill, use skill.read(skill_id) to load its full instructions."#;

    type Args = ListSkillsArgs;
    type Output = ListSkillsOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.call_impl(args).await.map_err(Into::into)
    }
}
