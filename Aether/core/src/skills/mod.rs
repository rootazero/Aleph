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

pub mod installer;
pub mod registry;

use crate::error::{AetherError, Result};
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
            return Err(AetherError::invalid_config(
                "SKILL.md must start with YAML frontmatter (---)",
            ));
        }

        // Find the closing delimiter
        let rest = &content[3..]; // Skip opening ---
        let end_pos = rest.find("\n---").ok_or_else(|| {
            AetherError::invalid_config("SKILL.md missing closing frontmatter delimiter (---)")
        })?;

        // Extract frontmatter YAML
        let frontmatter_yaml = &rest[..end_pos].trim();
        let after_frontmatter = &rest[end_pos + 4..]; // Skip \n---

        // Parse YAML frontmatter
        let frontmatter: SkillFrontmatter = serde_yaml::from_str(frontmatter_yaml).map_err(|e| {
            AetherError::invalid_config(format!("Invalid SKILL.md frontmatter YAML: {}", e))
        })?;

        // Validate required fields
        if frontmatter.name.is_empty() {
            return Err(AetherError::invalid_config(
                "SKILL.md frontmatter missing required 'name' field",
            ));
        }

        if frontmatter.description.is_empty() {
            return Err(AetherError::invalid_config(
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
}

// Re-exports
pub use installer::SkillsInstaller;
pub use registry::SkillsRegistry;

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
}
