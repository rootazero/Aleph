//! Auto-discover manifest builder for plugins with no manifest file
//!
//! When no manifest file is found, this module scans default component locations
//! to construct a minimal `PluginManifest`. Any matching component file is sufficient
//! to treat the directory as a plugin.

use std::path::Path;

use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::manifest::aleph_plugin::sanitize_plugin_id;
use crate::extension::manifest::types::PluginManifest;
use crate::extension::types::PluginKind;

// =============================================================================
// Component discovery patterns
// =============================================================================

/// Glob-like subpath patterns that indicate components exist.
///
/// We do a simple directory-listing check rather than full glob to stay sync
/// and dependency-free. Each entry is either a literal path (checked with
/// `exists()`) or a "glob dir" pattern (directory + file suffix).
struct ComponentCheck {
    /// Directory to list (relative to plugin root)
    dir: &'static str,
    /// Required file suffix within that directory, or None for plain exists check
    suffix: Option<&'static str>,
}

/// All component locations that signal a valid plugin.
const COMPONENT_CHECKS: &[ComponentCheck] = &[
    // skills/*/SKILL.md
    ComponentCheck { dir: "skills", suffix: Some("SKILL.md") },
    // agents/*.md
    ComponentCheck { dir: "agents", suffix: Some(".md") },
    // commands/*.md
    ComponentCheck { dir: "commands", suffix: Some(".md") },
    // hooks/hooks.json (plain file)
    ComponentCheck { dir: "hooks/hooks.json", suffix: None },
    // .mcp.json (plain file)
    ComponentCheck { dir: ".mcp.json", suffix: None },
    // .lsp.json (plain file)
    ComponentCheck { dir: ".lsp.json", suffix: None },
];

// =============================================================================
// Auto-discovery
// =============================================================================

/// Attempt to auto-discover a plugin by scanning well-known component locations.
///
/// Called as the last fallback in manifest discovery when no manifest file
/// (`aleph.plugin.toml`, `aleph.plugin.json`, `package.json`,
/// `.claude-plugin/plugin.toml`, `.claude-plugin/plugin.json`) is found.
///
/// # Scan targets
/// - `skills/*/SKILL.md`   → skills exist
/// - `agents/*.md`         → agents exist
/// - `commands/*.md`       → commands exist
/// - `hooks/hooks.json`    → hooks exist
/// - `.mcp.json`           → MCP servers exist
/// - `.lsp.json`           → LSP servers exist (deferred, still counts)
///
/// # Returns
/// - `Ok(PluginManifest)` with `id` derived from the directory name,
///   `kind = Static`, and `entry = "."` if any component is found.
/// - `Err(ExtensionError::InvalidManifest)` if no components are found.
pub fn auto_discover_manifest(dir: &Path) -> ExtensionResult<PluginManifest> {
    if has_any_component(dir) {
        let dir_name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        let id = sanitize_plugin_id(dir_name);

        let mut manifest =
            PluginManifest::new(id.clone(), id, PluginKind::Static, ".".into());
        manifest.root_dir = dir.to_path_buf();
        Ok(manifest)
    } else {
        Err(ExtensionError::invalid_manifest(
            dir,
            "No manifest file and no recognisable plugin components found \
             (skills/, agents/, commands/, hooks/hooks.json, .mcp.json, .lsp.json)",
        ))
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Return `true` if the directory contains at least one recognised component.
fn has_any_component(root: &Path) -> bool {
    for check in COMPONENT_CHECKS {
        let target = root.join(check.dir);
        match check.suffix {
            None => {
                // Plain file / directory existence check
                if target.exists() {
                    return true;
                }
            }
            Some(suffix) => {
                // Directory listing — any entry whose name ends with `suffix`
                if let Ok(entries) = std::fs::read_dir(&target) {
                    for entry in entries.flatten() {
                        let name = entry.file_name();
                        if name.to_str().map(|s| s.ends_with(suffix)).unwrap_or(false) {
                            return true;
                        }
                        // Also recurse one level for skills/*/SKILL.md pattern
                        if check.suffix == Some("SKILL.md") {
                            let nested = entry.path();
                            if nested.is_dir() {
                                if let Ok(inner) = std::fs::read_dir(&nested) {
                                    for inner_entry in inner.flatten() {
                                        let inner_name = inner_entry.file_name();
                                        if inner_name
                                            .to_str()
                                            .map(|s| s.ends_with(suffix))
                                            .unwrap_or(false)
                                        {
                                            return true;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    false
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_auto_discover_skills() {
        let tmp = tempdir().unwrap();
        // Create skills/hello/SKILL.md
        let skill_dir = tmp.path().join("skills").join("hello");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# Hello").unwrap();

        let manifest = auto_discover_manifest(tmp.path()).unwrap();
        assert_eq!(manifest.kind, PluginKind::Static);
        assert_eq!(manifest.entry, std::path::PathBuf::from("."));
        assert_eq!(manifest.root_dir, tmp.path());
        // id is derived from dir name — tempdir names are valid identifiers
        assert!(!manifest.id.is_empty());
    }

    #[test]
    fn test_auto_discover_agents() {
        let tmp = tempdir().unwrap();
        // Create agents/reviewer.md
        let agents_dir = tmp.path().join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("reviewer.md"), "# Reviewer").unwrap();

        let manifest = auto_discover_manifest(tmp.path()).unwrap();
        assert_eq!(manifest.kind, PluginKind::Static);
        assert_eq!(manifest.root_dir, tmp.path());
        assert!(!manifest.id.is_empty());
    }

    #[test]
    fn test_auto_discover_commands() {
        let tmp = tempdir().unwrap();
        let cmds_dir = tmp.path().join("commands");
        std::fs::create_dir_all(&cmds_dir).unwrap();
        std::fs::write(cmds_dir.join("deploy.md"), "# Deploy").unwrap();

        let manifest = auto_discover_manifest(tmp.path()).unwrap();
        assert_eq!(manifest.kind, PluginKind::Static);
    }

    #[test]
    fn test_auto_discover_mcp_json() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join(".mcp.json"), "{}").unwrap();

        let manifest = auto_discover_manifest(tmp.path()).unwrap();
        assert_eq!(manifest.kind, PluginKind::Static);
    }

    #[test]
    fn test_auto_discover_hooks_json() {
        let tmp = tempdir().unwrap();
        let hooks_dir = tmp.path().join("hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        std::fs::write(hooks_dir.join("hooks.json"), "[]").unwrap();

        let manifest = auto_discover_manifest(tmp.path()).unwrap();
        assert_eq!(manifest.kind, PluginKind::Static);
    }

    #[test]
    fn test_auto_discover_lsp_json() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join(".lsp.json"), "{}").unwrap();

        let manifest = auto_discover_manifest(tmp.path()).unwrap();
        assert_eq!(manifest.kind, PluginKind::Static);
    }

    #[test]
    fn test_auto_discover_empty_dir_fails() {
        let tmp = tempdir().unwrap();

        let result = auto_discover_manifest(tmp.path());
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            crate::extension::error::ExtensionError::InvalidManifest { .. }
        ));
    }

    #[test]
    fn test_auto_discover_id_derived_from_dir_name() {
        let tmp = tempdir().unwrap();
        let plugin_dir = tmp.path().join("My Cool Plugin");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        let agents_dir = plugin_dir.join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("agent.md"), "# Agent").unwrap();

        let manifest = auto_discover_manifest(&plugin_dir).unwrap();
        assert_eq!(manifest.id, "my-cool-plugin");
        assert_eq!(manifest.name, "my-cool-plugin");
    }
}
