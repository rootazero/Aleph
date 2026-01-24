//! Skill reading tools for AI agent integration
//!
//! Implements Claude's Progressive Disclosure pattern for skills:
//! - Level 1 (metadata) is always available in system prompt
//! - Level 2 (instructions) loaded via read_skill tool call
//! - Level 3 (resources) loaded on-demand via file_name parameter
//!
//! This enables the agent to actively request skill instructions,
//! treating them as task directives rather than passive context.

use std::fs;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use super::error::ToolError;
use super::{notify_tool_result, notify_tool_start};
use crate::error::Result;
use crate::tools::AetherTool;

// ============================================================================
// ReadSkillTool - Read skill instructions (Level 2) or resources (Level 3)
// ============================================================================

/// Arguments for read_skill tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ReadSkillArgs {
    /// The skill identifier (directory name, e.g., "refine-text", "translate")
    pub skill_id: String,

    /// Optional: specific file to read within the skill directory.
    /// Defaults to "SKILL.md" if not specified.
    /// Use this to access Level 3 resources like "ADVANCED.md", "REFERENCE.md", etc.
    #[serde(default)]
    pub file_name: Option<String>,
}

/// Output from read_skill tool
#[derive(Debug, Clone, Serialize)]
pub struct ReadSkillOutput {
    /// Whether the operation succeeded
    pub success: bool,

    /// The skill ID that was read
    pub skill_id: String,

    /// The file that was read
    pub file_name: String,

    /// The content of the skill file (instructions or resources)
    pub content: String,

    /// Size of the file in bytes
    pub size: u64,

    /// List of other files available in this skill directory
    /// Useful for discovering Level 3 resources
    pub available_files: Vec<String>,
}

/// Skill reading tool
///
/// Allows the agent to actively read skill instructions and resources.
/// This implements Claude's Progressive Disclosure pattern where:
/// - The agent sees skill metadata in the system prompt
/// - The agent calls this tool to load full instructions when needed
/// - The agent can request additional resources as needed
///
/// Supports multi-location discovery:
/// - Project level: .aether/skills/, .claude/skills/
/// - Global level: ~/.config/aether/skills, ~/.claude/skills
pub struct ReadSkillTool {
    /// All skills directories (for multi-location discovery)
    skills_dirs: Vec<PathBuf>,

    /// Maximum file size to read (5MB default)
    max_file_size: u64,
}

impl ReadSkillTool {
    /// Tool identifier
    pub const NAME: &'static str = "read_skill";

    /// Tool description for AI prompt
    pub const DESCRIPTION: &'static str = r#"Read the instructions of an installed skill.

Use this tool when you need to execute a task that matches a skill's purpose.
The skill instructions tell you exactly how to approach the task.

After reading a skill, you MUST follow its instructions exactly.
Skill instructions are task directives, not suggestions.

Skills are discovered from multiple locations:
- Project level: .aether/skills/, .claude/skills/ (traverse up to git root)
- Global level: ~/.config/aether/skills, ~/.claude/skills

Examples:
- User asks to "refine this text" → read_skill(skill_id="refine-text")
- User asks to "translate to Chinese" → read_skill(skill_id="translate")
- User asks to "summarize this" → read_skill(skill_id="summarize")

You can also read additional resources within a skill by specifying file_name:
- read_skill(skill_id="code-review", file_name="CHECKLIST.md")
"#;

    /// Create a new ReadSkillTool with a single directory (backwards compatible)
    pub fn new(skills_dir: PathBuf) -> Self {
        Self {
            skills_dirs: vec![skills_dir],
            max_file_size: 5 * 1024 * 1024, // 5MB
        }
    }

    /// Create a ReadSkillTool with multiple directories
    pub fn with_directories(skills_dirs: Vec<PathBuf>) -> Self {
        Self {
            skills_dirs,
            max_file_size: 5 * 1024 * 1024,
        }
    }

    /// Create a ReadSkillTool with auto-discovery
    pub fn with_auto_discover(project_dir: Option<&Path>) -> Self {
        let skills_dirs = crate::utils::paths::get_all_skills_dirs(project_dir)
            .unwrap_or_else(|_| vec![]);

        if skills_dirs.is_empty() {
            // Fallback to default directory
            let default_dir = crate::utils::paths::get_skills_dir()
                .unwrap_or_else(|_| PathBuf::from("~/.config/aether/skills"));
            Self {
                skills_dirs: vec![default_dir],
                max_file_size: 5 * 1024 * 1024,
            }
        } else {
            Self {
                skills_dirs,
                max_file_size: 5 * 1024 * 1024,
            }
        }
    }

    /// Create with custom max file size
    pub fn with_max_size(mut self, max_size: u64) -> Self {
        self.max_file_size = max_size;
        self
    }

    /// Find the skill directory by ID across all configured directories
    fn find_skill_dir(&self, skill_id: &str) -> Option<PathBuf> {
        for skills_dir in &self.skills_dirs {
            let skill_dir = skills_dir.join(skill_id);
            if skill_dir.is_dir() && skill_dir.join("SKILL.md").exists() {
                return Some(skill_dir);
            }
        }
        None
    }

    /// Validate skill_id to prevent path traversal attacks
    fn validate_skill_id(&self, skill_id: &str) -> std::result::Result<(), ToolError> {
        // Check for empty
        if skill_id.is_empty() {
            return Err(ToolError::InvalidArgs("skill_id cannot be empty".to_string()));
        }

        // Check for path traversal attempts
        if skill_id.contains("..") || skill_id.contains('/') || skill_id.contains('\\') {
            return Err(ToolError::InvalidArgs(
                "skill_id cannot contain path separators or '..'".to_string(),
            ));
        }

        // Check for hidden files
        if skill_id.starts_with('.') {
            return Err(ToolError::InvalidArgs(
                "skill_id cannot start with '.'".to_string(),
            ));
        }

        Ok(())
    }

    /// Validate file_name to prevent path traversal
    fn validate_file_name(&self, file_name: &str) -> std::result::Result<(), ToolError> {
        if file_name.contains("..") || file_name.contains('/') || file_name.contains('\\') {
            return Err(ToolError::InvalidArgs(
                "file_name cannot contain path separators or '..'".to_string(),
            ));
        }

        if file_name.starts_with('.') {
            return Err(ToolError::InvalidArgs(
                "file_name cannot start with '.'".to_string(),
            ));
        }

        Ok(())
    }

    /// List files in a skill directory
    fn list_skill_files(&self, skill_dir: &Path) -> Vec<String> {
        let mut files = Vec::new();

        if let Ok(entries) = fs::read_dir(skill_dir) {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type() {
                    if file_type.is_file() {
                        if let Some(name) = entry.file_name().to_str() {
                            // Skip hidden files
                            if !name.starts_with('.') {
                                files.push(name.to_string());
                            }
                        }
                    }
                }
            }
        }

        files.sort();
        files
    }

    /// Execute the read_skill operation (internal implementation)
    async fn call_impl(&self, args: ReadSkillArgs) -> std::result::Result<ReadSkillOutput, ToolError> {
        let args_summary = format!(
            "Reading skill: {} (file: {})",
            args.skill_id,
            args.file_name.as_deref().unwrap_or("SKILL.md")
        );
        notify_tool_start(Self::NAME, &args_summary);

        // Validate skill_id
        self.validate_skill_id(&args.skill_id)?;

        // Determine file to read
        let file_name = args.file_name.as_deref().unwrap_or("SKILL.md");
        self.validate_file_name(file_name)?;

        // Find skill directory across all configured locations
        let skill_dir = self.find_skill_dir(&args.skill_id).ok_or_else(|| {
            let error_msg = format!("Skill '{}' not found", args.skill_id);
            notify_tool_result(Self::NAME, &error_msg, false);
            ToolError::NotFound(error_msg)
        })?;

        let file_path = skill_dir.join(file_name);

        // Check file exists
        if !file_path.exists() || !file_path.is_file() {
            let available = self.list_skill_files(&skill_dir);
            let error_msg = format!(
                "File '{}' not found in skill '{}'. Available files: {:?}",
                file_name, args.skill_id, available
            );
            notify_tool_result(Self::NAME, &error_msg, false);
            return Err(ToolError::NotFound(error_msg));
        }

        // Check file size
        let metadata = fs::metadata(&file_path).map_err(|e| {
            ToolError::ExecutionFailed(format!("Failed to read file metadata: {}", e))
        })?;

        if metadata.len() > self.max_file_size {
            let error_msg = format!(
                "File too large: {} bytes (max: {} bytes)",
                metadata.len(),
                self.max_file_size
            );
            notify_tool_result(Self::NAME, &error_msg, false);
            return Err(ToolError::ExecutionFailed(error_msg));
        }

        // Read file content
        let content = fs::read_to_string(&file_path).map_err(|e| {
            ToolError::ExecutionFailed(format!("Failed to read file: {}", e))
        })?;

        // List available files
        let available_files = self.list_skill_files(&skill_dir);

        let result_msg = format!(
            "Read {} bytes from {}/{}",
            metadata.len(),
            args.skill_id,
            file_name
        );
        notify_tool_result(Self::NAME, &result_msg, true);

        info!(
            skill_id = %args.skill_id,
            file_name = %file_name,
            size = metadata.len(),
            "Skill file read successfully"
        );

        Ok(ReadSkillOutput {
            success: true,
            skill_id: args.skill_id,
            file_name: file_name.to_string(),
            content,
            size: metadata.len(),
            available_files,
        })
    }
}

impl Default for ReadSkillTool {
    fn default() -> Self {
        Self::with_auto_discover(None)
    }
}

impl Clone for ReadSkillTool {
    fn clone(&self) -> Self {
        Self {
            skills_dirs: self.skills_dirs.clone(),
            max_file_size: self.max_file_size,
        }
    }
}

/// Implementation of AetherTool trait for ReadSkillTool
#[async_trait]
impl AetherTool for ReadSkillTool {
    const NAME: &'static str = "read_skill";
    const DESCRIPTION: &'static str = r#"Read the instructions of an installed skill.

Use this tool when you need to execute a task that matches a skill's purpose.
The skill instructions tell you exactly how to approach the task.

After reading a skill, you MUST follow its instructions exactly.
Skill instructions are task directives, not suggestions."#;

    type Args = ReadSkillArgs;
    type Output = ReadSkillOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.call_impl(args).await.map_err(Into::into)
    }
}

// ============================================================================
// ListSkillsTool - List available skills (Level 1 metadata)
// ============================================================================

/// Arguments for list_skills tool
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

    /// Trigger keywords (if any)
    pub triggers: Vec<String>,

    /// Files available in this skill
    pub files: Vec<String>,

    /// Source location type (project or global)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,

    /// Full path to the skill directory
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<PathBuf>,
}

/// Output from list_skills tool
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
/// - Project level: .aether/skills/, .claude/skills/
/// - Global level: ~/.config/aether/skills, ~/.claude/skills
pub struct ListSkillsTool {
    /// All skills directories (for multi-location discovery)
    skills_dirs: Vec<PathBuf>,
}

impl ListSkillsTool {
    /// Tool identifier
    pub const NAME: &'static str = "list_skills";

    /// Tool description for AI prompt
    pub const DESCRIPTION: &'static str = r#"List all available skills installed on the system.

Use this tool to discover what skills are available before using read_skill.
Each skill has an ID, name, description, and optional trigger keywords.

Skills are discovered from multiple locations:
- Project level: .aether/skills/, .claude/skills/ (traverse up to git root)
- Global level: ~/.config/aether/skills, ~/.claude/skills

After finding a relevant skill, use read_skill(skill_id) to load its full instructions.
"#;

    /// Create a new ListSkillsTool with a single directory (backwards compatible)
    pub fn new(skills_dir: PathBuf) -> Self {
        Self {
            skills_dirs: vec![skills_dir],
        }
    }

    /// Create a ListSkillsTool with multiple directories
    pub fn with_directories(skills_dirs: Vec<PathBuf>) -> Self {
        Self { skills_dirs }
    }

    /// Create a ListSkillsTool with auto-discovery
    pub fn with_auto_discover(project_dir: Option<&Path>) -> Self {
        let skills_dirs = crate::utils::paths::get_all_skills_dirs(project_dir)
            .unwrap_or_else(|_| vec![]);

        if skills_dirs.is_empty() {
            // Fallback to default directory
            let default_dir = crate::utils::paths::get_skills_dir()
                .unwrap_or_else(|_| PathBuf::from("~/.config/aether/skills"));
            Self {
                skills_dirs: vec![default_dir],
            }
        } else {
            Self { skills_dirs }
        }
    }

    /// Determine source type based on path
    fn get_source_type(&self, skill_dir: &Path) -> String {
        if let Ok(home) = crate::utils::paths::get_home_dir() {
            if skill_dir.starts_with(&home) {
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

        // Parse frontmatter
        let skill = crate::skills::Skill::parse(&id, &content).ok()?;

        // List files
        let files = self.list_skill_files(skill_dir);

        // Determine source
        let source = self.get_source_type(skill_dir);

        Some(SkillSummary {
            id,
            name: skill.frontmatter.name,
            description: skill.frontmatter.description,
            triggers: skill.frontmatter.triggers,
            files,
            source: Some(source),
            location: Some(skill_dir.to_path_buf()),
        })
    }

    /// List files in a skill directory
    fn list_skill_files(&self, skill_dir: &Path) -> Vec<String> {
        let mut files = Vec::new();

        if let Ok(entries) = fs::read_dir(skill_dir) {
            for entry in entries.flatten() {
                if let Ok(file_type) = entry.file_type() {
                    if file_type.is_file() {
                        if let Some(name) = entry.file_name().to_str() {
                            if !name.starts_with('.') {
                                files.push(name.to_string());
                            }
                        }
                    }
                }
            }
        }

        files.sort();
        files
    }

    /// Execute the list_skills operation (internal implementation)
    async fn call_impl(&self, args: ListSkillsArgs) -> std::result::Result<ListSkillsOutput, ToolError> {
        let args_summary = match &args.filter {
            Some(f) => format!("Listing skills (filter: {})", f),
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
                                if name.to_str().map_or(false, |n| n.starts_with('.')) {
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
                                        || summary.description.to_lowercase().contains(&filter_lower)
                                        || summary.triggers.iter().any(|t| {
                                            t.to_lowercase().contains(&filter_lower)
                                        });

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
        let result_msg = format!("Found {} skills", count);
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

/// Implementation of AetherTool trait for ListSkillsTool
#[async_trait]
impl AetherTool for ListSkillsTool {
    const NAME: &'static str = "list_skills";
    const DESCRIPTION: &'static str = r#"List all available skills installed on the system.

Use this tool to discover what skills are available before using read_skill.
Each skill has an ID, name, description, and optional trigger keywords.

After finding a relevant skill, use read_skill(skill_id) to load its full instructions."#;

    type Args = ListSkillsArgs;
    type Output = ListSkillsOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.call_impl(args).await.map_err(Into::into)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::AetherTool;
    use tempfile::TempDir;

    fn create_test_skill(dir: &Path, id: &str, name: &str, description: &str) {
        let skill_dir = dir.join(id);
        fs::create_dir_all(&skill_dir).unwrap();

        let content = format!(
            r#"---
name: {}
description: {}
triggers:
  - test
---

# {} Skill

These are the skill instructions.
Follow them carefully.
"#,
            name, description, name
        );

        fs::write(skill_dir.join("SKILL.md"), content).unwrap();

        // Add an extra resource file
        fs::write(
            skill_dir.join("REFERENCE.md"),
            "# Reference\n\nAdditional reference material.",
        )
        .unwrap();
    }

    #[tokio::test]
    async fn test_read_skill_success() {
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        create_test_skill(&skills_dir, "test-skill", "Test Skill", "A test skill");

        let tool = ReadSkillTool::new(skills_dir);
        let args = ReadSkillArgs {
            skill_id: "test-skill".to_string(),
            file_name: None,
        };

        // Use fully qualified syntax
        let result = AetherTool::call(&tool, args).await.unwrap();
        assert!(result.success);
        assert_eq!(result.skill_id, "test-skill");
        assert_eq!(result.file_name, "SKILL.md");
        assert!(result.content.contains("Test Skill"));
        assert!(result.content.contains("skill instructions"));
        assert!(result.available_files.contains(&"SKILL.md".to_string()));
        assert!(result.available_files.contains(&"REFERENCE.md".to_string()));
    }

    #[tokio::test]
    async fn test_read_skill_resource() {
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        create_test_skill(&skills_dir, "test-skill", "Test Skill", "A test skill");

        let tool = ReadSkillTool::new(skills_dir);
        let args = ReadSkillArgs {
            skill_id: "test-skill".to_string(),
            file_name: Some("REFERENCE.md".to_string()),
        };

        // Use fully qualified syntax
        let result = AetherTool::call(&tool, args).await.unwrap();
        assert!(result.success);
        assert_eq!(result.file_name, "REFERENCE.md");
        assert!(result.content.contains("Additional reference material"));
    }

    #[tokio::test]
    async fn test_read_skill_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        let tool = ReadSkillTool::new(skills_dir);
        let args = ReadSkillArgs {
            skill_id: "nonexistent".to_string(),
            file_name: None,
        };

        // Use fully qualified syntax
        let result = AetherTool::call(&tool, args).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not found") || err_msg.contains("NotFound"), "Error should indicate not found: {}", err_msg);
    }

    #[tokio::test]
    async fn test_read_skill_path_traversal() {
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        let tool = ReadSkillTool::new(skills_dir);

        // Test skill_id path traversal
        let args = ReadSkillArgs {
            skill_id: "../etc/passwd".to_string(),
            file_name: None,
        };
        // Use fully qualified syntax
        let result = AetherTool::call(&tool, args).await;
        assert!(result.is_err());

        // Test file_name path traversal
        let args = ReadSkillArgs {
            skill_id: "test".to_string(),
            file_name: Some("../../../etc/passwd".to_string()),
        };
        let result = AetherTool::call(&tool, args).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_list_skills() {
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        create_test_skill(&skills_dir, "skill-a", "Skill A", "First skill");
        create_test_skill(&skills_dir, "skill-b", "Skill B", "Second skill");

        let tool = ListSkillsTool::new(skills_dir);
        let args = ListSkillsArgs { filter: None };

        // Use fully qualified syntax
        let result = AetherTool::call(&tool, args).await.unwrap();
        assert!(result.success);
        assert_eq!(result.count, 2);
        assert_eq!(result.skills[0].id, "skill-a");
        assert_eq!(result.skills[1].id, "skill-b");
    }

    #[tokio::test]
    async fn test_list_skills_filter() {
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        create_test_skill(&skills_dir, "refine-text", "Refine Text", "Improve writing");
        create_test_skill(&skills_dir, "translate", "Translate", "Translate text");

        let tool = ListSkillsTool::new(skills_dir);
        let args = ListSkillsArgs {
            filter: Some("writing".to_string()),
        };

        // Use fully qualified syntax
        let result = AetherTool::call(&tool, args).await.unwrap();
        assert!(result.success);
        assert_eq!(result.count, 1);
        assert_eq!(result.skills[0].id, "refine-text");
    }

    #[tokio::test]
    async fn test_multi_directory_discovery() {
        // Create two separate skills directories
        let temp_dir1 = TempDir::new().unwrap();
        let temp_dir2 = TempDir::new().unwrap();
        let skills_dir1 = temp_dir1.path().to_path_buf();
        let skills_dir2 = temp_dir2.path().to_path_buf();

        // Create skills in different directories
        create_test_skill(&skills_dir1, "skill-a", "Skill A", "From directory 1");
        create_test_skill(&skills_dir2, "skill-b", "Skill B", "From directory 2");

        // Test ListSkillsTool with multiple directories
        let tool = ListSkillsTool::with_directories(vec![skills_dir1.clone(), skills_dir2.clone()]);
        let args = ListSkillsArgs { filter: None };

        let result = AetherTool::call(&tool, args).await.unwrap();
        assert!(result.success);
        assert_eq!(result.count, 2);

        // Both skills should be found
        let ids: Vec<&str> = result.skills.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"skill-a"));
        assert!(ids.contains(&"skill-b"));
    }

    #[tokio::test]
    async fn test_multi_directory_deduplication() {
        // Create two directories with the same skill
        let temp_dir1 = TempDir::new().unwrap();
        let temp_dir2 = TempDir::new().unwrap();
        let skills_dir1 = temp_dir1.path().to_path_buf();
        let skills_dir2 = temp_dir2.path().to_path_buf();

        // Create same skill ID in both directories
        create_test_skill(&skills_dir1, "same-skill", "Skill From Dir1", "First directory");
        create_test_skill(&skills_dir2, "same-skill", "Skill From Dir2", "Second directory");

        // Test that first occurrence wins
        let tool = ListSkillsTool::with_directories(vec![skills_dir1.clone(), skills_dir2.clone()]);
        let args = ListSkillsArgs { filter: None };

        let result = AetherTool::call(&tool, args).await.unwrap();
        assert!(result.success);
        assert_eq!(result.count, 1);
        assert_eq!(result.skills[0].id, "same-skill");
        // Should get the one from dir1 (first in list)
        assert!(result.skills[0].description.contains("First directory"));
    }

    #[tokio::test]
    async fn test_read_skill_multi_directory() {
        // Create two directories
        let temp_dir1 = TempDir::new().unwrap();
        let temp_dir2 = TempDir::new().unwrap();
        let skills_dir1 = temp_dir1.path().to_path_buf();
        let skills_dir2 = temp_dir2.path().to_path_buf();

        // Only create skill in the second directory
        create_test_skill(&skills_dir2, "unique-skill", "Unique Skill", "Only in dir2");

        // ReadSkillTool should find it even though it's in the second directory
        let tool = ReadSkillTool::with_directories(vec![skills_dir1, skills_dir2]);
        let args = ReadSkillArgs {
            skill_id: "unique-skill".to_string(),
            file_name: None,
        };

        let result = AetherTool::call(&tool, args).await.unwrap();
        assert!(result.success);
        assert_eq!(result.skill_id, "unique-skill");
        assert!(result.content.contains("Unique Skill"));
    }
}
