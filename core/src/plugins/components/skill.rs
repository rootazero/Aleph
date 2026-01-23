//! Skill and Command parser
//!
//! Parses Claude Code compatible skill and command files.
//!
//! ## File Formats
//!
//! Commands support two formats:
//! - Simple: `commands/name.md` - Direct markdown file (recommended for simple commands)
//! - Directory: `commands/name/SKILL.md` - For commands with supporting files
//!
//! Skills always use directory format:
//! - `skills/name/SKILL.md` - With optional supporting files

use std::path::Path;

use crate::plugins::error::{PluginError, PluginResult};
use crate::plugins::types::{PluginSkill, SkillFrontmatter, SkillType};

/// Skill loader for parsing skill and command files
#[derive(Debug, Default)]
pub struct SkillLoader;

impl SkillLoader {
    /// Create a new skill loader
    pub fn new() -> Self {
        Self
    }

    /// Load commands from a commands/ directory
    ///
    /// Supports two formats:
    /// - `commands/name.md` - Simple markdown file
    /// - `commands/name/SKILL.md` - Directory with SKILL.md
    pub fn load_commands(&self, dir: &Path, plugin_name: &str) -> PluginResult<Vec<PluginSkill>> {
        let mut skills = Vec::new();

        if !dir.exists() {
            return Ok(skills);
        }

        let entries = std::fs::read_dir(dir)?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            // Get the name (file stem or directory name)
            let name = path
                .file_stem()
                .or_else(|| path.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();

            // Skip hidden files/directories
            if name.starts_with('.') {
                continue;
            }

            if path.is_dir() {
                // Directory format: commands/name/SKILL.md
                let skill_md = path.join("SKILL.md");
                if skill_md.exists() {
                    match self.parse_skill_file(&skill_md, plugin_name, &name, SkillType::Command) {
                        Ok(skill) => skills.push(skill),
                        Err(e) => {
                            tracing::warn!("Failed to parse command {:?}: {}", skill_md, e);
                        }
                    }
                }
            } else if path.extension().map(|e| e == "md").unwrap_or(false) {
                // Simple format: commands/name.md
                match self.parse_skill_file(&path, plugin_name, &name, SkillType::Command) {
                    Ok(skill) => skills.push(skill),
                    Err(e) => {
                        tracing::warn!("Failed to parse command {:?}: {}", path, e);
                    }
                }
            }
        }

        Ok(skills)
    }

    /// Load skills from a skills/ directory
    ///
    /// Skills always use directory format: `skills/name/SKILL.md`
    pub fn load_skills(&self, dir: &Path, plugin_name: &str) -> PluginResult<Vec<PluginSkill>> {
        let mut skills = Vec::new();

        if !dir.exists() {
            return Ok(skills);
        }

        let entries = std::fs::read_dir(dir)?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            if !path.is_dir() {
                continue;
            }

            let skill_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();

            // Skip hidden directories
            if skill_name.starts_with('.') {
                continue;
            }

            let skill_md = path.join("SKILL.md");
            if skill_md.exists() {
                match self.parse_skill_file(&skill_md, plugin_name, &skill_name, SkillType::Skill) {
                    Ok(skill) => skills.push(skill),
                    Err(e) => {
                        tracing::warn!("Failed to parse skill {:?}: {}", skill_md, e);
                    }
                }
            }
        }

        Ok(skills)
    }

    /// Parse a single SKILL.md file
    fn parse_skill_file(
        &self,
        path: &Path,
        plugin_name: &str,
        skill_name: &str,
        skill_type: SkillType,
    ) -> PluginResult<PluginSkill> {
        let content = std::fs::read_to_string(path)?;
        self.parse_skill_content(&content, plugin_name, skill_name, skill_type, path)
    }

    /// Parse SKILL.md content
    pub fn parse_skill_content(
        &self,
        content: &str,
        plugin_name: &str,
        skill_name: &str,
        skill_type: SkillType,
        path: &Path,
    ) -> PluginResult<PluginSkill> {
        let (frontmatter, body) = parse_frontmatter(content)?;

        // Parse YAML frontmatter
        let fm: SkillFrontmatter = if frontmatter.is_empty() {
            SkillFrontmatter::default()
        } else {
            serde_yaml::from_str(&frontmatter).map_err(|e| PluginError::SkillParseError {
                path: path.to_path_buf(),
                reason: format!("Invalid YAML frontmatter: {}", e),
            })?
        };

        // Use frontmatter name if provided, otherwise use directory name
        let name = fm.name.unwrap_or_else(|| skill_name.to_string());

        // Use frontmatter description if provided, otherwise extract from body
        let description = fm
            .description
            .unwrap_or_else(|| extract_description(&body));

        Ok(PluginSkill {
            plugin_name: plugin_name.to_string(),
            skill_name: name,
            skill_type,
            description,
            content: body.trim().to_string(),
            disable_model_invocation: fm.disable_model_invocation,
        })
    }
}

/// Parse YAML frontmatter from markdown content
///
/// Returns (frontmatter, body) tuple.
fn parse_frontmatter(content: &str) -> PluginResult<(String, String)> {
    let content = content.trim();

    // Check if content starts with frontmatter delimiter
    if !content.starts_with("---") {
        return Ok((String::new(), content.to_string()));
    }

    // Find the closing delimiter
    let rest = &content[3..];
    let end_pos = rest.find("\n---");

    match end_pos {
        Some(pos) => {
            let frontmatter = rest[..pos].trim().to_string();
            let body = rest[pos + 4..].to_string();
            Ok((frontmatter, body))
        }
        None => {
            // No closing delimiter, treat entire content as body
            Ok((String::new(), content.to_string()))
        }
    }
}

/// Extract a description from markdown body
///
/// Takes the first paragraph or first line as description.
fn extract_description(body: &str) -> String {
    let body = body.trim();

    // Skip headers
    let body = body
        .lines()
        .skip_while(|line| line.starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");

    let body = body.trim();

    // Take first paragraph
    let first_para = body.split("\n\n").next().unwrap_or(body);

    // Take first line of paragraph, limit length
    let first_line = first_para.lines().next().unwrap_or("");
    let description = first_line.trim();

    if description.len() > 100 {
        format!("{}...", &description[..97])
    } else {
        description.to_string()
    }
}

/// Substitute $ARGUMENTS placeholder in skill content
pub fn substitute_arguments(content: &str, arguments: &str) -> String {
    content.replace("$ARGUMENTS", arguments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_parse_frontmatter() {
        let content = r#"---
description: Test skill
name: my-skill
---

# Skill Content

Do something with $ARGUMENTS"#;

        let (fm, body) = parse_frontmatter(content).unwrap();
        assert!(fm.contains("description: Test skill"));
        assert!(fm.contains("name: my-skill"));
        assert!(body.contains("# Skill Content"));
    }

    #[test]
    fn test_parse_frontmatter_no_frontmatter() {
        let content = "# Just Markdown\n\nNo frontmatter here.";
        let (fm, body) = parse_frontmatter(content).unwrap();
        assert!(fm.is_empty());
        assert!(body.contains("# Just Markdown"));
    }

    #[test]
    fn test_parse_skill_content() {
        let loader = SkillLoader::new();
        let content = r#"---
description: Say hello to the user
disable-model-invocation: true
---

# Hello Command

Greet $ARGUMENTS warmly."#;

        let skill = loader
            .parse_skill_content(
                content,
                "test-plugin",
                "hello",
                SkillType::Command,
                Path::new("/test"),
            )
            .unwrap();

        assert_eq!(skill.plugin_name, "test-plugin");
        assert_eq!(skill.skill_name, "hello");
        assert_eq!(skill.description, "Say hello to the user");
        assert!(skill.disable_model_invocation);
        assert!(skill.content.contains("Greet $ARGUMENTS warmly"));
    }

    #[test]
    fn test_parse_skill_with_custom_name() {
        let loader = SkillLoader::new();
        let content = r#"---
name: custom-name
description: Custom named skill
---

Content here."#;

        let skill = loader
            .parse_skill_content(
                content,
                "test-plugin",
                "directory-name",
                SkillType::Skill,
                Path::new("/test"),
            )
            .unwrap();

        assert_eq!(skill.skill_name, "custom-name");
    }

    #[test]
    fn test_substitute_arguments() {
        let content = "Hello $ARGUMENTS, welcome!";
        let result = substitute_arguments(content, "World");
        assert_eq!(result, "Hello World, welcome!");
    }

    #[test]
    fn test_extract_description() {
        let body = "# Header\n\nThis is the first paragraph.\n\nSecond paragraph.";
        let desc = extract_description(body);
        assert_eq!(desc, "This is the first paragraph.");
    }

    #[test]
    fn test_load_commands_directory_format() {
        let temp = TempDir::new().unwrap();
        let commands_dir = temp.path().join("commands");

        // Create a command using directory format
        let hello_dir = commands_dir.join("hello");
        fs::create_dir_all(&hello_dir).unwrap();
        fs::write(
            hello_dir.join("SKILL.md"),
            r#"---
description: Say hello
---

Say hello to $ARGUMENTS"#,
        )
        .unwrap();

        let loader = SkillLoader::new();
        let skills = loader.load_commands(&commands_dir, "test-plugin").unwrap();

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].skill_name, "hello");
        assert_eq!(skills[0].skill_type, SkillType::Command);
    }

    #[test]
    fn test_load_commands_simple_md_format() {
        let temp = TempDir::new().unwrap();
        let commands_dir = temp.path().join("commands");
        fs::create_dir_all(&commands_dir).unwrap();

        // Create commands using simple .md file format
        fs::write(
            commands_dir.join("commit.md"),
            r#"---
description: Create a git commit
---

Create a commit with message: $ARGUMENTS"#,
        )
        .unwrap();

        fs::write(
            commands_dir.join("review.md"),
            r#"---
description: Review code changes
---

Review the current code changes."#,
        )
        .unwrap();

        let loader = SkillLoader::new();
        let skills = loader.load_commands(&commands_dir, "test-plugin").unwrap();

        assert_eq!(skills.len(), 2);
        assert!(skills.iter().any(|s| s.skill_name == "commit"));
        assert!(skills.iter().any(|s| s.skill_name == "review"));

        let commit = skills.iter().find(|s| s.skill_name == "commit").unwrap();
        assert_eq!(commit.description, "Create a git commit");
        assert_eq!(commit.skill_type, SkillType::Command);
    }

    #[test]
    fn test_load_commands_mixed_formats() {
        let temp = TempDir::new().unwrap();
        let commands_dir = temp.path().join("commands");
        fs::create_dir_all(&commands_dir).unwrap();

        // Simple .md file format
        fs::write(
            commands_dir.join("simple.md"),
            r#"---
description: Simple command
---

Do simple things."#,
        )
        .unwrap();

        // Directory format with SKILL.md
        let complex_dir = commands_dir.join("complex");
        fs::create_dir_all(&complex_dir).unwrap();
        fs::write(
            complex_dir.join("SKILL.md"),
            r#"---
description: Complex command with scripts
---

Do complex things with supporting files."#,
        )
        .unwrap();

        let loader = SkillLoader::new();
        let skills = loader.load_commands(&commands_dir, "test-plugin").unwrap();

        assert_eq!(skills.len(), 2);
        assert!(skills.iter().any(|s| s.skill_name == "simple"));
        assert!(skills.iter().any(|s| s.skill_name == "complex"));
    }

    #[test]
    fn test_load_commands_skips_hidden_files() {
        let temp = TempDir::new().unwrap();
        let commands_dir = temp.path().join("commands");
        fs::create_dir_all(&commands_dir).unwrap();

        // Hidden file should be skipped
        fs::write(
            commands_dir.join(".hidden.md"),
            "---\ndescription: Hidden\n---\nHidden content",
        )
        .unwrap();

        // Visible file should be loaded
        fs::write(
            commands_dir.join("visible.md"),
            "---\ndescription: Visible\n---\nVisible content",
        )
        .unwrap();

        let loader = SkillLoader::new();
        let skills = loader.load_commands(&commands_dir, "test-plugin").unwrap();

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].skill_name, "visible");
    }
}
