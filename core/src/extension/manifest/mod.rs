//! Plugin manifest parsing and validation
//!
//! This module provides unified manifest parsing for Aether plugins from multiple formats:
//!
//! 1. **package.json** (Node.js plugins) - Standard npm package with "aether" field
//! 2. **aether.plugin.json** (WASM/Static plugins) - Native Aether plugin manifest
//! 3. **.claude-plugin/plugin.json** (Legacy) - Claude Code plugin format
//!
//! # Auto-Detection
//!
//! The `parse_manifest_from_dir()` function automatically detects the manifest format:
//! - If `aether.plugin.json` exists, parse it as an Aether native manifest
//! - Otherwise, if `package.json` exists with "aether" field, parse it as Node.js plugin
//! - Otherwise, if `.claude-plugin/plugin.json` exists, parse it as legacy Claude plugin
//!
//! # Example
//!
//! ```rust,ignore
//! use aethecore::extension::manifest::parse_manifest_from_dir;
//! use std::path::Path;
//!
//! let manifest = parse_manifest_from_dir(Path::new("/path/to/plugin")).await?;
//! println!("Plugin: {} v{:?}", manifest.id, manifest.version);
//! ```

mod aether_plugin;
mod package_json;
mod types;

pub use aether_plugin::{
    parse_aether_plugin, parse_aether_plugin_content, sanitize_plugin_id, validate_plugin_id,
};
pub use package_json::{parse_package_json, parse_package_json_content};
pub use types::{AuthorInfo, ConfigUiHint, PluginManifest, PluginPermission};

// Re-export legacy types for backward compatibility with loader
pub use self::legacy::{
    parse_plugin_manifest, LegacyPluginManifest, PluginAuthor, PluginRepository,
};

use crate::extension::error::{ExtensionError, ExtensionResult};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// =============================================================================
// Legacy Plugin Manifest Types (for backward compatibility with loader)
// =============================================================================

/// Module containing legacy plugin manifest types for .claude-plugin/plugin.json format
pub mod legacy {
    use super::*;

    /// Legacy plugin manifest (parsed from .claude-plugin/plugin.json)
    ///
    /// This type is kept for backward compatibility with the existing plugin loader.
    /// For new plugins, use `PluginManifest` from `types.rs` instead.
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct LegacyPluginManifest {
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

    /// Parse legacy plugin manifest from file
    pub async fn parse_plugin_manifest(path: &Path) -> ExtensionResult<LegacyPluginManifest> {
        let content = tokio::fs::read_to_string(path).await?;

        let manifest: LegacyPluginManifest = serde_json::from_str(&content).map_err(|e| {
            ExtensionError::invalid_manifest(path, format!("JSON parse error: {}", e))
        })?;

        // Validate required fields
        if manifest.name.is_empty() {
            return Err(ExtensionError::missing_field(path, "name"));
        }

        Ok(manifest)
    }
}

// Type alias for backward compatibility
pub use legacy::LegacyPluginManifest as PluginManifestLegacy;

// =============================================================================
// Manifest File Names
// =============================================================================

/// Aether native manifest filename
pub const AETHER_PLUGIN_MANIFEST: &str = "aether.plugin.json";

/// npm package manifest filename
pub const PACKAGE_JSON: &str = "package.json";

/// Claude plugin manifest path (legacy compatibility)
pub const CLAUDE_PLUGIN_MANIFEST: &str = ".claude-plugin/plugin.json";

// =============================================================================
// Auto-Detection Parser
// =============================================================================

/// Parse a plugin manifest from a directory
///
/// This function auto-detects the manifest format by checking for:
/// 1. `aether.plugin.json` - Native Aether manifest (preferred)
/// 2. `package.json` with "aether" field - Node.js plugin
/// 3. `.claude-plugin/plugin.json` - Legacy Claude plugin format
///
/// The returned manifest will have `root_dir` set to the directory path.
///
/// # Arguments
/// * `dir` - Path to the plugin directory
///
/// # Returns
/// * `Ok(PluginManifest)` - Parsed manifest with root_dir set
/// * `Err(ExtensionError)` - If no valid manifest found or parsing fails
///
/// # Example
///
/// ```rust,ignore
/// let manifest = parse_manifest_from_dir(Path::new("/plugins/my-plugin")).await?;
/// assert_eq!(manifest.root_dir, PathBuf::from("/plugins/my-plugin"));
/// ```
pub async fn parse_manifest_from_dir(dir: &Path) -> ExtensionResult<PluginManifest> {
    // 1. Check for aether.plugin.json (preferred)
    let aether_manifest_path = dir.join(AETHER_PLUGIN_MANIFEST);
    if aether_manifest_path.exists() {
        let mut manifest = parse_aether_plugin(&aether_manifest_path).await?;
        manifest.root_dir = dir.to_path_buf();
        return Ok(manifest);
    }

    // 2. Check for package.json with aether field
    let package_json_path = dir.join(PACKAGE_JSON);
    if package_json_path.exists() {
        // Try to parse - will fail if no "aether" field
        match parse_package_json(&package_json_path).await {
            Ok(mut manifest) => {
                manifest.root_dir = dir.to_path_buf();
                return Ok(manifest);
            }
            Err(ExtensionError::InvalidManifest { message, .. })
                if message.contains("Missing 'aether' field") =>
            {
                // package.json exists but is not an Aether plugin - continue checking
            }
            Err(e) => return Err(e),
        }
    }

    // 3. Check for legacy .claude-plugin/plugin.json
    let claude_manifest_path = dir.join(CLAUDE_PLUGIN_MANIFEST);
    if claude_manifest_path.exists() {
        let manifest = parse_legacy_claude_manifest(&claude_manifest_path).await?;
        return Ok(manifest.with_root_dir(dir.to_path_buf()));
    }

    // No valid manifest found
    Err(ExtensionError::invalid_manifest(
        dir,
        format!(
            "No plugin manifest found. Expected {} or {} with 'aether' field",
            AETHER_PLUGIN_MANIFEST, PACKAGE_JSON
        ),
    ))
}

/// Synchronous version of parse_manifest_from_dir
///
/// Useful for tests and non-async contexts.
pub fn parse_manifest_from_dir_sync(dir: &Path) -> ExtensionResult<PluginManifest> {
    // 1. Check for aether.plugin.json
    let aether_manifest_path = dir.join(AETHER_PLUGIN_MANIFEST);
    if aether_manifest_path.exists() {
        let content = std::fs::read_to_string(&aether_manifest_path)?;
        let mut manifest = parse_aether_plugin_content(&content, &aether_manifest_path)?;
        manifest.root_dir = dir.to_path_buf();
        return Ok(manifest);
    }

    // 2. Check for package.json
    let package_json_path = dir.join(PACKAGE_JSON);
    if package_json_path.exists() {
        let content = std::fs::read_to_string(&package_json_path)?;
        match parse_package_json_content(&content, &package_json_path) {
            Ok(mut manifest) => {
                manifest.root_dir = dir.to_path_buf();
                return Ok(manifest);
            }
            Err(ExtensionError::InvalidManifest { message, .. })
                if message.contains("Missing 'aether' field") =>
            {
                // Continue checking
            }
            Err(e) => return Err(e),
        }
    }

    // 3. Check for legacy .claude-plugin/plugin.json
    let claude_manifest_path = dir.join(CLAUDE_PLUGIN_MANIFEST);
    if claude_manifest_path.exists() {
        let content = std::fs::read_to_string(&claude_manifest_path)?;
        let manifest = parse_legacy_claude_manifest_content(&content, &claude_manifest_path)?;
        return Ok(manifest.with_root_dir(dir.to_path_buf()));
    }

    Err(ExtensionError::invalid_manifest(
        dir,
        format!(
            "No plugin manifest found. Expected {} or {} with 'aether' field",
            AETHER_PLUGIN_MANIFEST, PACKAGE_JSON
        ),
    ))
}

// =============================================================================
// Legacy Claude Plugin Format
// =============================================================================

/// Legacy Claude plugin manifest structure
#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)]
struct LegacyClaudeManifest {
    name: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

/// Parse legacy .claude-plugin/plugin.json format
async fn parse_legacy_claude_manifest(path: &Path) -> ExtensionResult<PluginManifest> {
    let content = tokio::fs::read_to_string(path).await?;
    parse_legacy_claude_manifest_content(&content, path)
}

/// Parse legacy Claude manifest content
fn parse_legacy_claude_manifest_content(
    content: &str,
    path: &Path,
) -> ExtensionResult<PluginManifest> {
    let legacy: LegacyClaudeManifest = serde_json::from_str(content)
        .map_err(|e| ExtensionError::invalid_manifest(path, format!("JSON parse error: {}", e)))?;

    // Convert legacy format to PluginManifest
    // Legacy plugins are treated as static content plugins
    let id = sanitize_plugin_id(&legacy.name);

    validate_plugin_id(&id).map_err(|reason| ExtensionError::invalid_plugin_name(&id, reason))?;

    Ok(PluginManifest::new(
        id,
        legacy.name,
        crate::extension::types::PluginKind::Static,
        ".".into(),
    ))
}

// =============================================================================
// Re-export frontmatter parsing
// =============================================================================

/// Parse YAML frontmatter from markdown content
///
/// This is re-exported from the original manifest module for backward compatibility.
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

/// Validate plugin name format (alias for validate_plugin_id)
///
/// Plugin names must be:
/// - Lowercase
/// - Start with a letter
/// - Only contain letters, numbers, and hyphens
pub fn validate_plugin_name(name: &str) -> ExtensionResult<()> {
    validate_plugin_id(name).map_err(|reason| ExtensionError::invalid_plugin_name(name, reason))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::types::SkillFrontmatter;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn test_validate_plugin_name() {
        assert!(validate_plugin_name("my-plugin").is_ok());
        assert!(validate_plugin_name("plugin123").is_ok());
        assert!(validate_plugin_name("a").is_ok());
        assert!(validate_plugin_name("").is_err());
        assert!(validate_plugin_name("My-Plugin").is_err());
        assert!(validate_plugin_name("123plugin").is_err());
    }

    #[test]
    fn test_parse_frontmatter() {
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
        let content = "Just plain content.";
        let (fm, body) =
            parse_frontmatter::<SkillFrontmatter>(content, Path::new("/test")).unwrap();

        assert!(fm.name.is_none());
        assert_eq!(body, "Just plain content.");
    }

    #[test]
    fn test_parse_frontmatter_empty() {
        let content = r#"---
---

Body content."#;

        let (fm, body) =
            parse_frontmatter::<SkillFrontmatter>(content, Path::new("/test")).unwrap();

        assert!(fm.name.is_none());
        assert_eq!(body, "Body content.");
    }

    #[test]
    fn test_sanitize_plugin_id() {
        // Tests the consolidated sanitize_plugin_id from aether_plugin.rs
        assert_eq!(sanitize_plugin_id("My Plugin"), "my-plugin");
        assert_eq!(sanitize_plugin_id("my_plugin"), "my-plugin");
        assert_eq!(sanitize_plugin_id("my--plugin"), "my-plugin");
        assert_eq!(sanitize_plugin_id("-my-plugin-"), "my-plugin");
        assert_eq!(sanitize_plugin_id("Plugin 123"), "plugin-123");
    }

    #[test]
    fn test_parse_manifest_from_dir_aether_plugin() {
        let temp_dir = TempDir::new().unwrap();
        let manifest_path = temp_dir.path().join("aether.plugin.json");

        std::fs::write(
            &manifest_path,
            r#"{
                "id": "test-plugin",
                "name": "Test Plugin",
                "version": "1.0.0"
            }"#,
        )
        .unwrap();

        let manifest = parse_manifest_from_dir_sync(temp_dir.path()).unwrap();

        assert_eq!(manifest.id, "test-plugin");
        assert_eq!(manifest.name, "Test Plugin");
        assert_eq!(manifest.version, Some("1.0.0".to_string()));
        assert_eq!(manifest.root_dir, temp_dir.path());
    }

    #[test]
    fn test_parse_manifest_from_dir_package_json() {
        let temp_dir = TempDir::new().unwrap();
        let manifest_path = temp_dir.path().join("package.json");

        std::fs::write(
            &manifest_path,
            r#"{
                "name": "npm-plugin",
                "version": "2.0.0",
                "main": "dist/index.js",
                "aether": {
                    "id": "npm-plugin"
                }
            }"#,
        )
        .unwrap();

        let manifest = parse_manifest_from_dir_sync(temp_dir.path()).unwrap();

        assert_eq!(manifest.id, "npm-plugin");
        assert_eq!(manifest.version, Some("2.0.0".to_string()));
        assert_eq!(manifest.entry, PathBuf::from("dist/index.js"));
        assert_eq!(manifest.root_dir, temp_dir.path());
    }

    #[test]
    fn test_parse_manifest_from_dir_prefers_aether_plugin() {
        let temp_dir = TempDir::new().unwrap();

        // Create both manifests
        std::fs::write(
            temp_dir.path().join("aether.plugin.json"),
            r#"{"id": "aether-version"}"#,
        )
        .unwrap();

        std::fs::write(
            temp_dir.path().join("package.json"),
            r#"{
                "name": "npm-version",
                "aether": {}
            }"#,
        )
        .unwrap();

        let manifest = parse_manifest_from_dir_sync(temp_dir.path()).unwrap();

        // Should prefer aether.plugin.json
        assert_eq!(manifest.id, "aether-version");
    }

    #[test]
    fn test_parse_manifest_from_dir_legacy_claude() {
        let temp_dir = TempDir::new().unwrap();
        let claude_dir = temp_dir.path().join(".claude-plugin");
        std::fs::create_dir(&claude_dir).unwrap();

        std::fs::write(
            claude_dir.join("plugin.json"),
            r#"{
                "name": "Legacy Plugin",
                "version": "0.1.0"
            }"#,
        )
        .unwrap();

        let manifest = parse_manifest_from_dir_sync(temp_dir.path()).unwrap();

        assert_eq!(manifest.id, "legacy-plugin");
        assert_eq!(manifest.name, "Legacy Plugin");
        assert_eq!(manifest.root_dir, temp_dir.path());
    }

    #[test]
    fn test_parse_manifest_from_dir_no_manifest() {
        let temp_dir = TempDir::new().unwrap();

        let result = parse_manifest_from_dir_sync(temp_dir.path());
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(err, ExtensionError::InvalidManifest { .. }));
    }

    #[test]
    fn test_parse_manifest_from_dir_package_json_without_aether() {
        let temp_dir = TempDir::new().unwrap();

        std::fs::write(
            temp_dir.path().join("package.json"),
            r#"{
                "name": "regular-npm-package",
                "version": "1.0.0"
            }"#,
        )
        .unwrap();

        // Should fail because package.json exists but has no "aether" field
        let result = parse_manifest_from_dir_sync(temp_dir.path());
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_parse_manifest_from_dir_async() {
        let temp_dir = TempDir::new().unwrap();

        std::fs::write(
            temp_dir.path().join("aether.plugin.json"),
            r#"{"id": "async-test"}"#,
        )
        .unwrap();

        let manifest = parse_manifest_from_dir(temp_dir.path()).await.unwrap();
        assert_eq!(manifest.id, "async-test");
    }
}
