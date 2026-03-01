//! Skill template processor
//!
//! Handles template syntax in skill content:
//! - `$ARGUMENTS` - replaced with provided arguments
//! - `@./path` - relative file reference (from skill directory)
//! - `@/path` - absolute file reference

use super::error::{ExtensionError, ExtensionResult};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Regex for matching file references: @./path or @/path
/// Matches @./relative/path or @/absolute/path, stopping at whitespace or common delimiters
static FILE_REF_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    // Pattern: @./path or @/path, stopping at whitespace or delimiters
    Regex::new(r#"@(\.?/[^\s\]\)>`"']+)"#).expect("Invalid file reference regex")
});

/// Skill template processor
#[derive(Debug, Clone)]
pub struct SkillTemplate {
    /// Raw template content
    content: String,
    /// Base directory for relative paths
    base_dir: PathBuf,
}

impl SkillTemplate {
    /// Create a new template processor
    ///
    /// # Arguments
    /// * `content` - Raw skill content with template syntax
    /// * `source_path` - Path to the skill file (used to derive base_dir)
    pub fn new(content: &str, source_path: &Path) -> Self {
        let base_dir = source_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));

        Self {
            content: content.to_string(),
            base_dir,
        }
    }

    /// Create from content and explicit base directory
    pub fn with_base_dir(content: &str, base_dir: PathBuf) -> Self {
        Self {
            content: content.to_string(),
            base_dir,
        }
    }

    /// Get the base directory
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Render the template with the given arguments
    ///
    /// Performs all template substitutions:
    /// 1. `$ARGUMENTS` replacement
    /// 2. `@file` reference expansion
    pub async fn render(&self, arguments: &str) -> ExtensionResult<String> {
        // 1. Replace $ARGUMENTS
        let mut result = self.content.replace("$ARGUMENTS", arguments);

        // 2. Expand file references
        result = self.expand_file_refs(&result).await?;

        Ok(result)
    }

    /// Expand all file references in the content
    async fn expand_file_refs(&self, content: &str) -> ExtensionResult<String> {
        let mut result = content.to_string();
        let mut replacements = Vec::new();

        // Find all file references
        for cap in FILE_REF_REGEX.captures_iter(content) {
            let full_match = cap.get(0).unwrap();
            let path_str = cap.get(1).unwrap().as_str();

            // Resolve the path
            let resolved_path = self.resolve_path(path_str)?;

            // Read file content
            let file_content = self.read_file(&resolved_path).await?;

            replacements.push((
                full_match.start(),
                full_match.end(),
                full_match.as_str().to_string(),
                file_content,
            ));
        }

        // Apply replacements in reverse order to preserve positions
        for (_, _, original, replacement) in replacements.into_iter().rev() {
            result = result.replace(&original, &replacement);
        }

        Ok(result)
    }

    /// Resolve a file path from the template syntax
    fn resolve_path(&self, path_str: &str) -> ExtensionResult<PathBuf> {
        let path = if let Some(relative) = path_str.strip_prefix("./") {
            // Relative path from base_dir
            let resolved = self.base_dir.join(relative);

            // Security check: ensure the resolved path is within base_dir
            self.validate_path_security(&resolved)?;

            resolved
        } else if path_str.starts_with('/') {
            // Absolute paths are not allowed — they bypass base_dir containment
            return Err(ExtensionError::file_reference(
                path_str,
                "Absolute paths are not allowed in file references; use relative paths (./path) instead",
            ));
        } else {
            // Treat as relative
            let resolved = self.base_dir.join(path_str);
            self.validate_path_security(&resolved)?;
            resolved
        };

        Ok(path)
    }

    /// Validate that a path doesn't escape the base directory (for relative paths)
    fn validate_path_security(&self, resolved: &Path) -> ExtensionResult<()> {
        // Check for obvious traversal patterns
        let path_str = resolved.to_string_lossy();
        if path_str.contains("..") {
            return Err(ExtensionError::file_reference(
                resolved,
                "Path traversal (..) not allowed in relative file references",
            ));
        }

        // If the file exists, canonicalize and verify containment within base_dir
        if resolved.exists() {
            if let (Ok(canonical_path), Ok(canonical_base)) =
                (resolved.canonicalize(), self.base_dir.canonicalize())
            {
                if !canonical_path.starts_with(&canonical_base) {
                    return Err(ExtensionError::file_reference(
                        resolved,
                        "Resolved path escapes the base directory",
                    ));
                }
            }
        }

        Ok(())
    }

    /// Read a file's content
    async fn read_file(&self, path: &Path) -> ExtensionResult<String> {
        tokio::fs::read_to_string(path).await.map_err(|e| {
            ExtensionError::file_reference(path, format!("Failed to read file: {}", e))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_arguments_substitution() {
        let template = SkillTemplate::new("Hello $ARGUMENTS!", Path::new("/test/skill"));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(template.render("World")).unwrap();
        assert_eq!(result, "Hello World!");
    }

    #[test]
    fn test_multiple_arguments() {
        let template =
            SkillTemplate::new("$ARGUMENTS says $ARGUMENTS", Path::new("/test/skill"));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(template.render("Hello")).unwrap();
        assert_eq!(result, "Hello says Hello");
    }

    #[tokio::test]
    async fn test_file_reference_relative() {
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join("config.json");
        std::fs::write(&config_path, r#"{"key": "value"}"#).unwrap();

        let template = SkillTemplate::with_base_dir(
            "Config: @./config.json",
            temp.path().to_path_buf(),
        );

        let result = template.render("").await.unwrap();
        assert_eq!(result, r#"Config: {"key": "value"}"#);
    }

    #[tokio::test]
    async fn test_file_reference_absolute_blocked() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("test.txt");
        std::fs::write(&file_path, "Test content").unwrap();

        let template = SkillTemplate::with_base_dir(
            &format!("Content: @{}", file_path.display()),
            PathBuf::from("/other"),
        );

        // Absolute paths must be rejected
        let result = template.render("").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ExtensionError::FileReference { .. }));
    }

    #[tokio::test]
    async fn test_path_traversal_blocked() {
        let template = SkillTemplate::with_base_dir(
            "Content: @./../../../etc/passwd",
            PathBuf::from("/test/skill"),
        );

        let result = template.render("").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ExtensionError::FileReference { .. }));
    }

    #[tokio::test]
    async fn test_file_not_found() {
        let template = SkillTemplate::with_base_dir(
            "Content: @./nonexistent.txt",
            PathBuf::from("/test/skill"),
        );

        let result = template.render("").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_file_ref_regex() {
        let content = "See @./config.json and @/etc/hosts for details.";
        let matches: Vec<_> = FILE_REF_REGEX.find_iter(content).collect();
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].as_str(), "@./config.json");
        assert_eq!(matches[1].as_str(), "@/etc/hosts");
    }

    #[tokio::test]
    async fn test_combined_template() {
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join("settings.json");
        std::fs::write(&config_path, r#"{"name": "test"}"#).unwrap();

        let template = SkillTemplate::with_base_dir(
            "User: $ARGUMENTS\nSettings: @./settings.json",
            temp.path().to_path_buf(),
        );

        let result = template.render("Alice").await.unwrap();
        assert_eq!(result, "User: Alice\nSettings: {\"name\": \"test\"}");
    }
}
