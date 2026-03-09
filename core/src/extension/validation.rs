//! Plugin validation — checks manifest, entry point, and registration uniqueness.
//!
//! Used by `aleph plugin validate` CLI command and internal pre-load checks.

use std::collections::HashSet;
use std::path::Path;

/// Result of validating a plugin directory.
#[derive(Debug, Default)]
pub struct ValidationResult {
    /// Critical issues that prevent the plugin from loading.
    pub errors: Vec<String>,
    /// Non-critical issues or suggestions.
    pub warnings: Vec<String>,
    /// Informational messages.
    pub info: Vec<String>,
}

impl ValidationResult {
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Validate a plugin directory for correctness.
///
/// Checks:
/// 1. Manifest exists and parses correctly
/// 2. Entry file exists (warning if missing — may need build)
/// 3. No duplicate tool names
/// 4. No duplicate hook handlers for same event
/// 5. Plugin ID is non-empty and reasonable
/// 6. Version is valid semver (if present)
pub fn validate_plugin(plugin_dir: &Path) -> ValidationResult {
    let mut result = ValidationResult::default();

    // 1. Check directory exists
    if !plugin_dir.exists() {
        result
            .errors
            .push(format!("Directory does not exist: {}", plugin_dir.display()));
        return result;
    }

    // 2. Parse manifest (sync version — no runtime needed)
    let manifest = match super::manifest::parse_manifest_from_dir_sync(plugin_dir) {
        Ok(m) => m,
        Err(e) => {
            result
                .errors
                .push(format!("Failed to parse manifest: {}", e));
            return result;
        }
    };

    result
        .info
        .push(format!("Plugin: {} ({})", manifest.name, manifest.id));
    result.info.push(format!("Kind: {:?}", manifest.kind));

    // 3. Check plugin ID
    if manifest.id.is_empty() {
        result.errors.push("Plugin ID is empty".to_string());
    } else if manifest.id.contains(' ') {
        result
            .warnings
            .push("Plugin ID contains spaces — consider using kebab-case".to_string());
    }

    // 4. Check entry file exists
    let entry_path = plugin_dir.join(&manifest.entry);
    if !entry_path.exists() {
        result.warnings.push(format!(
            "Entry file not found: {} (run build first?)",
            manifest.entry.display()
        ));
    }

    // 5. Check for duplicate tool names (V2 tools from TOML manifest)
    if let Some(ref tools) = manifest.tools_v2 {
        let mut tool_names = HashSet::new();
        for tool in tools {
            if !tool_names.insert(&tool.name) {
                result
                    .errors
                    .push(format!("Duplicate tool name: '{}'", tool.name));
            }
        }
    }

    // 6. Check for duplicate hook handler+event pairs (V2 hooks)
    if let Some(ref hooks) = manifest.hooks_v2 {
        let mut hook_keys = HashSet::new();
        for hook in hooks {
            let handler_name = hook.handler.as_deref().unwrap_or("(default)");
            let key = format!("{}:{}", hook.event, handler_name);
            if !hook_keys.insert(key) {
                result.warnings.push(format!(
                    "Duplicate hook handler '{}' for event '{}'",
                    handler_name, hook.event
                ));
            }
        }
    }

    // 7. Version check (if present)
    if let Some(ref version) = manifest.version {
        if !version.is_empty() {
            // Simple semver check: should match X.Y.Z pattern
            let parts: Vec<&str> = version.split('.').collect();
            if parts.len() != 3 || !parts.iter().all(|p| p.parse::<u32>().is_ok()) {
                result.warnings.push(format!(
                    "Version '{}' is not valid semver (expected X.Y.Z)",
                    version
                ));
            }
        }
    }

    // 8. Summary
    let tool_count = manifest.tools_v2.as_ref().map_or(0, |t| t.len());
    let hook_count = manifest.hooks_v2.as_ref().map_or(0, |h| h.len());
    if result.errors.is_empty() {
        result.info.push(format!(
            "Validation passed: {} tool(s), {} hook(s)",
            tool_count, hook_count
        ));
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn valid_minimal_manifest() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("aleph.plugin.toml"),
            r#"
[plugin]
id = "test-plugin"
name = "Test Plugin"
kind = "static"
entry = "SKILL.md"
"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("SKILL.md"), "# Test Skill").unwrap();

        let result = validate_plugin(dir.path());
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
    }

    #[test]
    fn missing_manifest() {
        let dir = tempdir().unwrap();
        let result = validate_plugin(dir.path());
        assert!(!result.is_valid());
        assert!(result.errors[0].contains("manifest"));
    }

    #[test]
    fn missing_entry_file_is_warning() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("aleph.plugin.toml"),
            r#"
[plugin]
id = "test"
name = "Test"
kind = "nodejs"
entry = "dist/index.js"
"#,
        )
        .unwrap();

        let result = validate_plugin(dir.path());
        // Should be valid (missing entry is only a warning)
        assert!(result.is_valid(), "Errors: {:?}", result.errors);
        assert!(result
            .warnings
            .iter()
            .any(|w| w.contains("entry") || w.contains("Entry")));
    }

    #[test]
    fn duplicate_tool_names_error() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("aleph.plugin.toml"),
            r#"
[plugin]
id = "test"
name = "Test"
kind = "static"
entry = "SKILL.md"

[[tools]]
name = "my_tool"
description = "First"
handler = "handle1"

[[tools]]
name = "my_tool"
description = "Duplicate"
handler = "handle2"
"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("SKILL.md"), "# Skill").unwrap();

        let result = validate_plugin(dir.path());
        assert!(!result.is_valid());
        assert!(result
            .errors
            .iter()
            .any(|e| e.contains("Duplicate") || e.contains("duplicate")));
    }

    #[test]
    fn nonexistent_directory() {
        let result = validate_plugin(Path::new("/nonexistent/path/to/plugin"));
        assert!(!result.is_valid());
        assert!(result.errors[0].contains("exist"));
    }
}
