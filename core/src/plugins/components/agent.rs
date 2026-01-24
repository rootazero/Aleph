//! Agent definition parser
//!
//! Parses Claude Code compatible agent.md files from agents/ directory.

use std::path::Path;

use crate::plugins::error::{PluginError, PluginResult};
use crate::plugins::types::{AgentFrontmatter, PluginAgent};

/// Safely truncate a string at character boundaries (UTF-8 safe)
fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let end_byte = s
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    format!("{}...", &s[..end_byte])
}

/// Agent loader for parsing agent.md files
#[derive(Debug, Default)]
pub struct AgentLoader;

impl AgentLoader {
    /// Create a new agent loader
    pub fn new() -> Self {
        Self
    }

    /// Load all agents from an agents/ directory
    pub fn load_all(&self, dir: &Path, plugin_name: &str) -> PluginResult<Vec<PluginAgent>> {
        let mut agents = Vec::new();

        if !dir.exists() {
            return Ok(agents);
        }

        let entries = std::fs::read_dir(dir)?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            // Handle both directory-based (agents/name/agent.md) and file-based (agents/name.md)
            let (agent_name, agent_file) = if path.is_dir() {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();

                // Skip hidden directories
                if name.starts_with('.') {
                    continue;
                }

                let file = path.join("agent.md");
                if !file.exists() {
                    continue;
                }
                (name, file)
            } else if path.extension().map(|e| e == "md").unwrap_or(false) {
                let name = path
                    .file_stem()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();

                // Skip hidden files
                if name.starts_with('.') {
                    continue;
                }

                (name, path.clone())
            } else {
                continue;
            };

            match self.parse_agent_file(&agent_file, plugin_name, &agent_name) {
                Ok(agent) => agents.push(agent),
                Err(e) => {
                    tracing::warn!("Failed to parse agent {:?}: {}", agent_file, e);
                }
            }
        }

        Ok(agents)
    }

    /// Parse a single agent file
    fn parse_agent_file(
        &self,
        path: &Path,
        plugin_name: &str,
        agent_name: &str,
    ) -> PluginResult<PluginAgent> {
        let content = std::fs::read_to_string(path)?;
        self.parse_agent_content(&content, plugin_name, agent_name, path)
    }

    /// Parse agent content
    pub fn parse_agent_content(
        &self,
        content: &str,
        plugin_name: &str,
        agent_name: &str,
        path: &Path,
    ) -> PluginResult<PluginAgent> {
        let (frontmatter, body) = parse_frontmatter(content)?;

        // Parse YAML frontmatter
        let fm: AgentFrontmatter = if frontmatter.is_empty() {
            AgentFrontmatter::default()
        } else {
            serde_yaml::from_str(&frontmatter).map_err(|e| PluginError::AgentParseError {
                path: path.to_path_buf(),
                reason: format!("Invalid YAML frontmatter: {}", e),
            })?
        };

        let description = fm
            .description
            .unwrap_or_else(|| extract_description(&body));

        Ok(PluginAgent {
            plugin_name: plugin_name.to_string(),
            agent_name: agent_name.to_string(),
            description,
            capabilities: fm.capabilities,
            system_prompt: body.trim().to_string(),
        })
    }
}

/// Parse YAML frontmatter from markdown content
fn parse_frontmatter(content: &str) -> PluginResult<(String, String)> {
    let content = content.trim();

    if !content.starts_with("---") {
        return Ok((String::new(), content.to_string()));
    }

    let rest = &content[3..];
    let end_pos = rest.find("\n---");

    match end_pos {
        Some(pos) => {
            let frontmatter = rest[..pos].trim().to_string();
            let body = rest[pos + 4..].to_string();
            Ok((frontmatter, body))
        }
        None => Ok((String::new(), content.to_string())),
    }
}

/// Extract description from body
fn extract_description(body: &str) -> String {
    let body = body.trim();
    let body = body
        .lines()
        .skip_while(|line| line.starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");

    let first_para = body.trim().split("\n\n").next().unwrap_or("");
    let first_line = first_para.lines().next().unwrap_or("");

    truncate_str(first_line, 50)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_parse_agent_content() {
        let loader = AgentLoader::new();
        let content = r#"---
description: Code review specialist
capabilities:
  - review code for bugs
  - suggest improvements
  - check security issues
---

# Code Reviewer Agent

You are a code review specialist. When given code, analyze it for:
- Bugs and logic errors
- Performance issues
- Security vulnerabilities
"#;

        let agent = loader
            .parse_agent_content(content, "test-plugin", "reviewer", Path::new("/test"))
            .unwrap();

        assert_eq!(agent.plugin_name, "test-plugin");
        assert_eq!(agent.agent_name, "reviewer");
        assert_eq!(agent.description, "Code review specialist");
        assert_eq!(agent.capabilities.len(), 3);
        assert!(agent.system_prompt.contains("code review specialist"));
    }

    #[test]
    fn test_parse_agent_without_frontmatter() {
        let loader = AgentLoader::new();
        let content = "# Simple Agent\n\nDo simple things.";

        let agent = loader
            .parse_agent_content(content, "plugin", "simple", Path::new("/test"))
            .unwrap();

        assert_eq!(agent.description, "Do simple things.");
        assert!(agent.capabilities.is_empty());
    }

    #[test]
    fn test_load_agents_directory() {
        let temp = TempDir::new().unwrap();
        let agents_dir = temp.path().join("agents");

        // Create directory-based agent
        let reviewer_dir = agents_dir.join("reviewer");
        fs::create_dir_all(&reviewer_dir).unwrap();
        fs::write(
            reviewer_dir.join("agent.md"),
            r#"---
description: Reviews code
capabilities: [review]
---

Review code."#,
        )
        .unwrap();

        // Create file-based agent
        fs::write(
            agents_dir.join("helper.md"),
            r#"---
description: Helps with tasks
---

Help with tasks."#,
        )
        .unwrap();

        let loader = AgentLoader::new();
        let agents = loader.load_all(&agents_dir, "test-plugin").unwrap();

        assert_eq!(agents.len(), 2);
        assert!(agents.iter().any(|a| a.agent_name == "reviewer"));
        assert!(agents.iter().any(|a| a.agent_name == "helper"));
    }

    #[test]
    fn test_qualified_name() {
        let agent = PluginAgent {
            plugin_name: "my-plugin".to_string(),
            agent_name: "reviewer".to_string(),
            description: "Test".to_string(),
            capabilities: vec![],
            system_prompt: "Test".to_string(),
        };

        assert_eq!(agent.qualified_name(), "my-plugin:reviewer");
    }
}
