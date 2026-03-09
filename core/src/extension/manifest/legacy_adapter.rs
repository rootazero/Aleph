//! Adapter for converting LegacyPluginManifest to PluginManifest (V2)
//!
//! Converts `.claude-plugin/plugin.json` format into the unified PluginManifest.

use super::types::{AuthorInfo, PluginManifest};
use super::{sanitize_plugin_id, LegacyPluginManifest, PluginRepository};
use crate::extension::types::PluginKind;
use std::path::Path;

/// Detect plugin kind from directory contents
pub fn detect_plugin_kind(dir: &Path) -> PluginKind {
    if dir.join("package.json").exists() {
        PluginKind::NodeJs
    } else if dir.join("plugin.wasm").exists() {
        PluginKind::Wasm
    } else {
        PluginKind::Static
    }
}

/// Detect entry point based on plugin kind and directory contents
pub fn detect_entry_point(_dir: &Path, kind: &PluginKind) -> String {
    match kind {
        PluginKind::NodeJs => "index.js".to_string(),
        PluginKind::Wasm => "plugin.wasm".to_string(),
        PluginKind::Static => ".".to_string(),
    }
}

/// Convert a LegacyPluginManifest to the unified PluginManifest format
pub fn adapt_legacy_manifest(
    legacy: &LegacyPluginManifest,
    plugin_dir: &Path,
) -> Result<PluginManifest, String> {
    if legacy.name.is_empty() {
        return Err("Legacy manifest missing required 'name' field".to_string());
    }

    let id = sanitize_plugin_id(&legacy.name);
    let kind = detect_plugin_kind(plugin_dir);
    let entry = detect_entry_point(plugin_dir, &kind);

    let mut manifest = PluginManifest::new(id, legacy.name.clone(), kind, entry.into());

    manifest.version = legacy.version.clone();
    manifest.description = legacy.description.clone();
    manifest.license = legacy.license.clone();
    manifest.keywords = legacy.keywords.clone().unwrap_or_default();
    manifest.root_dir = plugin_dir.to_path_buf();

    // Convert author
    if let Some(ref author) = legacy.author {
        manifest.author = Some(AuthorInfo {
            name: Some(author.name.clone()),
            email: author.email.clone(),
            url: author.url.clone(),
        });
    }

    // Convert repository URL
    if let Some(ref repo) = legacy.repository {
        manifest.repository = Some(match repo {
            PluginRepository::Url(url) => url.clone(),
            PluginRepository::Detailed { url, .. } => url.clone(),
        });
    }

    manifest.homepage = legacy.homepage.clone();

    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::manifest::PluginAuthor;
    use tempfile::TempDir;

    fn make_legacy(name: &str) -> LegacyPluginManifest {
        LegacyPluginManifest {
            name: name.to_string(),
            version: None,
            description: None,
            author: None,
            homepage: None,
            repository: None,
            license: None,
            keywords: None,
            commands: None,
            skills: None,
            agents: None,
            hooks: None,
            mcp_servers: None,
        }
    }

    #[test]
    fn test_detect_nodejs_plugin() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        assert_eq!(detect_plugin_kind(dir.path()), PluginKind::NodeJs);
    }

    #[test]
    fn test_detect_wasm_plugin() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("plugin.wasm"), &[0u8; 4]).unwrap();
        assert_eq!(detect_plugin_kind(dir.path()), PluginKind::Wasm);
    }

    #[test]
    fn test_detect_static_plugin() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("SKILL.md"), "# My Skill").unwrap();
        assert_eq!(detect_plugin_kind(dir.path()), PluginKind::Static);
    }

    #[test]
    fn test_adapt_legacy_manifest() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("SKILL.md"), "# Skill").unwrap();

        let mut legacy = make_legacy("My Cool Plugin");
        legacy.version = Some("1.2.3".to_string());
        legacy.description = Some("A cool plugin".to_string());
        legacy.license = Some("MIT".to_string());
        legacy.keywords = Some(vec!["cool".to_string(), "plugin".to_string()]);
        legacy.author = Some(PluginAuthor {
            name: "Alice".to_string(),
            email: Some("alice@example.com".to_string()),
            url: None,
        });

        let manifest = adapt_legacy_manifest(&legacy, dir.path()).unwrap();

        assert_eq!(manifest.id, "my-cool-plugin");
        assert_eq!(manifest.name, "My Cool Plugin");
        assert_eq!(manifest.version, Some("1.2.3".to_string()));
        assert_eq!(manifest.description, Some("A cool plugin".to_string()));
        assert_eq!(manifest.license, Some("MIT".to_string()));
        assert_eq!(manifest.keywords, vec!["cool", "plugin"]);
        assert_eq!(manifest.kind, PluginKind::Static);
        assert_eq!(manifest.entry.to_str().unwrap(), ".");
        assert_eq!(manifest.root_dir, dir.path());

        let author = manifest.author.unwrap();
        assert_eq!(author.name, Some("Alice".to_string()));
        assert_eq!(author.email, Some("alice@example.com".to_string()));

        // V2 fields should all be None
        assert!(manifest.tools_v2.is_none());
        assert!(manifest.hooks_v2.is_none());
        assert!(manifest.commands_v2.is_none());
        assert!(manifest.services_v2.is_none());
        assert!(manifest.capabilities_v2.is_none());
    }
}
