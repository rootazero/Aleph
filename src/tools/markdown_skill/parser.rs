//! Markdown Skill Parser
//!
//! Parses SKILL.md files with YAML frontmatter into `AlephSkillSpec`.

use super::spec::AlephSkillSpec;
use anyhow::{Context, Result};

/// Parse a SKILL.md file into `AlephSkillSpec`
pub fn parse_skill_file(content: &str) -> Result<AlephSkillSpec> {
    // Normalize Windows line endings so "\n---\n" delimiter matching works
    let content = content.replace("\r\n", "\n");
    // 1. Split frontmatter and content
    let (frontmatter, markdown) = extract_frontmatter(&content)?;

    // 2. Parse YAML frontmatter
    let mut spec: AlephSkillSpec =
        serde_yaml::from_str(frontmatter).context("Failed to parse skill frontmatter")?;

    // 3. Attach markdown content
    spec.markdown_content = markdown.to_string();

    // 4. Validate required fields
    validate_spec(&spec)?;

    Ok(spec)
}

/// Extract YAML frontmatter and markdown body
fn extract_frontmatter(content: &str) -> Result<(&str, &str)> {
    let content = content.trim_start();

    // Check for frontmatter delimiter
    if !content.starts_with("---") {
        anyhow::bail!("Skill file must start with YAML frontmatter (---)");
    }

    // Find closing delimiter ("---" is ASCII, so byte offset 3 is always a char boundary)
    let after_first = content
        .get(3..)
        .ok_or_else(|| anyhow::anyhow!("Frontmatter too short"))?;

    // Try "\n---\n" first, then "\n---" at end-of-string (no trailing newline)
    let end_pos = after_first
        .find("\n---\n")
        .or_else(|| {
            let suffix = "\n---";
            if after_first.ends_with(suffix) {
                Some(after_first.len() - suffix.len())
            } else {
                None
            }
        })
        .ok_or_else(|| anyhow::anyhow!("Frontmatter must be closed with --- on a new line"))?;

    let frontmatter = after_first
        .get(..end_pos)
        .ok_or_else(|| anyhow::anyhow!("Invalid frontmatter boundary"))?
        .trim();
    // After "\n---\n" there are 5 bytes; after "\n---" at EOF there may be nothing
    let markdown_start = end_pos + 4; // skip "\n---"
    let markdown_start = if after_first.get(markdown_start..markdown_start + 1) == Some("\n") {
        markdown_start + 1
    } else {
        markdown_start
    };
    let markdown = after_first.get(markdown_start..).unwrap_or("").trim();
    Ok((frontmatter, markdown))
}

/// Validate spec has required fields
fn validate_spec(spec: &AlephSkillSpec) -> Result<()> {
    if spec.name.is_empty() {
        anyhow::bail!("Skill name cannot be empty");
    }

    if spec.description.is_empty() {
        anyhow::bail!("Skill description cannot be empty");
    }

    // Check required binaries exist (optional: can be a warning instead)
    for bin in &spec.metadata.requires.bins {
        if bin.is_empty() {
            anyhow::bail!("Required binary name cannot be empty");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic_skill() {
        let content = r#"---
name: test-tool
description: A test tool
metadata:
  requires:
    bins: ["gh"]
---
# Test Tool
Use this tool for testing.

## Examples
```bash
gh pr list
```
"#;
        let spec = parse_skill_file(content).unwrap();
        assert_eq!(spec.name, "test-tool");
        assert_eq!(spec.description, "A test tool");
        assert!(spec.markdown_content.contains("Use this tool"));
    }

    #[test]
    fn test_parse_windows_line_endings() {
        let content = "---\r\nname: win-tool\r\ndescription: A Windows-authored tool\r\nmetadata:\r\n  requires:\r\n    bins: [\"gh\"]\r\n---\r\n# Win Tool\r\nWorks on Windows.\r\n";
        let spec = parse_skill_file(content).unwrap();
        assert_eq!(spec.name, "win-tool");
        assert!(spec.markdown_content.contains("Works on Windows"));
    }

    #[test]
    fn test_parse_missing_frontmatter() {
        let content = "# No frontmatter here";
        let result = parse_skill_file(content);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must start with YAML frontmatter"));
    }

    #[test]
    fn test_parse_unclosed_frontmatter() {
        let content = "---\nname: test\n# No closing delimiter";
        let result = parse_skill_file(content);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must be closed with ---"));
    }

    #[test]
    fn test_parse_invalid_yaml() {
        let content = "---\n{{{invalid yaml\n---\nContent";
        let result = parse_skill_file(content);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_empty_name() {
        let content = r#"---
name: ""
description: Test
metadata:
  requires:
    bins: []
---
Content"#;
        let result = parse_skill_file(content);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("name cannot be empty"));
    }
}
