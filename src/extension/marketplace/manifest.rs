//! Marketplace manifest parsing — reads marketplace.toml / marketplace.json
//! from a cloned marketplace repository.
//!
//! File layout inside a marketplace repo:
//!   .claude-plugin/marketplace.toml   (preferred)
//!   .claude-plugin/marketplace.json   (CC-compatible fallback)

use std::path::Path;

use super::types::MarketplaceManifest;

// =============================================================================
// Constants
// =============================================================================

const MANIFEST_DIR: &str = ".claude-plugin";
const TOML_FILE: &str = "marketplace.toml";
const JSON_FILE: &str = "marketplace.json";

// =============================================================================
// Parse functions
// =============================================================================

/// Parse a marketplace manifest from TOML text.
pub fn parse_marketplace_toml_content(content: &str) -> Result<MarketplaceManifest, String> {
    toml::from_str(content).map_err(|e| format!("Failed to parse marketplace TOML: {e}"))
}

/// Parse a marketplace manifest from JSON text.
pub fn parse_marketplace_json_content(content: &str) -> Result<MarketplaceManifest, String> {
    serde_json::from_str(content).map_err(|e| format!("Failed to parse marketplace JSON: {e}"))
}

/// Parse a marketplace manifest from a directory.
///
/// Search order:
/// 1. `<dir>/.claude-plugin/marketplace.toml` (preferred)
/// 2. `<dir>/.claude-plugin/marketplace.json` (CC-compat fallback)
pub fn parse_marketplace_manifest(dir: &Path) -> Result<MarketplaceManifest, String> {
    let toml_path = dir.join(MANIFEST_DIR).join(TOML_FILE);
    let json_path = dir.join(MANIFEST_DIR).join(JSON_FILE);

    if toml_path.exists() {
        let content = std::fs::read_to_string(&toml_path)
            .map_err(|e| format!("Failed to read {}: {e}", toml_path.display()))?;
        return parse_marketplace_toml_content(&content);
    }

    if json_path.exists() {
        let content = std::fs::read_to_string(&json_path)
            .map_err(|e| format!("Failed to read {}: {e}", json_path.display()))?;
        return parse_marketplace_json_content(&content);
    }

    Err(format!(
        "No marketplace manifest found in {} (looked for {TOML_FILE} and {JSON_FILE} under {MANIFEST_DIR}/)",
        dir.display()
    ))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_toml() -> &'static str {
        r#"
name = "test-marketplace"

[owner]
name = "Alice"
email = "alice@example.com"
url = "https://example.com"

[metadata]
description = "A test marketplace"
version = "1.0.0"
plugin-root = "plugins"

[[plugins]]
name = "hello-plugin"
source = "owner/hello-plugin"
description = "Says hello"
version = "0.1.0"

[[plugins]]
name = "world-plugin"
source = "owner/world-plugin"
"#
    }

    fn make_json() -> &'static str {
        r#"{
  "name": "json-marketplace",
  "owner": { "name": "Bob" },
  "plugins": [
    { "name": "foo", "source": "owner/foo", "version": "1.2.3" }
  ]
}"#
    }

    #[test]
    fn test_parse_marketplace_toml() {
        let manifest = parse_marketplace_toml_content(make_toml()).unwrap();
        assert_eq!(manifest.name, "test-marketplace");
        let owner = manifest.owner.unwrap();
        assert_eq!(owner.name, "Alice");
        assert_eq!(owner.email.as_deref(), Some("alice@example.com"));
        assert_eq!(owner.url.as_deref(), Some("https://example.com"));
        assert_eq!(
            manifest.metadata.description.as_deref(),
            Some("A test marketplace")
        );
        assert_eq!(manifest.metadata.version.as_deref(), Some("1.0.0"));
        assert_eq!(manifest.metadata.plugin_root.as_deref(), Some("plugins"));
        assert_eq!(manifest.plugins.len(), 2);
        assert_eq!(manifest.plugins[0].name, "hello-plugin");
        assert_eq!(
            manifest.plugins[0].source.as_relative_path(),
            Some("owner/hello-plugin")
        );
        assert_eq!(
            manifest.plugins[0].description.as_deref(),
            Some("Says hello")
        );
        assert_eq!(manifest.plugins[0].version.as_deref(), Some("0.1.0"));
        assert_eq!(manifest.plugins[1].name, "world-plugin");
        assert!(manifest.plugins[1].description.is_none());
    }

    #[test]
    fn test_parse_marketplace_json() {
        let manifest = parse_marketplace_json_content(make_json()).unwrap();
        assert_eq!(manifest.name, "json-marketplace");
        let owner = manifest.owner.unwrap();
        assert_eq!(owner.name, "Bob");
        assert_eq!(manifest.plugins.len(), 1);
        assert_eq!(manifest.plugins[0].name, "foo");
        assert_eq!(manifest.plugins[0].version.as_deref(), Some("1.2.3"));
    }

    #[test]
    fn test_parse_marketplace_from_dir_prefers_toml() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = tmp.path().join(".claude-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();

        // Write both files — TOML should win.
        fs::write(plugin_dir.join("marketplace.toml"), make_toml()).unwrap();
        fs::write(plugin_dir.join("marketplace.json"), make_json()).unwrap();

        let manifest = parse_marketplace_manifest(tmp.path()).unwrap();
        // TOML has name "test-marketplace", JSON has "json-marketplace"
        assert_eq!(manifest.name, "test-marketplace");
    }

    #[test]
    fn test_parse_marketplace_from_dir_json_fallback() {
        let tmp = TempDir::new().unwrap();
        let plugin_dir = tmp.path().join(".claude-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();

        fs::write(plugin_dir.join("marketplace.json"), make_json()).unwrap();

        let manifest = parse_marketplace_manifest(tmp.path()).unwrap();
        assert_eq!(manifest.name, "json-marketplace");
    }

    #[test]
    fn test_parse_marketplace_from_dir_missing() {
        let tmp = TempDir::new().unwrap();
        let result = parse_marketplace_manifest(tmp.path());
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("No marketplace manifest found"), "got: {msg}");
    }

    /// Claude Code's `PluginSourceSchema` is a six-arm union. Aleph modelled
    /// `source` as a bare `String` until 2026-08-19, and serde fails the whole
    /// struct on a type mismatch — so a single `{"source":"github",...}` entry
    /// made the entire marketplace unparseable and **every** plugin in it
    /// invisible. The per-entry refusal is the point: the manifest still loads.
    #[test]
    fn an_object_source_does_not_take_the_whole_marketplace_down() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join(".claude-plugin");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("marketplace.json"),
            r#"{
              "name": "mixed",
              "plugins": [
                {"name": "local-one", "source": "./plugins/local-one"},
                {"name": "gh-one", "source": {"source": "github", "repo": "owner/repo"}},
                {"name": "npm-one", "source": {"source": "npm", "package": "x"}},
                {"name": "future-one", "source": {"source": "not-invented-yet", "x": 1}}
              ]
            }"#,
        )
        .unwrap();

        // The oracle for why this type is an enum: the shape it replaced
        // cannot read this file at all. This stays in the test rather than
        // being a one-off mutation, because "a String would also work" is
        // exactly the simplification a future reader will attempt.
        #[derive(serde::Deserialize)]
        struct OldEntry {
            #[allow(dead_code)]
            name: String,
            #[allow(dead_code)]
            source: String,
        }
        #[derive(serde::Deserialize)]
        struct OldManifest {
            #[allow(dead_code)]
            plugins: Vec<OldEntry>,
        }
        let raw = std::fs::read_to_string(plugin_dir.join("marketplace.json")).unwrap();
        assert!(
            serde_json::from_str::<OldManifest>(&raw).is_err(),
            "the pre-2026-08-19 `source: String` shape must fail on this file — \
             if it parses, this test is no longer proving anything"
        );

        let manifest = parse_marketplace_manifest(dir.path()).unwrap();
        assert_eq!(
            manifest.plugins.len(),
            4,
            "every entry must survive, including the forms Aleph cannot install"
        );
        assert_eq!(
            manifest.plugins[0].source.as_relative_path(),
            Some("./plugins/local-one")
        );
        assert_eq!(manifest.plugins[1].source.external_kind(), Some("github"));
        assert_eq!(manifest.plugins[2].source.external_kind(), Some("npm"));
        assert_eq!(
            manifest.plugins[3].source.external_kind(),
            Some("not-invented-yet"),
            "an arm Claude Code adds later must parse, not brick the marketplace"
        );
        for entry in &manifest.plugins[1..] {
            assert!(
                entry.source.as_relative_path().is_none(),
                "an external source must not resolve to a path inside the marketplace"
            );
        }
    }
}
