//! Unified config loader supporting TOML and JSONC.
//!
//! This module provides utilities for loading extension configuration files
//! in either TOML or JSONC format, with TOML being the preferred format.
//! JSONC (JSON with Comments) support is maintained for backward compatibility.

use std::path::{Path, PathBuf};

use super::types::AlephConfig;
use crate::extension::ExtensionError;

/// Config file priority order (TOML preferred over JSONC).
const CONFIG_FILES: &[&str] = &["aether.toml", "aleph.jsonc", "aleph.json"];

/// Find the config file in a directory.
///
/// Returns the path to the first existing config file found,
/// in priority order: `aether.toml` > `aleph.jsonc` > `aleph.json`.
///
/// # Arguments
///
/// * `dir` - Directory to search for config files
///
/// # Returns
///
/// The path to the config file if found, or None if no config file exists.
pub fn find_config_file(dir: &Path) -> Option<PathBuf> {
    for filename in CONFIG_FILES {
        let path = dir.join(filename);
        if path.exists() {
            return Some(path);
        }
    }
    None
}

/// Load extension config from a directory.
///
/// Searches for a config file in the given directory using the priority order
/// (TOML preferred) and loads it if found.
///
/// # Arguments
///
/// * `dir` - Directory to search for config files
///
/// # Returns
///
/// * `Ok(Some(config))` - Config loaded successfully
/// * `Ok(None)` - No config file found in directory
/// * `Err(...)` - Error parsing config file
pub fn load_extension_config(dir: &Path) -> Result<Option<AlephConfig>, ExtensionError> {
    let Some(path) = find_config_file(dir) else {
        return Ok(None);
    };
    load_config_file(&path).map(Some)
}

/// Load config from a specific file.
///
/// Automatically detects the format based on file extension and parses
/// the configuration appropriately.
///
/// # Arguments
///
/// * `path` - Path to the config file
///
/// # Supported formats
///
/// * `.toml` - TOML format
/// * `.jsonc` - JSON with comments
/// * `.json` - Standard JSON
pub fn load_config_file(path: &Path) -> Result<AlephConfig, ExtensionError> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        ExtensionError::config_parse(path, format!("Failed to read file: {}", e))
    })?;

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    match ext {
        "toml" => parse_toml(&content, path),
        "jsonc" | "json" => parse_jsonc(&content, path),
        _ => Err(ExtensionError::config_parse(
            path,
            format!("Unknown config file extension: .{}", ext),
        )),
    }
}

/// Parse TOML content into AlephConfig.
fn parse_toml(content: &str, path: &Path) -> Result<AlephConfig, ExtensionError> {
    toml::from_str(content).map_err(|e| {
        ExtensionError::config_parse(path, format!("TOML parse error: {}", e))
    })
}

/// Parse JSONC (JSON with comments) content into AlephConfig.
fn parse_jsonc(content: &str, path: &Path) -> Result<AlephConfig, ExtensionError> {
    let stripped = strip_json_comments(content);
    // Handle trailing commas (common in JSONC)
    let trailing_comma_re = regex::Regex::new(r",(\s*[\]}])").unwrap();
    let cleaned = trailing_comma_re.replace_all(&stripped, "$1").to_string();

    serde_json::from_str(&cleaned).map_err(|e| {
        ExtensionError::config_parse(path, format!("JSONC parse error: {}", e))
    })
}

/// Strip single-line and multi-line comments from JSON.
///
/// This function handles:
/// - Single-line comments (`// ...`)
/// - Multi-line comments (`/* ... */`)
/// - Preserves strings that contain comment-like patterns
fn strip_json_comments(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escape_next = false;

    while let Some(c) = chars.next() {
        // Handle escape sequences in strings
        if escape_next {
            result.push(c);
            escape_next = false;
            continue;
        }

        // Check for escape character
        if c == '\\' && in_string {
            result.push(c);
            escape_next = true;
            continue;
        }

        // Track string state
        if c == '"' {
            in_string = !in_string;
            result.push(c);
            continue;
        }

        // If we're inside a string, pass through
        if in_string {
            result.push(c);
            continue;
        }

        // Check for comments
        if c == '/' {
            match chars.peek() {
                Some('/') => {
                    // Single-line comment: skip until newline
                    chars.next(); // consume second '/'
                    while let Some(&nc) = chars.peek() {
                        if nc == '\n' {
                            break;
                        }
                        chars.next();
                    }
                }
                Some('*') => {
                    // Multi-line comment: skip until */
                    chars.next(); // consume '*'
                    while let Some(nc) = chars.next() {
                        if nc == '*' && chars.peek() == Some(&'/') {
                            chars.next(); // consume '/'
                            break;
                        }
                    }
                }
                _ => result.push(c),
            }
        } else {
            result.push(c);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_single_line_comments() {
        let input = r#"{"key": "value" // comment
        }"#;
        let stripped = strip_json_comments(input);
        assert!(!stripped.contains("//"));
        assert!(stripped.contains("\"key\""));
        assert!(stripped.contains("\"value\""));
    }

    #[test]
    fn test_strip_multiline_comments() {
        let input = r#"{"key": /* comment */ "value"}"#;
        let stripped = strip_json_comments(input);
        assert!(!stripped.contains("/*"));
        assert!(!stripped.contains("*/"));
        assert!(stripped.contains("\"value\""));
    }

    #[test]
    fn test_preserve_string_with_slashes() {
        let input = r#"{"url": "https://example.com/path"}"#;
        let stripped = strip_json_comments(input);
        assert_eq!(stripped, input);
    }

    #[test]
    fn test_preserve_string_with_comment_pattern() {
        let input = r#"{"pattern": "// this is not a comment"}"#;
        let stripped = strip_json_comments(input);
        assert!(stripped.contains("// this is not a comment"));
    }

    #[test]
    fn test_find_config_file_toml_preferred() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path();

        // Create both files
        std::fs::write(dir.join("aether.toml"), "").unwrap();
        std::fs::write(dir.join("aleph.jsonc"), "").unwrap();

        let found = find_config_file(dir);
        assert!(found.is_some());
        assert!(found.unwrap().ends_with("aether.toml"));
    }

    #[test]
    fn test_find_config_file_jsonc_fallback() {
        let temp_dir = tempfile::tempdir().unwrap();
        let dir = temp_dir.path();

        // Create only jsonc
        std::fs::write(dir.join("aleph.jsonc"), "").unwrap();

        let found = find_config_file(dir);
        assert!(found.is_some());
        assert!(found.unwrap().ends_with("aleph.jsonc"));
    }

    #[test]
    fn test_find_config_file_none() {
        let temp_dir = tempfile::tempdir().unwrap();
        let found = find_config_file(temp_dir.path());
        assert!(found.is_none());
    }

    #[test]
    fn test_load_toml_config() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("aether.toml");

        let toml_content = r#"
model = "anthropic/claude-4"
plugin = ["my-plugin"]
"#;
        std::fs::write(&path, toml_content).unwrap();

        let config = load_config_file(&path).unwrap();
        assert_eq!(config.model, Some("anthropic/claude-4".to_string()));
        assert_eq!(config.plugin, Some(vec!["my-plugin".to_string()]));
    }

    #[test]
    fn test_load_jsonc_config() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("aleph.jsonc");

        let jsonc_content = r#"{
  // This is a comment
  "model": "anthropic/claude-4",
  "plugin": ["my-plugin"]
}"#;
        std::fs::write(&path, jsonc_content).unwrap();

        let config = load_config_file(&path).unwrap();
        assert_eq!(config.model, Some("anthropic/claude-4".to_string()));
        assert_eq!(config.plugin, Some(vec!["my-plugin".to_string()]));
    }

    #[test]
    fn test_load_jsonc_with_trailing_comma() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("aleph.jsonc");

        let jsonc_content = r#"{
  "plugin": [
    "plugin-a",
    "plugin-b",
  ],
}"#;
        std::fs::write(&path, jsonc_content).unwrap();

        let config = load_config_file(&path).unwrap();
        let plugins = config.plugin.unwrap();
        assert_eq!(plugins.len(), 2);
    }
}
