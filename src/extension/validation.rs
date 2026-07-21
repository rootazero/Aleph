//! Plugin validation — checks manifest, entry point, registration uniqueness,
//! and configuration-schema soundness.
//!
//! Used as the pre-install gate in
//! [`crate::extension::marketplace::MarketplaceManager::install_to_scope`] and
//! exposed for direct validation of a plugin directory.

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
    #[must_use]
    pub const fn is_valid(&self) -> bool {
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
#[must_use]
pub fn validate_plugin(plugin_dir: &Path) -> ValidationResult {
    let mut result = ValidationResult::default();

    // 1. Check directory exists
    if !plugin_dir.exists() {
        result.errors.push(format!(
            "Directory does not exist: {}",
            plugin_dir.display()
        ));
        return result;
    }

    // 2. Parse manifest (sync version — no runtime needed)
    let manifest = match super::manifest::parse_manifest_from_dir_sync(plugin_dir) {
        Ok(m) => m,
        Err(e) => {
            result.errors.push(format!("Failed to parse manifest: {e}"));
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

    match manifest.entry_path() {
        Ok(entry_path) if !entry_path.exists() => result.warnings.push(format!(
            "Entry file not found: {} (run build first?)",
            manifest.entry.display()
        )),
        Ok(_) => {}
        Err(e) => result.errors.push(e.to_string()),
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
                    "Version '{version}' is not valid semver (expected X.Y.Z)"
                ));
            }
        }
    }

    // 8. Configuration schema soundness (activates manifest `config_schema` /
    //    `config_ui_hints`). A declared schema that does not compile would
    //    silently reject every user config at runtime, so it is an error. When
    //    a sample config file ships alongside the plugin, validate it too.
    if let Some(ref schema) = manifest.config_schema {
        match super::manifest::validate_config_schema_declaration(schema) {
            Ok(()) => {
                result.info.push("Config schema is valid".to_string());

                // Validate a shipped sample config, if present. Authors keep one
                // so users have a starting point — catch drift between the
                // schema and its own example before publishing.
                for sample in ["config.json", "config.default.json"] {
                    let sample_path = plugin_dir.join(sample);
                    if !sample_path.exists() {
                        continue;
                    }
                    match std::fs::read_to_string(&sample_path)
                        .ok()
                        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
                    {
                        Some(value) => {
                            for err in
                                super::manifest::validate_config_against_schema(schema, &value)
                            {
                                result.errors.push(format!("{sample}: {err}"));
                            }
                        }
                        None => result
                            .warnings
                            .push(format!("Could not parse sample config '{sample}' as JSON")),
                    }
                }
            }
            Err(e) => result.errors.push(e),
        }
    }

    // 9. Surface UI-hint metadata (read consumer for `config_ui_hints`).
    if !manifest.config_ui_hints.is_empty() {
        let report = super::manifest::summarize_ui_hints(&manifest.config_ui_hints);
        result.info.push(format!(
            "Config UI hints: {} field(s), {} sensitive, {} advanced",
            report.total, report.sensitive, report.advanced
        ));
    }

    // 10. Summary
    let tool_count = manifest.tools_v2.as_ref().map_or(0, |t| t.len());
    let hook_count = manifest.hooks_v2.as_ref().map_or(0, |h| h.len());
    if result.errors.is_empty() {
        result.info.push(format!(
            "Validation passed: {tool_count} tool(s), {hook_count} hook(s)"
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
kind = "mcp"
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
