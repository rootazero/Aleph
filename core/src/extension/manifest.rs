//! Plugin manifest parsing and validation

use super::error::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Plugin manifest (parsed from .claude-plugin/plugin.json)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Plugin name (required, used as namespace)
    pub name: String,

    /// Plugin version (semver format)
    #[serde(default)]
    pub version: Option<String>,

    /// Plugin description
    #[serde(default)]
    pub description: Option<String>,

    /// Plugin author information
    #[serde(default)]
    pub author: Option<PluginAuthor>,

    /// Plugin homepage URL
    #[serde(default)]
    pub homepage: Option<String>,

    /// Plugin repository URL
    #[serde(default)]
    pub repository: Option<PluginRepository>,

    /// Plugin license
    #[serde(default)]
    pub license: Option<String>,

    /// Keywords for search
    #[serde(default)]
    pub keywords: Option<Vec<String>>,

    // Custom paths (optional, override default locations)
    /// Custom commands directory path
    #[serde(default)]
    pub commands: Option<PathBuf>,

    /// Custom skills directory path
    #[serde(default)]
    pub skills: Option<PathBuf>,

    /// Custom agents directory path
    #[serde(default)]
    pub agents: Option<PathBuf>,

    /// Custom hooks file path
    #[serde(default)]
    pub hooks: Option<PathBuf>,

    /// Custom MCP servers file path
    #[serde(rename = "mcpServers", default)]
    pub mcp_servers: Option<PathBuf>,
}

/// Plugin author information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginAuthor {
    /// Author name
    pub name: String,

    /// Author email
    #[serde(default)]
    pub email: Option<String>,

    /// Author URL
    #[serde(default)]
    pub url: Option<String>,
}

/// Plugin repository information
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PluginRepository {
    /// Simple URL string
    Url(String),
    /// Detailed repository info
    Detailed {
        #[serde(rename = "type", default)]
        repo_type: Option<String>,
        url: String,
    },
}

/// Parse plugin manifest from file
pub async fn parse_plugin_manifest(path: &Path) -> ExtensionResult<PluginManifest> {
    let content = tokio::fs::read_to_string(path).await?;

    let manifest: PluginManifest = serde_json::from_str(&content)
        .map_err(|e| ExtensionError::invalid_manifest(path, format!("JSON parse error: {}", e)))?;

    // Validate required fields
    if manifest.name.is_empty() {
        return Err(ExtensionError::missing_field(path, "name"));
    }

    Ok(manifest)
}

/// Validate plugin name format
///
/// Plugin names must be:
/// - Lowercase
/// - Start with a letter
/// - Only contain letters, numbers, and hyphens
pub fn validate_plugin_name(name: &str) -> ExtensionResult<()> {
    if name.is_empty() {
        return Err(ExtensionError::invalid_plugin_name(
            name,
            "Plugin name cannot be empty",
        ));
    }

    let first_char = name.chars().next().unwrap();
    if !first_char.is_ascii_lowercase() {
        return Err(ExtensionError::invalid_plugin_name(
            name,
            "Plugin name must start with a lowercase letter",
        ));
    }

    for ch in name.chars() {
        if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() && ch != '-' {
            return Err(ExtensionError::invalid_plugin_name(
                name,
                format!(
                    "Invalid character '{}'. Only lowercase letters, numbers, and hyphens allowed",
                    ch
                ),
            ));
        }
    }

    // Check for consecutive hyphens
    if name.contains("--") {
        return Err(ExtensionError::invalid_plugin_name(
            name,
            "Plugin name cannot contain consecutive hyphens",
        ));
    }

    // Check for leading/trailing hyphens
    if name.starts_with('-') || name.ends_with('-') {
        return Err(ExtensionError::invalid_plugin_name(
            name,
            "Plugin name cannot start or end with a hyphen",
        ));
    }

    Ok(())
}

/// Parse YAML frontmatter from markdown content
pub fn parse_frontmatter<T: serde::de::DeserializeOwned + Default>(
    content: &str,
    path: &Path,
) -> ExtensionResult<(T, String)> {
    let content = content.trim();

    // Check for frontmatter delimiter
    if !content.starts_with("---") {
        // No frontmatter, return defaults and full content
        return Ok((T::default(), content.to_string()));
    }

    // Find end delimiter
    let rest = &content[3..];
    let end_pos = rest.find("\n---").or_else(|| rest.find("\r\n---"));

    match end_pos {
        Some(pos) => {
            let frontmatter_str = &rest[..pos].trim();
            let body_start = pos + 4; // Skip "\n---"
            let body = rest[body_start..].trim().to_string();

            // Handle empty frontmatter
            if frontmatter_str.is_empty() {
                return Ok((T::default(), body));
            }

            // Parse YAML
            let frontmatter: T = serde_yaml::from_str(frontmatter_str)
                .map_err(|e| ExtensionError::yaml_parse(path, format!("YAML error: {}", e)))?;

            Ok((frontmatter, body))
        }
        None => {
            // No closing delimiter, treat as no frontmatter
            Ok((T::default(), content.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_plugin_name_valid() {
        assert!(validate_plugin_name("my-plugin").is_ok());
        assert!(validate_plugin_name("plugin123").is_ok());
        assert!(validate_plugin_name("a").is_ok());
        assert!(validate_plugin_name("my-awesome-plugin").is_ok());
    }

    #[test]
    fn test_validate_plugin_name_invalid() {
        assert!(validate_plugin_name("").is_err());
        assert!(validate_plugin_name("My-Plugin").is_err()); // uppercase
        assert!(validate_plugin_name("123plugin").is_err()); // starts with number
        assert!(validate_plugin_name("my_plugin").is_err()); // underscore
        assert!(validate_plugin_name("my--plugin").is_err()); // double hyphen
        assert!(validate_plugin_name("-plugin").is_err()); // leading hyphen
        assert!(validate_plugin_name("plugin-").is_err()); // trailing hyphen
    }

    #[test]
    fn test_parse_frontmatter() {
        use super::super::types::SkillFrontmatter;

        let content = r#"---
name: test
description: A test skill
---

This is the body."#;

        let (fm, body) =
            parse_frontmatter::<SkillFrontmatter>(content, Path::new("/test")).unwrap();

        assert_eq!(fm.name, Some("test".to_string()));
        assert_eq!(fm.description, Some("A test skill".to_string()));
        assert_eq!(body, "This is the body.");
    }

    #[test]
    fn test_parse_frontmatter_no_frontmatter() {
        use super::super::types::SkillFrontmatter;

        let content = "Just plain content.";
        let (fm, body) =
            parse_frontmatter::<SkillFrontmatter>(content, Path::new("/test")).unwrap();

        assert!(fm.name.is_none());
        assert_eq!(body, "Just plain content.");
    }

    #[test]
    fn test_parse_frontmatter_empty() {
        use super::super::types::SkillFrontmatter;

        let content = r#"---
---

Body content."#;

        let (fm, body) =
            parse_frontmatter::<SkillFrontmatter>(content, Path::new("/test")).unwrap();

        assert!(fm.name.is_none());
        assert_eq!(body, "Body content.");
    }
}
