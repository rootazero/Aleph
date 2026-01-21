//! Plugin manifest parsing
//!
//! Parses Claude Code compatible plugin.json files.

use std::path::Path;

use crate::plugins::error::{PluginError, PluginResult};
use crate::plugins::types::PluginManifest;

/// Parse a plugin manifest from a directory
///
/// Looks for `.claude-plugin/plugin.json` in the given directory.
pub fn parse_manifest(plugin_dir: &Path) -> PluginResult<PluginManifest> {
    let manifest_path = plugin_dir.join(".claude-plugin").join("plugin.json");

    if !manifest_path.exists() {
        return Err(PluginError::InvalidStructure {
            path: plugin_dir.to_path_buf(),
            reason: "Missing .claude-plugin/plugin.json".to_string(),
        });
    }

    let content = std::fs::read_to_string(&manifest_path)?;

    let manifest: PluginManifest =
        serde_json::from_str(&content).map_err(|e| PluginError::ManifestParseError {
            path: manifest_path.clone(),
            source: e,
        })?;

    // Validate required fields
    if manifest.name.is_empty() {
        return Err(PluginError::MissingRequiredField {
            path: manifest_path,
            field: "name".to_string(),
        });
    }

    Ok(manifest)
}

/// Validate a plugin manifest
pub fn validate_manifest(manifest: &PluginManifest) -> PluginResult<()> {
    // Name must be non-empty
    if manifest.name.is_empty() {
        return Err(PluginError::MissingRequiredField {
            path: std::path::PathBuf::new(),
            field: "name".to_string(),
        });
    }

    // Name should be valid identifier (kebab-case)
    if !is_valid_plugin_name(&manifest.name) {
        return Err(PluginError::InvalidStructure {
            path: std::path::PathBuf::new(),
            reason: format!(
                "Invalid plugin name '{}'. Use kebab-case (e.g., 'my-plugin')",
                manifest.name
            ),
        });
    }

    // Version should be semver if provided
    if let Some(ref version) = manifest.version {
        if !is_valid_semver(version) {
            tracing::warn!(
                "Plugin '{}' has non-semver version '{}'. Consider using semver format.",
                manifest.name,
                version
            );
        }
    }

    Ok(())
}

/// Check if a plugin name is valid (kebab-case identifier)
fn is_valid_plugin_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }

    // Must start with letter
    let first = name.chars().next().unwrap();
    if !first.is_ascii_lowercase() {
        return false;
    }

    // Must contain only lowercase letters, numbers, and hyphens
    name.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Check if a version string is valid semver
fn is_valid_semver(version: &str) -> bool {
    // Basic semver check: X.Y.Z with optional prerelease/build
    let parts: Vec<&str> = version.split('-').next().unwrap().split('.').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return false;
    }
    parts.iter().all(|p| p.parse::<u32>().is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_plugin_names() {
        assert!(is_valid_plugin_name("my-plugin"));
        assert!(is_valid_plugin_name("plugin123"));
        assert!(is_valid_plugin_name("a"));
        assert!(is_valid_plugin_name("my-cool-plugin-2"));
    }

    #[test]
    fn test_invalid_plugin_names() {
        assert!(!is_valid_plugin_name("")); // empty
        assert!(!is_valid_plugin_name("My-Plugin")); // uppercase
        assert!(!is_valid_plugin_name("123plugin")); // starts with number
        assert!(!is_valid_plugin_name("my_plugin")); // underscore
        assert!(!is_valid_plugin_name("my plugin")); // space
    }

    #[test]
    fn test_valid_semver() {
        assert!(is_valid_semver("1.0.0"));
        assert!(is_valid_semver("0.1.0"));
        assert!(is_valid_semver("10.20.30"));
        assert!(is_valid_semver("1.0.0-alpha"));
        assert!(is_valid_semver("1.0")); // Allow 2 parts
    }

    #[test]
    fn test_invalid_semver() {
        assert!(!is_valid_semver("1")); // Only 1 part
        assert!(!is_valid_semver("v1.0.0")); // prefix
        assert!(!is_valid_semver("1.0.0.0")); // 4 parts
    }

    #[test]
    fn test_parse_minimal_manifest() {
        let json = r#"{"name": "test-plugin"}"#;
        let manifest: PluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.name, "test-plugin");
        assert!(manifest.version.is_none());
        assert!(manifest.description.is_none());
    }

    #[test]
    fn test_parse_full_manifest() {
        let json = r#"{
            "name": "test-plugin",
            "version": "1.0.0",
            "description": "A test plugin",
            "author": {
                "name": "Test Author",
                "email": "test@example.com"
            },
            "license": "MIT"
        }"#;
        let manifest: PluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.name, "test-plugin");
        assert_eq!(manifest.version, Some("1.0.0".to_string()));
        assert_eq!(manifest.description, Some("A test plugin".to_string()));
        assert!(manifest.author.is_some());
        assert_eq!(manifest.author.unwrap().name, "Test Author");
    }
}
