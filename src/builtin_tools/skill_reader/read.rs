//! ReadSkillTool — Read skill instructions (Level 2) or resources (Level 3).

use std::fs;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use super::super::error::ToolError;
use super::super::{notify_tool_result, notify_tool_start};
use super::list_skill_files;
use crate::error::Result;
use crate::tools::AlephTool;

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

    /// Absolute path to the skill directory
    /// This allows the agent to locate scripts and resources within the skill
    pub location: String,

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
/// - Project level: .aleph/skills/, .claude/skills/
/// - Global level: ~/.aleph/skills, ~/.claude/skills
pub struct ReadSkillTool {
    /// All skills directories (for multi-location discovery)
    skills_dirs: Vec<PathBuf>,

    /// Maximum file size to read (5MB default)
    max_file_size: u64,
}

impl ReadSkillTool {
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
        let skills_dirs =
            crate::utils::paths::get_all_skills_dirs(project_dir).unwrap_or_else(|_| vec![]);

        if skills_dirs.is_empty() {
            // Fallback to default directory
            let default_dir = crate::utils::paths::get_skills_dir()
                .unwrap_or_else(|_| PathBuf::from("~/.aleph/skills"));
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

    /// Collect every directory that contains skill `skill_id` (a SKILL.md).
    /// Returns all matches so the caller can refuse ambiguous names rather
    /// than silently shadowing — mirrors hermes-agent's collision refusal.
    fn find_skill_dirs(&self, skill_id: &str) -> Vec<PathBuf> {
        let mut hits = Vec::new();
        for skills_dir in &self.skills_dirs {
            let skill_dir = skills_dir.join(skill_id);
            if skill_dir.is_dir() && skill_dir.join("SKILL.md").exists() {
                hits.push(skill_dir);
            }
        }
        hits
    }

    /// Validate skill_id to prevent path traversal attacks
    fn validate_skill_id(&self, skill_id: &str) -> std::result::Result<(), ToolError> {
        // Check for empty
        if skill_id.is_empty() {
            return Err(ToolError::InvalidArgs(
                "skill_id cannot be empty".to_string(),
            ));
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

    /// Validate a `file_name` that MAY contain forward-slash subdir segments
    /// (e.g. `references/guide.md`). Rejects `..` components, absolute paths,
    /// backslashes, and any leading-dot segment. The caller additionally
    /// confirms the resolved path stays inside the skill dir via `is_path_within`.
    fn validate_file_name(&self, file_name: &str) -> std::result::Result<(), ToolError> {
        if file_name.is_empty() {
            return Err(ToolError::InvalidArgs("file_name cannot be empty".into()));
        }
        if file_name.contains('\\') {
            return Err(ToolError::InvalidArgs(
                "file_name cannot contain backslashes".into(),
            ));
        }
        let path = std::path::Path::new(file_name);
        if path.is_absolute() {
            return Err(ToolError::InvalidArgs(
                "file_name cannot be absolute".into(),
            ));
        }
        for component in path.components() {
            match component {
                std::path::Component::ParentDir => {
                    return Err(ToolError::InvalidArgs(
                        "invalid file_name: traversal via '..' is not allowed".into(),
                    ))
                }
                std::path::Component::Normal(seg) => {
                    if seg.to_string_lossy().starts_with('.') {
                        return Err(ToolError::InvalidArgs(
                            "file_name segments cannot start with '.'".into(),
                        ));
                    }
                }
                _ => {
                    return Err(ToolError::InvalidArgs(
                        "file_name contains an invalid path component".into(),
                    ))
                }
            }
        }
        Ok(())
    }

    /// Execute the read_skill operation (internal implementation)
    async fn call_impl(
        &self,
        args: ReadSkillArgs,
    ) -> std::result::Result<ReadSkillOutput, ToolError> {
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

        // Find skill directory across all configured locations.
        // Refuse ambiguous names to mirror hermes-agent's collision refusal.
        let candidates = self.find_skill_dirs(&args.skill_id);
        let skill_dir = match candidates.len() {
            0 => {
                let error_msg = format!("Skill '{}' not found", args.skill_id);
                notify_tool_result(Self::NAME, &error_msg, false);
                return Err(ToolError::NotFound(error_msg));
            }
            1 => candidates
                .into_iter()
                .next()
                .unwrap_or_else(|| unreachable!("candidates.len() == 1")),
            _ => {
                let paths: Vec<String> =
                    candidates.iter().map(|p| p.display().to_string()).collect();
                let error_msg = format!(
                    "skill '{}' is ambiguous — found in multiple locations: {}. \
                     Disambiguate by removing the duplicate or renaming one.",
                    args.skill_id,
                    paths.join(", ")
                );
                notify_tool_result(Self::NAME, &error_msg, false);
                return Err(ToolError::InvalidArgs(error_msg));
            }
        };

        let file_path = skill_dir.join(file_name);

        // Defense in depth: ensure the resolved path stays inside skill_dir.
        if !crate::utils::path_within::is_path_within(&skill_dir, &file_path) {
            return Err(ToolError::InvalidArgs(
                "file_name escapes the skill directory".into(),
            ));
        }

        // Check file exists
        if !file_path.exists() || !file_path.is_file() {
            let available = list_skill_files(&skill_dir);
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
        let raw_content = fs::read_to_string(&file_path)
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to read file: {}", e)))?;

        // Preprocess Markdown instruction files: expand `${ALEPH_SKILL_DIR}` /
        // `${ALEPH_SESSION_ID}` template variables and, when the skill opts in
        // via `allow-inline-shell: true`, splice in bounded inline-shell output.
        // Non-Markdown resources (scripts, data) are returned verbatim so their
        // bytes are never altered. Content with no template token and no opt-in
        // is returned unchanged — existing skills render identically.
        let content = if file_name.to_ascii_lowercase().ends_with(".md") {
            let ctx = crate::skill::SkillPreprocessContext::new(skill_dir.clone());
            crate::skill::preprocess_skill_content(&raw_content, &ctx).await
        } else {
            raw_content
        };

        // List available files
        let available_files = list_skill_files(&skill_dir);

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

        // Best-effort usage tracking — never affects the tool result.
        if let Some(parent) = skill_dir.parent() {
            let store = crate::skill::UsageStore::new(parent);
            if file_name == "SKILL.md" {
                store.record_use(&args.skill_id);
                // Co-occurrence capture rides this existing use chokepoint: the
                // dream pipeline mines temporally-close uses into MetaSkill
                // proposals (see `WorkflowProposalStage`). Best-effort.
                crate::skill::CoOccurrenceLog::new(parent).record(&args.skill_id);
            } else {
                store.record_view(&args.skill_id);
            }
        }

        Ok(ReadSkillOutput {
            success: true,
            skill_id: args.skill_id,
            file_name: file_name.to_string(),
            content,
            size: metadata.len(),
            location: skill_dir.to_string_lossy().to_string(),
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

/// Implementation of AlephTool trait for ReadSkillTool
#[async_trait]
impl AlephTool for ReadSkillTool {
    const NAME: &'static str = "skill_read";
    const DESCRIPTION: &'static str = r#"Read the instructions of an installed skill.

Use this tool when you need to execute a task that matches a skill's purpose.
The skill instructions tell you exactly how to approach the task.

After reading a skill, you MUST follow its instructions exactly.
Skill instructions are task directives, not suggestions.

Skills are discovered from multiple locations:
- Project level: .aleph/skills/, .claude/skills/ (traverse up to git root)
- Global level: ~/.aleph/skills, ~/.claude/skills

Examples:
- User asks to "refine this text" → skill.read(skill_id="refine-text")
- User asks to "translate to Chinese" → skill.read(skill_id="translate")
- User asks to "summarize this" → skill.read(skill_id="summarize")

You can also read additional resources within a skill by specifying file_name:
- skill.read(skill_id="code-review", file_name="CHECKLIST.md")

Markdown instructions are preprocessed on read: `${ALEPH_SKILL_DIR}` expands to
the skill's directory (use it to reference bundled scripts/resources). Skills
that declare `allow-inline-shell: true` in their frontmatter may also embed
live context via inline-shell snippets written as !`command`."#;

    type Args = ReadSkillArgs;
    type Output = ReadSkillOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.call_impl(args).await.map_err(Into::into)
    }
}
