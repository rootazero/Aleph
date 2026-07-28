//! `ReadSkillTool` — Read skill instructions (Level 2) or resources (Level 3).

use std::fs;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use super::super::error::ToolError;
use super::super::{notify_tool_result, notify_tool_start};
use super::list_skill_files;
use crate::error::Result;
use crate::tools::AlephTool;

/// Arguments for `read_skill` tool
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

/// Output from `read_skill` tool
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

    /// Absolute path to the skill directory. Provided so the agent can run
    /// bundled scripts referenced by the instructions (the same dir the body's
    /// `${ALEPH_SKILL_DIR}` token expands to). To *read* a supporting file, use
    /// `skill_read` with `file_name` — see `usage_hint` — not `cat` on this path.
    pub location: String,

    /// List of other files available in this skill directory
    /// Useful for discovering Level 3 resources
    pub available_files: Vec<String>,

    /// How to read the files listed in `available_files`: call
    /// `skill_read(skill_id, file_name=…)` rather than `cat`-ing them off
    /// `location`. Empty when the skill ships no supporting files.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub usage_hint: String,
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
    /// Create a new `ReadSkillTool` with a single directory (backwards compatible)
    #[must_use]
    pub fn new(skills_dir: PathBuf) -> Self {
        Self {
            skills_dirs: vec![skills_dir],
            max_file_size: 5 * 1024 * 1024, // 5MB
        }
    }

    /// Create a `ReadSkillTool` with multiple directories
    #[must_use]
    pub const fn with_directories(skills_dirs: Vec<PathBuf>) -> Self {
        Self {
            skills_dirs,
            max_file_size: 5 * 1024 * 1024,
        }
    }

    /// Create a `ReadSkillTool` with auto-discovery
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
    #[must_use]
    pub const fn with_max_size(mut self, max_size: u64) -> Self {
        self.max_file_size = max_size;
        self
    }

    /// Collect every *distinct* directory that contains skill `skill_id`
    /// (a `SKILL.md`), in precedence order (agent > project > global, per
    /// [`crate::utils::paths::get_all_skills_dirs`]).
    ///
    /// Paths are canonicalized so symlink / hard-link twins that resolve to the
    /// same physical directory collapse to a single hit: a skill installed once
    /// but surfaced through several roots (e.g. `~/.aleph/skills/<id>` and
    /// `~/.claude/skills/<id>` both symlinking `~/.agents/skills/<id>`) is NOT a
    /// collision. Canonicalization failures fall back to the raw path so a real
    /// hit is never dropped. Genuine collisions (distinct physical dirs sharing
    /// an id) are preserved for the caller to resolve by precedence — mirroring
    /// codex's `seen_canonical_keys` dedup and `skill_list`'s first-occurrence
    /// win rather than hermes-agent's hard refusal.
    fn find_skill_dirs(&self, skill_id: &str) -> Vec<PathBuf> {
        let mut hits = Vec::new();
        let mut seen_canonical = std::collections::HashSet::new();
        for skills_dir in &self.skills_dirs {
            // First: the obvious match — directory whose name equals the id.
            let skill_dir = skills_dir.join(skill_id);
            if skill_dir.is_dir() && skill_dir.join("SKILL.md").exists() {
                let key = fs::canonicalize(&skill_dir).unwrap_or_else(|_| skill_dir.clone());
                if seen_canonical.insert(key) {
                    hits.push(skill_dir);
                }
            }
            // Fallback: scan sibling dirs whose SKILL.md frontmatter slugs to
            // the requested id (manifest `name` may differ from the on-disk
            // dir, e.g. hub installs). Without this the id the registry /
            // prompt advertises cannot be resolved by `skill_read`.
            if let Ok(entries) = std::fs::read_dir(skills_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if !path.is_dir() || !path.join("SKILL.md").exists() {
                        continue;
                    }
                    if let Some(slug) = slug_from_skill_md(&path) {
                        if slug == skill_id
                            && seen_canonical
                                .insert(fs::canonicalize(&path).unwrap_or_else(|_| path.clone()))
                        {
                            hits.push(path);
                        }
                    }
                }
            }
        }
        hits
    }

    /// Read the first `name:` frontmatter line from `<skill_dir>/SKILL.md`
    /// and return the same slug the registry uses (lowercase, spaces → `-`).
    /// Returns `None` for any I/O / parse failure so the caller can simply
    /// skip that directory instead of erroring out of the whole lookup.
    #[allow(dead_code)]
    fn skill_md_slug(skill_dir: &std::path::Path) -> Option<String> {
        slug_from_skill_md(skill_dir)
    }

    /// Validate `skill_id` to prevent path traversal attacks
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

    /// Execute the `read_skill` operation (internal implementation)
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

        // Resolve `skill_id` to a directory across all configured locations.
        // `find_skill_dirs` already folded symlink twins, so any remaining
        // multi-hit is a genuine same-name collision across distinct physical
        // dirs. Resolve it by precedence — the highest-priority root wins
        // (agent > project > global) — matching `skill_list`'s first-occurrence
        // dedup and Claude Code's "closer skill wins". A hard refusal here only
        // stranded the caller: the skill index the model is shown already
        // surfaces exactly one entry per id, so refusing to read what it was
        // shown is an internal contradiction (it fell back to a raw `cat` loop).
        // Shadowed dirs are logged so genuine collisions stay observable.
        let mut candidates = self.find_skill_dirs(&args.skill_id).into_iter();
        let Some(skill_dir) = candidates.next() else {
            let error_msg = format!("Skill '{}' not found", args.skill_id);
            notify_tool_result(Self::NAME, &error_msg, false);
            return Err(ToolError::NotFound(error_msg));
        };
        let shadowed: Vec<String> = candidates.map(|p| p.display().to_string()).collect();
        if !shadowed.is_empty() {
            warn!(
                skill_id = %args.skill_id,
                winner = %skill_dir.display(),
                shadowed = ?shadowed,
                "skill id resolves to multiple distinct locations; using highest-precedence match"
            );
        }

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

        // Symlink-safe containment: the lexical check above does not resolve
        // symlinks, so a `Normal`-component path that traverses a symlink could
        // still escape the skill dir. Canonicalize both and re-check. Both the
        // skill dir and the resolved target share the same real root, so the
        // supported symlinked-skill-dir twins keep working while a symlink that
        // points outside the skill is rejected. Fail closed if either path
        // cannot be resolved (the file already passed exists()/is_file()).
        let real_base = fs::canonicalize(&skill_dir)
            .map_err(|e| ToolError::Execution(format!("Failed to resolve skill dir: {e}")))?;
        let real_target = fs::canonicalize(&file_path)
            .map_err(|e| ToolError::Execution(format!("Failed to resolve file path: {e}")))?;
        if !real_target.starts_with(&real_base) {
            return Err(ToolError::InvalidArgs(
                "file_name escapes the skill directory".into(),
            ));
        }

        // Check file size
        let metadata = fs::metadata(&file_path)
            .map_err(|e| ToolError::Execution(format!("Failed to read file metadata: {e}")))?;

        if metadata.len() > self.max_file_size {
            let error_msg = format!(
                "File too large: {} bytes (max: {} bytes)",
                metadata.len(),
                self.max_file_size
            );
            notify_tool_result(Self::NAME, &error_msg, false);
            return Err(ToolError::Execution(error_msg));
        }

        // Read file content
        let raw_content = fs::read_to_string(&file_path)
            .map_err(|e| ToolError::Execution(format!("Failed to read file: {e}")))?;

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

        // Steer the model to read supporting files through the skill mechanism
        // rather than `cat`-ing them off the absolute `location` path. Only
        // meaningful when the skill ships supporting files (`available_files`
        // already excludes the primary SKILL.md).
        let usage_hint = if available_files.is_empty() {
            String::new()
        } else {
            format!(
                "To read a supporting file in this skill, call \
                 skill_read(skill_id=\"{}\", file_name=\"<one of available_files>\"). \
                 Do not cat files from `location`.",
                args.skill_id
            )
        };

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
            usage_hint,
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

/// Read the first `name:` frontmatter line from `<skill_dir>/SKILL.md`
/// and return the same slug the registry uses (lowercase, spaces → `-`).
/// Returns `None` for any I/O / parse failure so the caller can simply
/// skip that directory instead of erroring out of the whole lookup.
fn slug_from_skill_md(skill_dir: &std::path::Path) -> Option<String> {
    let body = std::fs::read_to_string(skill_dir.join("SKILL.md")).ok()?;
    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("name:") {
            let name = rest.trim().trim_matches(['"', '\'']).trim();
            if name.is_empty() {
                return None;
            }
            return Some(name.to_lowercase().replace(' ', "-"));
        }
    }
    None
}

/// Implementation of `AlephTool` trait for `ReadSkillTool`
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

To read a skill's bundled resources (REFERENCE.md, CHECKLIST.md, or any entry in
the returned `available_files`), you MUST call this tool again with file_name —
do NOT `cat` or `file_read` them by absolute path:
- skill.read(skill_id="code-review", file_name="CHECKLIST.md")
Reading a resource by its absolute `location` path skips the preprocessing below
and yields stale content; use `location` ONLY to execute bundled scripts.

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
