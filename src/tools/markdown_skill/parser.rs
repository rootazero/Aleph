//! Markdown Skill Parser
//!
//! Parses SKILL.md files with YAML frontmatter into `AlephSkillSpec`.

use super::spec::AlephSkillSpec;
use anyhow::{Context, Result};

/// Parse a SKILL.md file into `AlephSkillSpec`.
///
/// Note the deliberate absence of `allowed-tools:` handling. This path
/// produces a `MarkdownCliTool` — a *tool*, registered into the markdown-skill
/// tool server — never a slash command. It therefore never reaches
/// `tool_metadata::registry::registration::register_skills`, which is the only
/// place a declared tool scope is resolved and enforced. Parsing the key here
/// would be a parse that reports success and restricts nothing; the key is
/// ignored instead (`AlephSkillSpec` has no `deny_unknown_fields`, so an
/// upstream skill carrying it still loads).
///
/// # Errors
///
/// When the content has no `---` fence, the YAML does not deserialise, or
/// [`validate_spec`] rejects the result.
pub fn parse_skill_file(content: &str) -> Result<AlephSkillSpec> {
    // 1. Split frontmatter and content (CRLF is normalised by the splitter)
    let (frontmatter, markdown) = extract_frontmatter(content)?;

    // 2. Parse YAML frontmatter
    let mut spec: AlephSkillSpec =
        crate::yaml::from_str(&frontmatter).context("Failed to parse skill frontmatter")?;

    // 3. Attach markdown content
    spec.markdown_content = markdown;

    // 4. Validate required fields
    validate_spec(&spec)?;

    Ok(spec)
}

/// Extract YAML frontmatter and markdown body.
///
/// Delegates the fence detection to [`crate::skill::frontmatter::split`], the
/// one implementation shared by all three SKILL.md readers. The local version
/// cut at the first `\n---\n` **substring**, so a `---` line inside a YAML
/// block scalar ended the frontmatter early — the remaining keys became body
/// and the truncated YAML usually failed to parse, dropping the skill.
///
/// Both halves are trimmed here, which is this path's own convention (the
/// splitter returns them verbatim because `skill::manifest` wants the raw
/// body).
fn extract_frontmatter(content: &str) -> Result<(String, String)> {
    let (frontmatter, markdown) = crate::skill::frontmatter::split(content).map_err(|_| {
        anyhow::anyhow!(
            "Skill file must start with YAML frontmatter (---) and must be closed with --- \
             on a line of its own"
        )
    })?;
    Ok((frontmatter.trim().to_string(), markdown.trim().to_string()))
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
