//! Plugin manifest parsing and validation
//!
//! Supported formats (in priority order):
//! 1. `.claude-plugin/plugin.toml` (CC-compat, preferred)
//! 2. `.claude-plugin/plugin.json` (CC-compat, read-only)
//! 3. `aleph.plugin.toml` (deprecated — warn)
//! 4. Auto-discover from well-known component directories

pub mod adapter;
pub mod adapters;
pub mod cc_plugin_json;
pub mod cc_plugin_toml;
pub mod component_source;
pub mod config_validation;
pub mod declared_sections;
pub mod manifest_cache;
pub mod parsers;
pub mod toml_types;
mod types;

// Re-export the LRU cache + global default instance so the rest of the
// extension system (specifically `ExtensionManager::load_all` and the hot
// reload path) can consult + populate it without holding its own copy.
pub use manifest_cache::{ManifestCache, MAX_ENTRIES};

/// Process-wide manifest parse cache.
///
/// Single shared instance (LRU, 512 entries — see [`MAX_ENTRIES`]). Used by
/// `parse_manifest_from_dir_cached_global` for hot paths (boot, hot-reload);
/// tests can ignore it and pass their own cache to
/// [`parse_manifest_from_dir_sync_with`] to keep isolation.
pub static GLOBAL_MANIFEST_CACHE: std::sync::OnceLock<ManifestCache> = std::sync::OnceLock::new();

/// Convenience accessor that initializes the global cache on first use.
#[must_use]
pub fn global_manifest_cache() -> &'static ManifestCache {
    GLOBAL_MANIFEST_CACHE.get_or_init(ManifestCache::new)
}

/// Like [`parse_manifest_from_dir_sync`] but consults + populates the
/// process-wide [`GLOBAL_MANIFEST_CACHE`].
pub fn parse_manifest_from_dir_cached_global(dir: &Path) -> ExtensionResult<PluginManifest> {
    parse_manifest_from_dir_sync_with(dir, Some(global_manifest_cache()))
}

// Re-export TOML types and parsing
pub use cc_plugin_json::{
    parse_cc_plugin_json, parse_cc_plugin_json_content, parse_cc_plugin_json_sync, CC_PLUGIN_JSON,
};
pub use cc_plugin_toml::{
    parse_cc_plugin_toml, parse_cc_plugin_toml_content, parse_cc_plugin_toml_sync, CC_PLUGIN_TOML,
};
pub use toml_types::{
    convert_permissions, parse_aleph_plugin_toml, parse_aleph_plugin_toml_content,
    parse_aleph_plugin_toml_sync, AlephPluginToml, CapabilitiesSection, CommandSection,
    FilesystemPermission, HookSection, PermissionsSection, PluginAuthorToml, PluginSection,
    PromptSection, ServiceSection, ToolSection, ALEPH_PLUGIN_TOML,
};
pub use types::{
    AlephExtensions, AlephRuntime, AuthorInfo, ConfigUiHint, FilesystemAccess, PluginManifest,
    PluginPermission,
};

// Re-export config-schema validation helpers (activates the manifest's
// `config_schema` / `config_ui_hints` fields).
pub use config_validation::{
    summarize_ui_hints, validate_config_against_schema, validate_config_schema_declaration,
    UiHintReport,
};

use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::types::PluginKind;
use std::path::Path;

// =============================================================================
// Plugin ID Utilities
// =============================================================================

/// Sanitize a string into a valid plugin ID.
///
/// Converts to lowercase, replaces invalid chars with hyphens,
/// collapses consecutive hyphens, and trims leading/trailing hyphens.
#[must_use]
pub fn sanitize_plugin_id(name: &str) -> String {
    let mut id: String = name
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();

    while id.contains("--") {
        id = id.replace("--", "-");
    }

    id.trim_matches('-').to_string()
}

/// Validate a plugin ID.
///
/// Must: start with lowercase letter, contain only `[a-z0-9-]`,
/// no consecutive hyphens, no leading/trailing hyphens, max 64 chars.
pub fn validate_plugin_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("Plugin ID cannot be empty".to_string());
    }
    if id.len() > 64 {
        return Err(format!("Plugin ID too long ({} chars, max 64)", id.len()));
    }
    let first_char = id
        .chars()
        .next()
        .ok_or_else(|| "Plugin ID cannot be empty".to_string())?;
    if !first_char.is_ascii_lowercase() {
        return Err("Plugin ID must start with a lowercase letter".to_string());
    }
    for ch in id.chars() {
        if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() && ch != '-' {
            return Err(format!(
                "Invalid character '{ch}'. Only lowercase letters, numbers, and hyphens allowed"
            ));
        }
    }
    if id.contains("--") {
        return Err("Plugin ID cannot contain consecutive hyphens".to_string());
    }
    if id.starts_with('-') || id.ends_with('-') {
        return Err("Plugin ID cannot start or end with a hyphen".to_string());
    }
    Ok(())
}

// =============================================================================
// Manifest File Names
// =============================================================================

/// Aleph native manifest filename (deprecated)
pub const ALEPH_PLUGIN_MANIFEST: &str = "aleph.plugin.json";

// =============================================================================
// Auto-Detection Parser
// =============================================================================

/// Parse a plugin manifest from a directory (async).
///
/// Checks formats in priority order:
/// 1. `.claude-plugin/plugin.toml`
/// 2. `.claude-plugin/plugin.json`
/// 3. `aleph.plugin.toml` (deprecated)
/// 4. Auto-discover from component directories
pub async fn parse_manifest_from_dir(dir: &Path) -> ExtensionResult<PluginManifest> {
    // 1. .claude-plugin/plugin.toml (CC-compat, preferred)
    let cc_toml_path = dir.join(CC_PLUGIN_TOML);
    if cc_toml_path.exists() {
        return parse_cc_plugin_toml(dir).await;
    }

    // 2. .claude-plugin/plugin.json (CC-compat, read-only)
    let cc_json_path = dir.join(CC_PLUGIN_JSON);
    if cc_json_path.exists() {
        return parse_cc_plugin_json(dir).await;
    }

    // 3. aleph.plugin.toml (deprecated — warn)
    let aleph_toml_path = dir.join(ALEPH_PLUGIN_TOML);
    if aleph_toml_path.exists() {
        tracing::warn!(
            "Plugin at {:?} uses deprecated aleph.plugin.toml format. Migrate to .claude-plugin/plugin.toml",
            dir
        );
        return parse_aleph_plugin_toml(dir).await;
    }

    // 4. Auto-discover (no manifest file at all)
    auto_discover_manifest(dir)
}

/// Synchronous version of `parse_manifest_from_dir`
pub fn parse_manifest_from_dir_sync(dir: &Path) -> ExtensionResult<PluginManifest> {
    parse_manifest_from_dir_sync_with(dir, None)
}

/// Synchronous version of `parse_manifest_from_dir` that consults and
/// populates a [`ManifestCache`]. Cache hits skip the parse + deserialize
/// entirely; the (path, size, mtime, ctime, dev, ino) tuple invalidates the
/// entry on any in-place edit.
///
/// When `cache` is `None`, this is identical to
/// [`parse_manifest_from_dir_sync`].
pub fn parse_manifest_from_dir_sync_with(
    dir: &Path,
    cache: Option<&manifest_cache::ManifestCache>,
) -> ExtensionResult<PluginManifest> {
    // 1. .claude-plugin/plugin.toml (CC-compat, preferred)
    let cc_toml_path = dir.join(CC_PLUGIN_TOML);
    if let Some(outcome) = try_cached_or_parse(&cc_toml_path, dir, cache, parse_cc_plugin_toml_sync)
    {
        return outcome;
    }

    // 2. .claude-plugin/plugin.json (CC-compat, read-only)
    let cc_json_path = dir.join(CC_PLUGIN_JSON);
    if let Some(outcome) = try_cached_or_parse(&cc_json_path, dir, cache, parse_cc_plugin_json_sync)
    {
        return outcome;
    }

    // 3. aleph.plugin.toml (deprecated — warn)
    let aleph_toml_path = dir.join(ALEPH_PLUGIN_TOML);
    if let Some(outcome) = try_cached_or_parse(&aleph_toml_path, dir, cache, |d| {
        tracing::warn!(
                "Plugin at {:?} uses deprecated aleph.plugin.toml format. Migrate to .claude-plugin/plugin.toml",
                d
            );
        parse_aleph_plugin_toml_sync(d)
    }) {
        return outcome;
    }

    // 4. Auto-discover (no manifest file at all)
    auto_discover_manifest(dir)
}

/// Look up the parsed manifest in `cache` for `manifest_path`; on miss, call
/// `parser(dir)` to produce one and (if `cache` is provided) populate it.
///
/// Returns `None` when `manifest_path` does not exist on disk (caller should
/// move to the next candidate). Returns `Some(result)` when the file
/// existed — the inner `Result` propagates parse errors so the caller
/// doesn't accidentally fall through to the next candidate on a bad
/// manifest.
fn try_cached_or_parse<F>(
    manifest_path: &Path,
    dir: &Path,
    cache: Option<&manifest_cache::ManifestCache>,
    parser: F,
) -> Option<ExtensionResult<PluginManifest>>
where
    F: FnOnce(&Path) -> ExtensionResult<PluginManifest>,
{
    if !manifest_path.exists() {
        return None;
    }

    if let Some(cache) = cache {
        if let Ok(meta) = std::fs::metadata(manifest_path) {
            if let Some(manifest) = cache.get(manifest_path, &meta) {
                return Some(Ok(manifest));
            }
        }
    }

    match parser(dir) {
        Ok(manifest) => {
            if let Some(cache) = cache {
                if let Ok(meta) = std::fs::metadata(manifest_path) {
                    cache.put(manifest_path, &meta, manifest.clone());
                }
            }
            Some(Ok(manifest))
        }
        Err(e) => Some(Err(e)),
    }
}

// =============================================================================
// Auto-discover (inline — replaces old auto_discover.rs)
// =============================================================================

/// Component locations that signal a valid plugin directory
const COMPONENT_DIRS: &[(&str, Option<&str>)] = &[
    ("skills", Some("SKILL.md")),
    ("agents", Some(".md")),
    ("commands", Some(".md")),
    ("hooks/hooks.json", None),
    (".mcp.json", None),
    (".lsp.json", None),
];

/// Attempt to auto-discover a plugin by scanning well-known component locations.
pub fn auto_discover_manifest(dir: &Path) -> ExtensionResult<PluginManifest> {
    if has_any_component(dir) {
        let dir_name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        let id = sanitize_plugin_id(dir_name);
        let mut manifest = PluginManifest::new(id.clone(), id, PluginKind::Static, ".".into());
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

fn has_any_component(root: &Path) -> bool {
    for &(dir_path, suffix) in COMPONENT_DIRS {
        let target = root.join(dir_path);
        match suffix {
            None => {
                if target.exists() {
                    return true;
                }
            }
            Some(suf) => {
                if let Ok(entries) = std::fs::read_dir(&target) {
                    for entry in entries.flatten() {
                        let name = entry.file_name();
                        if name.to_str().is_some_and(|s| s.ends_with(suf)) {
                            return true;
                        }
                        // Recurse one level for skills/*/SKILL.md pattern
                        if suf == "SKILL.md" {
                            let nested = entry.path();
                            if nested.is_dir() {
                                if let Ok(inner) = std::fs::read_dir(&nested) {
                                    for inner_entry in inner.flatten() {
                                        if inner_entry
                                            .file_name()
                                            .to_str()
                                            .is_some_and(|s| s.ends_with(suf))
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
// Re-export frontmatter parsing
// =============================================================================

/// Parse YAML frontmatter from markdown content
pub fn parse_frontmatter<T: serde::de::DeserializeOwned + Default>(
    content: &str,
    path: &Path,
) -> ExtensionResult<(T, String)> {
    let content = content.trim();

    if !content.starts_with("---") {
        return Ok((T::default(), content.to_string()));
    }

    let rest = &content[3..];
    let end_pos = rest.find("\n---").or_else(|| rest.find("\r\n---"));

    match end_pos {
        Some(pos) => {
            let frontmatter_str = &rest[..pos].trim();
            let body_start = pos + 4;
            let body = rest[body_start..].trim().to_string();

            if frontmatter_str.is_empty() {
                return Ok((T::default(), body));
            }

            let frontmatter: T = serde_yml::from_str(frontmatter_str)
                .map_err(|e| ExtensionError::yaml_parse(path, format!("YAML error: {e}")))?;

            Ok((frontmatter, body))
        }
        None => Ok((T::default(), content.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::types::SkillFrontmatter;
    use tempfile::TempDir;

    #[test]
    fn test_parse_frontmatter() {
        let content = "---\nname: test\ndescription: A test skill\n---\n\nThis is the body.";
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
        let content = "---\n---\n\nBody content.";
        let (fm, body) =
            parse_frontmatter::<SkillFrontmatter>(content, Path::new("/test")).unwrap();
        assert!(fm.name.is_none());
        assert_eq!(body, "Body content.");
    }

    #[test]
    fn test_sanitize_plugin_id() {
        assert_eq!(sanitize_plugin_id("My Plugin"), "my-plugin");
        assert_eq!(sanitize_plugin_id("my_plugin"), "my-plugin");
        assert_eq!(sanitize_plugin_id("my--plugin"), "my-plugin");
        assert_eq!(sanitize_plugin_id("-my-plugin-"), "my-plugin");
        assert_eq!(sanitize_plugin_id("Plugin 123"), "plugin-123");
    }

    #[test]
    fn test_parse_manifest_from_dir_toml() {
        let temp_dir = TempDir::new().unwrap();
        std::fs::write(
            temp_dir.path().join("aleph.plugin.toml"),
            "[plugin]\nid = \"toml-plugin\"\nname = \"TOML Plugin\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();

        let manifest = parse_manifest_from_dir_sync(temp_dir.path()).unwrap();
        assert_eq!(manifest.id, "toml-plugin");
        assert_eq!(manifest.name, "TOML Plugin");
        assert_eq!(manifest.version, Some("1.0.0".to_string()));
        assert_eq!(manifest.root_dir, temp_dir.path());
    }

    #[test]
    fn test_parse_manifest_from_dir_legacy_claude() {
        let temp_dir = TempDir::new().unwrap();
        let claude_dir = temp_dir.path().join(".claude-plugin");
        std::fs::create_dir(&claude_dir).unwrap();
        std::fs::write(
            claude_dir.join("plugin.json"),
            r#"{"name": "Legacy Plugin", "version": "0.1.0"}"#,
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
        assert!(matches!(
            result.unwrap_err(),
            ExtensionError::InvalidManifest { .. }
        ));
    }

    #[test]
    fn test_auto_discover_skills() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("skills").join("hello");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# Hello").unwrap();

        let manifest = auto_discover_manifest(tmp.path()).unwrap();
        assert_eq!(manifest.kind, PluginKind::Static);
        assert_eq!(manifest.root_dir, tmp.path());
    }

    #[test]
    fn test_auto_discover_empty_dir_fails() {
        let tmp = TempDir::new().unwrap();
        let result = auto_discover_manifest(tmp.path());
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_parse_manifest_from_dir_async_toml() {
        let temp_dir = TempDir::new().unwrap();
        tokio::fs::write(
            temp_dir.path().join("aleph.plugin.toml"),
            "[plugin]\nid = \"async-toml-test\"\n",
        )
        .await
        .unwrap();

        let manifest = parse_manifest_from_dir(temp_dir.path()).await.unwrap();
        assert_eq!(manifest.id, "async-toml-test");
    }
}
