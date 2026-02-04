//! Skills module - Claude Agent Skills standard implementation.
//!
//! This module implements the Claude Agent Skills open standard for dynamic
//! instruction injection. Skills are not executable code; they inject comprehensive
//! instruction sets that modify how Claude approaches tasks.
//!
//! # SKILL.md Format
//!
//! ```markdown
//! ---
//! name: refine-text
//! description: Improve and polish writing.
//! allowed-tools:
//!   - Read
//!   - Edit
//! ---
//!
//! # Refine Text Skill
//!
//! When refining text, follow these principles:
//! 1. **Clarity**: Remove ambiguity
//! 2. **Conciseness**: Eliminate redundancy
//! ```
//!
//! # Architecture
//!
//! - `Skill`: Core data structure representing a parsed SKILL.md
//! - `SkillsRegistry`: Manages loaded skills from the skills directory
//! - `SkillsInstaller`: Downloads and installs skills from GitHub/ZIP
//!
pub mod cli_wrapper;
pub mod health;
pub mod installer;
pub mod registry;
pub mod types;

use crate::error::{AlephError, Result};
use serde::{Deserialize, Serialize};

/// SKILL.md frontmatter structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    /// Skill name (used as identifier)
    pub name: String,

    /// Human-readable description
    pub description: String,

    /// Allowed tools for this skill (reserved for MCP integration)
    #[serde(default, rename = "allowed-tools")]
    pub allowed_tools: Vec<String>,

    /// Trigger keywords for natural language command detection
    /// When user input contains any of these keywords, this skill may be auto-invoked.
    #[serde(default)]
    pub triggers: Vec<String>,

    // === New fields (all optional for backwards compatibility) ===

    /// UI icon emoji (e.g., "🐙")
    #[serde(default)]
    pub emoji: Option<String>,

    /// Category tag (e.g., "developer", "media", "productivity")
    #[serde(default)]
    pub category: Option<String>,

    /// Whether this skill is a CLI wrapper
    #[serde(default, rename = "cli-wrapper")]
    pub cli_wrapper: bool,

    /// Dependency requirements
    #[serde(default)]
    pub requirements: Option<types::SkillRequirements>,
}

/// A parsed Skill from SKILL.md
#[derive(Debug, Clone)]
pub struct Skill {
    /// Skill ID (directory name)
    pub id: String,

    /// Parsed frontmatter
    pub frontmatter: SkillFrontmatter,

    /// Markdown instructions (body after frontmatter)
    pub instructions: String,
}

impl Skill {
    /// Parse a SKILL.md file content
    ///
    /// # Arguments
    ///
    /// * `id` - The skill ID (typically directory name)
    /// * `content` - Full content of SKILL.md file
    ///
    /// # Returns
    ///
    /// A parsed Skill or error if format is invalid
    pub fn parse(id: &str, content: &str) -> Result<Self> {
        let content = content.trim();

        // Check for YAML frontmatter delimiter
        if !content.starts_with("---") {
            return Err(AlephError::invalid_config(
                "SKILL.md must start with YAML frontmatter (---)",
            ));
        }

        // Find the closing delimiter
        let rest = &content[3..]; // Skip opening ---
        let end_pos = rest.find("\n---").ok_or_else(|| {
            AlephError::invalid_config("SKILL.md missing closing frontmatter delimiter (---)")
        })?;

        // Extract frontmatter YAML
        let frontmatter_yaml = &rest[..end_pos].trim();
        let after_frontmatter = &rest[end_pos + 4..]; // Skip \n---

        // Parse YAML frontmatter
        let frontmatter: SkillFrontmatter =
            serde_yaml::from_str(frontmatter_yaml).map_err(|e| {
                AlephError::invalid_config(format!("Invalid SKILL.md frontmatter YAML: {}", e))
            })?;

        // Validate required fields
        if frontmatter.name.is_empty() {
            return Err(AlephError::invalid_config(
                "SKILL.md frontmatter missing required 'name' field",
            ));
        }

        if frontmatter.description.is_empty() {
            return Err(AlephError::invalid_config(
                "SKILL.md frontmatter missing required 'description' field",
            ));
        }

        // Extract markdown body (instructions)
        let instructions = after_frontmatter.trim().to_string();

        Ok(Self {
            id: id.to_string(),
            frontmatter,
            instructions,
        })
    }

    /// Get the skill name
    pub fn name(&self) -> &str {
        &self.frontmatter.name
    }

    /// Get the skill description
    pub fn description(&self) -> &str {
        &self.frontmatter.description
    }

    /// Get allowed tools
    pub fn allowed_tools(&self) -> &[String] {
        &self.frontmatter.allowed_tools
    }

    /// Convert to SkillInfo for FFI/UI display
    pub fn to_info(&self) -> SkillInfo {
        SkillInfo {
            id: self.id.clone(),
            name: self.frontmatter.name.clone(),
            description: self.frontmatter.description.clone(),
            triggers: self.frontmatter.triggers.clone(),
            allowed_tools: self.frontmatter.allowed_tools.clone(),
        }
    }
}

/// Skill information for UI display
///
/// A simplified view of Skill without the full instructions.
#[derive(Debug, Clone)]
pub struct SkillInfo {
    /// Skill ID (directory name)
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Description
    pub description: String,
    /// Trigger keywords for auto-detection
    pub triggers: Vec<String>,
    /// Allowed tools
    pub allowed_tools: Vec<String>,
}

// Re-exports
pub use cli_wrapper::{CliWrapperError, CliWrapperValidator};
pub use health::HealthChecker;
pub use installer::SkillsInstaller;
pub use registry::{SkillWithHealth, SkillsRegistry};
pub use types::{InstallCommand, PackageManager, SkillHealth, SkillRequirements};

use crate::utils::paths::get_skills_dir;
use std::fs;
use std::path::PathBuf;
use tracing::{debug, info, warn};

/// Initialize built-in skills (UniFFI export wrapper)
///
/// Wrapper that takes a String path for UniFFI compatibility.
///
/// # Arguments
/// * `bundle_skills_dir` - Path to the bundled skills directory as String
pub fn initialize_builtin_skills_ffi(bundle_skills_dir: String) -> Result<()> {
    let path = PathBuf::from(&bundle_skills_dir);
    initialize_builtin_skills(&path)
}

/// Initialize built-in skills
///
/// Copies built-in skills from Resources/skills to user's skills directory.
/// Never overwrites existing user skills.
///
/// # Arguments
/// * `bundle_skills_dir` - Path to the bundled skills directory (in app bundle)
pub fn initialize_builtin_skills(bundle_skills_dir: &PathBuf) -> Result<()> {
    let skills_dir = get_skills_dir()?;

    // Ensure skills directory exists
    fs::create_dir_all(&skills_dir)
        .map_err(|e| AlephError::config(format!("Failed to create skills directory: {}", e)))?;

    // Check if bundle skills directory exists
    if !bundle_skills_dir.exists() {
        info!(
            "Bundle skills directory not found at {:?}, skipping built-in skills",
            bundle_skills_dir
        );
        return Ok(());
    }

    // Iterate through bundled skills
    let entries = fs::read_dir(bundle_skills_dir).map_err(|e| {
        AlephError::config(format!("Failed to read bundle skills directory: {}", e))
    })?;

    let mut copied_count = 0;
    let mut skipped_count = 0;

    for entry in entries.flatten() {
        let path = entry.path();

        // Only process directories with SKILL.md
        if path.is_dir() {
            let skill_md = path.join("SKILL.md");
            if skill_md.exists() {
                let skill_id = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown");

                let target_dir = skills_dir.join(skill_id);

                // Never overwrite existing user skills
                if target_dir.exists() {
                    debug!(
                        skill_id = %skill_id,
                        "Skill already exists, skipping"
                    );
                    skipped_count += 1;
                    continue;
                }

                // Create skill directory and copy SKILL.md
                fs::create_dir_all(&target_dir).map_err(|e| {
                    AlephError::config(format!(
                        "Failed to create skill directory {}: {}",
                        target_dir.display(),
                        e
                    ))
                })?;

                let target_skill_md = target_dir.join("SKILL.md");
                fs::copy(&skill_md, &target_skill_md).map_err(|e| {
                    AlephError::config(format!("Failed to copy SKILL.md for {}: {}", skill_id, e))
                })?;

                info!(skill_id = %skill_id, "Installed built-in skill");
                copied_count += 1;
            }
        }
    }

    info!(
        copied = copied_count,
        skipped = skipped_count,
        "Built-in skills initialization complete"
    );

    // Reload skills registry if it exists
    let registry = SkillsRegistry::new(skills_dir);
    if let Err(e) = registry.load_all() {
        warn!("Failed to load skills after initialization: {}", e);
    }

    Ok(())
}

/// List all installed skills
///
/// Scans multiple skills directories and returns info for each valid skill.
/// Uses multi-location discovery to support both ~/.aleph/skills and ~/.claude/skills.
pub fn list_installed_skills() -> Result<Vec<SkillInfo>> {
    // Use multi-location discovery to support both ~/.aleph/skills and ~/.claude/skills
    // This enables Claude Code compatibility by scanning:
    // - Project level: .aether/skills/, .claude/skills/
    // - Global level: ~/.aleph/skills, ~/.claude/skills
    let registry = SkillsRegistry::with_auto_discover(None)?;
    registry.load_all()?;

    let skills = registry.list_skills();
    Ok(skills.into_iter().map(|s| s.to_info()).collect())
}

/// Delete a skill by ID
///
/// Removes the skill directory from the skills folder.
/// Note: Callers should refresh tool registry after deletion if needed.
pub fn delete_skill(skill_id: String) -> Result<()> {
    let skills_dir = get_skills_dir()?;
    let skill_path = skills_dir.join(&skill_id);

    if !skill_path.exists() {
        return Err(AlephError::invalid_config(format!(
            "Skill '{}' not found",
            skill_id
        )));
    }

    // Remove the entire skill directory
    fs::remove_dir_all(&skill_path).map_err(|e| {
        AlephError::config(format!("Failed to delete skill '{}': {}", skill_id, e))
    })?;

    info!(skill_id = %skill_id, "Deleted skill");
    Ok(())
}

/// Install a skill from URL
///
/// Downloads and installs a skill from a GitHub repository URL.
/// Note: Callers should refresh tool registry after installation if needed.
pub fn install_skill_from_url(url: String) -> Result<SkillInfo> {
    let skills_dir = get_skills_dir()?;

    // Ensure skills directory exists
    fs::create_dir_all(&skills_dir)
        .map_err(|e| AlephError::config(format!("Failed to create skills directory: {}", e)))?;

    // Use the installer to download and install (synchronous)
    let installer = SkillsInstaller::new(skills_dir);
    let skill = installer.install_from_url_sync(&url)?;

    info!(
        skill_id = %skill.id,
        skill_name = %skill.name(),
        "Installed skill from URL"
    );

    Ok(skill.to_info())
}

/// Install skills from ZIP
///
/// Extracts and installs skills from a ZIP archive.
/// Note: Callers should refresh tool registry after installation if needed.
pub fn install_skills_from_zip(zip_path: String) -> Result<Vec<String>> {
    let skills_dir = get_skills_dir()?;
    let zip_path = PathBuf::from(&zip_path);

    // Verify ZIP file exists
    if !zip_path.exists() {
        return Err(AlephError::invalid_config(format!(
            "ZIP file not found: {}",
            zip_path.display()
        )));
    }

    // Ensure skills directory exists
    fs::create_dir_all(&skills_dir)
        .map_err(|e| AlephError::config(format!("Failed to create skills directory: {}", e)))?;

    // Use the installer to extract and install
    let installer = SkillsInstaller::new(skills_dir);

    // Use tokio runtime to call async method
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| AlephError::config(format!("Failed to create async runtime: {}", e)))?;

    let installed = runtime.block_on(installer.install_from_zip(&zip_path))?;

    info!(count = installed.len(), "Installed skills from ZIP");

    Ok(installed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_SKILL_MD: &str = r#"---
name: refine-text
description: Improve and polish writing. Use when asked to refine text.
allowed-tools:
  - Read
  - Edit
---

# Refine Text Skill

When refining text, follow these principles:

1. **Clarity**: Remove ambiguity and improve readability
2. **Conciseness**: Eliminate redundancy without losing meaning
3. **Flow**: Ensure logical progression of ideas
"#;

    const SKILL_NO_TOOLS: &str = r#"---
name: simple-skill
description: A simple skill without tools
---

# Simple Skill

Just some instructions.
"#;

    const SKILL_EMPTY_INSTRUCTIONS: &str = r#"---
name: empty-skill
description: A skill with no instructions
---
"#;

    #[test]
    fn test_parse_valid_skill() {
        let skill = Skill::parse("refine-text", VALID_SKILL_MD).unwrap();

        assert_eq!(skill.id, "refine-text");
        assert_eq!(skill.frontmatter.name, "refine-text");
        assert_eq!(
            skill.frontmatter.description,
            "Improve and polish writing. Use when asked to refine text."
        );
        assert_eq!(skill.frontmatter.allowed_tools, vec!["Read", "Edit"]);
        assert!(skill.instructions.contains("Clarity"));
        assert!(skill.instructions.contains("Conciseness"));
    }

    #[test]
    fn test_parse_skill_no_tools() {
        let skill = Skill::parse("simple-skill", SKILL_NO_TOOLS).unwrap();

        assert_eq!(skill.frontmatter.name, "simple-skill");
        assert!(skill.frontmatter.allowed_tools.is_empty());
    }

    #[test]
    fn test_parse_skill_empty_instructions() {
        let skill = Skill::parse("empty-skill", SKILL_EMPTY_INSTRUCTIONS).unwrap();

        assert_eq!(skill.frontmatter.name, "empty-skill");
        assert!(skill.instructions.is_empty());
    }

    #[test]
    fn test_parse_missing_frontmatter() {
        let content = "# Just markdown\n\nNo frontmatter here.";
        let result = Skill::parse("test", content);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must start with YAML frontmatter"));
    }

    #[test]
    fn test_parse_missing_closing_delimiter() {
        let content = "---\nname: test\ndescription: test\n# No closing delimiter";
        let result = Skill::parse("test", content);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("missing closing frontmatter"));
    }

    #[test]
    fn test_parse_missing_name() {
        let content = r#"---
description: A skill without name
---

Instructions here.
"#;
        let result = Skill::parse("test", content);

        assert!(result.is_err());
        // serde_yaml reports missing field as "missing field `name`"
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("name") || err_msg.contains("missing"));
    }

    #[test]
    fn test_parse_missing_description() {
        let content = r#"---
name: test-skill
---

Instructions here.
"#;
        let result = Skill::parse("test", content);

        assert!(result.is_err());
    }

    #[test]
    fn test_skill_accessors() {
        let skill = Skill::parse("refine-text", VALID_SKILL_MD).unwrap();

        assert_eq!(skill.name(), "refine-text");
        assert_eq!(
            skill.description(),
            "Improve and polish writing. Use when asked to refine text."
        );
        assert_eq!(skill.allowed_tools(), &["Read", "Edit"]);
    }

    #[test]
    fn test_parse_skill_with_requirements() {
        let content = r#"---
name: github
description: GitHub CLI operations
emoji: "🐙"
category: developer
cli-wrapper: true
requirements:
  binaries:
    - gh
  platforms:
    - macos
    - linux
  install:
    - manager: brew
      package: gh
---

# GitHub Skill
"#;
        let skill = Skill::parse("github", content).unwrap();

        assert_eq!(skill.frontmatter.emoji, Some("🐙".to_string()));
        assert_eq!(skill.frontmatter.category, Some("developer".to_string()));
        assert!(skill.frontmatter.cli_wrapper);

        let req = skill.frontmatter.requirements.unwrap();
        assert_eq!(req.binaries, vec!["gh"]);
        assert_eq!(
            req.platforms,
            Some(vec!["macos".to_string(), "linux".to_string()])
        );
        assert_eq!(req.install.len(), 1);
        assert_eq!(req.install[0].package, "gh");
    }

    #[test]
    fn test_parse_skill_without_requirements_backwards_compat() {
        // Existing skills without new fields should still parse
        let skill = Skill::parse("refine-text", VALID_SKILL_MD).unwrap();

        assert!(skill.frontmatter.emoji.is_none());
        assert!(skill.frontmatter.category.is_none());
        assert!(!skill.frontmatter.cli_wrapper);
        assert!(skill.frontmatter.requirements.is_none());
    }
}
