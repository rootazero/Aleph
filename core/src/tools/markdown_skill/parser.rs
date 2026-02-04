//! Markdown Skill Parser
//!
//! Parses SKILL.md files with YAML frontmatter into AetherSkillSpec.

use anyhow::{Context, Result};
use super::spec::AetherSkillSpec;

/// Parse a SKILL.md file into AetherSkillSpec
pub fn parse_skill_file(content: &str) -> Result<AetherSkillSpec> {
    // 1. Split frontmatter and content
    let (frontmatter, markdown) = extract_frontmatter(content)?;

    // 2. Parse YAML frontmatter
    let mut spec: AetherSkillSpec = serde_yaml::from_str(&frontmatter)
        .context("Failed to parse skill frontmatter")?;

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

    // Find closing delimiter
    let after_first = &content[3..];
    if let Some(end_pos) = after_first.find("\n---\n") {
        let frontmatter = &after_first[..end_pos].trim();
        let markdown = &after_first[end_pos + 5..].trim();
        Ok((frontmatter, markdown))
    } else {
        anyhow::bail!("Frontmatter must be closed with --- on a new line");
    }
}

/// Validate spec has required fields
fn validate_spec(spec: &AetherSkillSpec) -> Result<()> {
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

/// Extract specific sections from markdown (e.g., ## Examples)
pub fn extract_markdown_section(content: &str, heading: &str) -> Option<String> {
    let search = format!("## {}", heading);

    if let Some(start) = content.find(&search) {
        let after_heading = &content[start + search.len()..];

        // Find next ## heading or end of document
        let end = after_heading
            .find("\n## ")
            .unwrap_or(after_heading.len());

        Some(after_heading[..end].trim().to_string())
    } else {
        None
    }
}

/// Extract first paragraph as short description
pub fn extract_first_paragraph(content: &str) -> String {
    content
        .lines()
        .skip_while(|l| l.trim().is_empty() || l.starts_with('#'))
        .take_while(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(200)
        .collect()
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
        assert!(result.unwrap_err().to_string().contains("name cannot be empty"));
    }

    #[test]
    fn test_extract_markdown_section() {
        let content = r#"
# Title

Some intro text.

## Examples

```bash
command --flag value
```

## Notes

Some notes here.
"#;
        let examples = extract_markdown_section(content, "Examples").unwrap();
        assert!(examples.contains("```bash"));
        assert!(examples.contains("command --flag value"));
        assert!(!examples.contains("## Notes"));
    }

    #[test]
    fn test_extract_missing_section() {
        let content = "# Title\n\nNo examples here.";
        let result = extract_markdown_section(content, "Examples");
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_first_paragraph() {
        let content = r#"
# Title

This is the first paragraph.
It has multiple lines.

This is the second paragraph.
"#;
        let para = extract_first_paragraph(content);
        assert_eq!(para, "This is the first paragraph. It has multiple lines.");
    }

    #[test]
    fn test_extract_first_paragraph_long() {
        let content = format!("# Title\n\n{}", "A".repeat(300));
        let para = extract_first_paragraph(&content);
        assert_eq!(para.len(), 200); // Truncated to 200 chars
    }
}
