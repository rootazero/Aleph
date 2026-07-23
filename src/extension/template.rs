//! Skill template processor
//!
//! Handles template syntax in skill content:
//! - `$ARGUMENTS` - replaced with provided arguments
//! - `@./path` - relative file reference (from skill directory)
//! - `@/path` - absolute file reference

use super::error::{ExtensionError, ExtensionResult};
use once_cell::sync::OnceCell;
use regex::Regex;
use std::path::{Path, PathBuf};

/// Regex for matching file references: @./path or @/path
/// Matches @./relative/path or @/absolute/path, stopping at whitespace or common delimiters
static FILE_REF_REGEX: OnceCell<Regex> = OnceCell::new();

/// Maximum bytes a single `@./file` reference expands to. Keeps a skill that
/// references a large file from inflating memory and the model context
/// window (mirrors the hook executor's `MAX_HOOK_OUTPUT_BYTES` cap).
const MAX_FILE_REF_BYTES: usize = 64 * 1024;

/// Maximum number of `@./file` references expanded per render.
const MAX_FILE_REFS: usize = 32;

/// Returns the compiled file-reference regex, initializing it on first use.
fn file_ref_regex() -> ExtensionResult<&'static Regex> {
    FILE_REF_REGEX.get_or_try_init(|| {
        // Pattern: @./path or @/path, stopping at whitespace or delimiters.
        // The regex is a compile-time constant; a parse failure is a programmer error.
        Regex::new(r#"@(\.?/[^\s\]\)>`"']+)"#)
            .map_err(|e| ExtensionError::template_error(format!("Invalid file reference regex: {e}")))
    })
}

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
    /// * `source_path` - Path to the skill file (used to derive `base_dir`)
    #[must_use]
    pub fn new(content: &str, source_path: &Path) -> Self {
        let base_dir = source_path
            .parent()
            .map_or_else(|| PathBuf::from("."), |p| p.to_path_buf());

        Self {
            content: content.to_string(),
            base_dir,
        }
    }

    /// Create from content and explicit base directory
    #[must_use]
    pub fn with_base_dir(content: &str, base_dir: PathBuf) -> Self {
        Self {
            content: content.to_string(),
            base_dir,
        }
    }

    /// Get the base directory
    #[must_use]
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
        for cap in file_ref_regex()?.captures_iter(content) {
            if replacements.len() >= MAX_FILE_REFS {
                return Err(ExtensionError::template_error(format!(
                    "too many file references (cap {MAX_FILE_REFS})"
                )));
            }
            let full_match = cap
                .get(0)
                .ok_or_else(|| ExtensionError::template_error("regex capture group 0 missing"))?;
            let path_str = cap
                .get(1)
                .ok_or_else(|| {
                    ExtensionError::template_error("regex capture group 1 missing for file refs")
                })?
                .as_str();

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

        // Apply replacements in reverse order to preserve positions.
        // Use positional replacement (single occurrence) to avoid corrupting
        // file contents that may contain the same reference pattern.
        for (start, end, _, replacement) in replacements.into_iter().rev() {
            result.replace_range(start..end, &replacement);
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

    /// Read a file's content, capped at [`MAX_FILE_REF_BYTES`] with a
    /// truncation marker appended when the file exceeds the cap.
    async fn read_file(&self, path: &Path) -> ExtensionResult<String> {
        use tokio::io::AsyncReadExt;

        // Read at most cap+1 bytes so an oversized file is detected without
        // being loaded whole into memory.
        let file = tokio::fs::File::open(path).await.map_err(|e| {
            ExtensionError::file_reference(path, format!("Failed to open file: {e}"))
        })?;
        let mut buf = Vec::new();
        file.take(MAX_FILE_REF_BYTES as u64 + 1)
            .read_to_end(&mut buf)
            .await
            .map_err(|e| {
                ExtensionError::file_reference(path, format!("Failed to read file: {e}"))
            })?;
        let truncated = buf.len() > MAX_FILE_REF_BYTES;
        if truncated {
            buf.truncate(MAX_FILE_REF_BYTES);
        }
        let mut content = String::from_utf8_lossy(&buf).into_owned();
        if truncated {
            content.push_str("\n...[truncated: file exceeds 64 KiB cap]");
        }
        Ok(content)
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
        let template = SkillTemplate::new("$ARGUMENTS says $ARGUMENTS", Path::new("/test/skill"));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(template.render("Hello")).unwrap();
        assert_eq!(result, "Hello says Hello");
    }

    #[tokio::test]
    async fn test_file_reference_relative() {
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join("config.json");
        tokio::fs::write(&config_path, r#"{"key": "value"}"#)
            .await
            .unwrap();

        let template =
            SkillTemplate::with_base_dir("Config: @./config.json", temp.path().to_path_buf());

        let result = template.render("").await.unwrap();
        assert_eq!(result, r#"Config: {"key": "value"}"#);
    }

    #[tokio::test]
    #[cfg(unix)] // POSIX-only: @/absolute file-ref syntax (a Windows C:\ path isn't matched)
    async fn test_file_reference_absolute_blocked() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("test.txt");
        tokio::fs::write(&file_path, "Test content").await.unwrap();

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
        let matches: Vec<_> = file_ref_regex().unwrap().find_iter(content).collect();
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].as_str(), "@./config.json");
        assert_eq!(matches[1].as_str(), "@/etc/hosts");
    }

    #[tokio::test]
    async fn test_combined_template() {
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join("settings.json");
        tokio::fs::write(&config_path, r#"{"name": "test"}"#)
            .await
            .unwrap();

        let template = SkillTemplate::with_base_dir(
            "User: $ARGUMENTS\nSettings: @./settings.json",
            temp.path().to_path_buf(),
        );

        let result = template.render("Alice").await.unwrap();
        assert_eq!(result, "User: Alice\nSettings: {\"name\": \"test\"}");
    }
}
