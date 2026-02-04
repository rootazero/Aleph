//! package.json parser for Node.js plugins
//!
//! This module parses npm `package.json` files that contain an "aleph" field
//! for plugin metadata. This allows Node.js packages to function as Aleph plugins.

use super::types::{AuthorInfo, ConfigUiHint, PluginManifest, PluginPermission};
use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::types::PluginKind;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::Path;

// =============================================================================
// Package.json Types
// =============================================================================

/// npm package.json structure (partial)
#[derive(Debug, Deserialize)]
struct PackageJson {
    /// Package name (required)
    name: String,

    /// Package version
    #[serde(default)]
    version: Option<String>,

    /// Package description
    #[serde(default)]
    description: Option<String>,

    /// Main entry point
    #[serde(default)]
    main: Option<String>,

    /// Author (string or object)
    #[serde(default)]
    author: Option<PackageAuthor>,

    /// Homepage URL
    #[serde(default)]
    homepage: Option<String>,

    /// Repository (string or object)
    #[serde(default)]
    repository: Option<PackageRepository>,

    /// License
    #[serde(default)]
    license: Option<String>,

    /// Keywords
    #[serde(default)]
    keywords: Option<Vec<String>>,

    /// Aleph plugin configuration
    #[serde(default)]
    aleph: Option<AlephConfig>,
}

/// Package author (supports string or object format)
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PackageAuthor {
    /// String format: "Name <email> (url)"
    String(String),
    /// Object format: { name, email, url }
    Object {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        email: Option<String>,
        #[serde(default)]
        url: Option<String>,
    },
}

impl From<PackageAuthor> for AuthorInfo {
    fn from(author: PackageAuthor) -> Self {
        match author {
            PackageAuthor::String(s) => AuthorInfo::from(s.as_str()),
            PackageAuthor::Object { name, email, url } => AuthorInfo { name, email, url },
        }
    }
}

/// Package repository (supports string or object format)
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PackageRepository {
    /// Simple URL string
    String(String),
    /// Object format: { type, url }
    Object {
        #[serde(default)]
        url: Option<String>,
        #[serde(rename = "type", default)]
        _repo_type: Option<String>,
    },
}

impl PackageRepository {
    fn url(&self) -> Option<String> {
        match self {
            PackageRepository::String(s) => Some(s.clone()),
            PackageRepository::Object { url, .. } => url.clone(),
        }
    }
}

/// Aleph-specific configuration in package.json
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AlephConfig {
    /// Plugin ID override (defaults to package name)
    #[serde(default)]
    id: Option<String>,

    /// Display name override
    #[serde(default)]
    name: Option<String>,

    /// Entry point override (defaults to main)
    #[serde(default)]
    entry: Option<String>,

    /// Plugin kind override (defaults to nodejs)
    #[serde(default)]
    kind: Option<PluginKind>,

    /// Configuration schema (JSON Schema)
    #[serde(default)]
    config_schema: Option<JsonValue>,

    /// UI hints for configuration
    #[serde(default)]
    config_ui_hints: Option<HashMap<String, ConfigUiHint>>,

    /// Required permissions
    #[serde(default)]
    permissions: Option<Vec<PluginPermission>>,

    /// File extensions this plugin handles
    #[serde(default)]
    extensions: Option<Vec<String>>,
}

// =============================================================================
// Parser
// =============================================================================

/// Parse a package.json file into a PluginManifest
///
/// The package.json must have an "aleph" field to be recognized as an Aleph plugin.
/// If no "aleph" field is present, returns an error.
///
/// # Arguments
/// * `path` - Path to the package.json file
///
/// # Returns
/// * `Ok(PluginManifest)` - Parsed manifest
/// * `Err(ExtensionError)` - If parsing fails or required fields are missing
pub async fn parse_package_json(path: &Path) -> ExtensionResult<PluginManifest> {
    let content = tokio::fs::read_to_string(path).await?;
    parse_package_json_content(&content, path)
}

/// Parse package.json content into a PluginManifest
///
/// This is the sync version for use in tests and when content is already loaded.
pub fn parse_package_json_content(content: &str, path: &Path) -> ExtensionResult<PluginManifest> {
    let pkg: PackageJson = serde_json::from_str(content)
        .map_err(|e| ExtensionError::invalid_manifest(path, format!("JSON parse error: {}", e)))?;

    // Require aleph field for plugin recognition
    let aleph = pkg.aleph.ok_or_else(|| {
        ExtensionError::invalid_manifest(
            path,
            "Missing 'aleph' field - not an Aleph plugin".to_string(),
        )
    })?;

    // Validate package name
    if pkg.name.is_empty() {
        return Err(ExtensionError::missing_field(path, "name"));
    }

    // Determine plugin ID (aleph.id > package name)
    let id = aleph.id.unwrap_or_else(|| sanitize_npm_package_name(&pkg.name));

    // Validate plugin ID
    super::aleph_plugin::validate_plugin_id(&id).map_err(|e| {
        ExtensionError::invalid_manifest(path, format!("Invalid plugin ID '{}': {}", id, e))
    })?;

    // Determine display name
    let name = aleph.name.unwrap_or(pkg.name);

    // Determine entry point
    let entry = aleph
        .entry
        .or(pkg.main)
        .unwrap_or_else(|| "index.js".to_string());

    // Determine plugin kind
    let kind = aleph.kind.unwrap_or(PluginKind::NodeJs);

    // Build manifest
    let manifest = PluginManifest {
        id,
        name,
        version: pkg.version,
        description: pkg.description,
        kind,
        entry: entry.into(),
        root_dir: path.parent().map(|p| p.to_path_buf()).unwrap_or_default(),
        config_schema: aleph.config_schema,
        config_ui_hints: aleph.config_ui_hints.unwrap_or_default(),
        permissions: aleph.permissions.unwrap_or_default(),
        author: pkg.author.map(AuthorInfo::from),
        homepage: pkg.homepage,
        repository: pkg.repository.and_then(|r| r.url()),
        license: pkg.license,
        keywords: pkg.keywords.unwrap_or_default(),
        extensions: aleph.extensions.unwrap_or_default(),
        // V2 fields not available in package.json format
        tools_v2: None,
        hooks_v2: None,
        commands_v2: None,
        services_v2: None,
        prompt_v2: None,
        capabilities_v2: None,
        // P2 fields not available in package.json format
        channels_v2: None,
        providers_v2: None,
        http_routes_v2: None,
    };

    Ok(manifest)
}

/// Sanitize an npm package name to a valid plugin ID.
///
/// Handles scoped packages (@org/name) by stripping the scope prefix,
/// then delegates to the shared `sanitize_plugin_id` function.
fn sanitize_npm_package_name(name: &str) -> String {
    let mut stripped = name.to_string();

    // Remove scope prefix (@org/)
    if let Some(pos) = stripped.find('/') {
        stripped = stripped[pos + 1..].to_string();
    }

    // Use the shared sanitization logic
    super::aleph_plugin::sanitize_plugin_id(&stripped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_package_json_basic() {
        let content = r#"{
            "name": "my-plugin",
            "version": "1.0.0",
            "description": "A test plugin",
            "main": "dist/index.js",
            "aleph": {}
        }"#;

        let manifest =
            parse_package_json_content(content, Path::new("/test/package.json")).unwrap();

        assert_eq!(manifest.id, "my-plugin");
        assert_eq!(manifest.name, "my-plugin");
        assert_eq!(manifest.version, Some("1.0.0".to_string()));
        assert_eq!(manifest.description, Some("A test plugin".to_string()));
        assert_eq!(manifest.entry, PathBuf::from("dist/index.js"));
        assert_eq!(manifest.kind, PluginKind::NodeJs);
    }

    #[test]
    fn test_parse_package_json_with_aleph_config() {
        let content = r#"{
            "name": "@org/my-plugin",
            "version": "2.0.0",
            "aleph": {
                "id": "custom-id",
                "name": "Custom Plugin",
                "entry": "src/main.js",
                "permissions": ["network", "env"],
                "extensions": [".txt", ".md"]
            }
        }"#;

        let manifest =
            parse_package_json_content(content, Path::new("/test/package.json")).unwrap();

        assert_eq!(manifest.id, "custom-id");
        assert_eq!(manifest.name, "Custom Plugin");
        assert_eq!(manifest.entry, PathBuf::from("src/main.js"));
        assert_eq!(manifest.permissions.len(), 2);
        assert!(manifest.permissions.contains(&PluginPermission::Network));
        assert!(manifest.permissions.contains(&PluginPermission::Env));
        assert_eq!(manifest.extensions, vec![".txt", ".md"]);
    }

    #[test]
    fn test_parse_package_json_author_string() {
        let content = r#"{
            "name": "test-plugin",
            "author": "John Doe <john@example.com> (https://example.com)",
            "aleph": {}
        }"#;

        let manifest =
            parse_package_json_content(content, Path::new("/test/package.json")).unwrap();

        let author = manifest.author.unwrap();
        assert_eq!(author.name, Some("John Doe".to_string()));
        assert_eq!(author.email, Some("john@example.com".to_string()));
        assert_eq!(author.url, Some("https://example.com".to_string()));
    }

    #[test]
    fn test_parse_package_json_author_object() {
        let content = r#"{
            "name": "test-plugin",
            "author": {
                "name": "Jane Doe",
                "email": "jane@example.com"
            },
            "aleph": {}
        }"#;

        let manifest =
            parse_package_json_content(content, Path::new("/test/package.json")).unwrap();

        let author = manifest.author.unwrap();
        assert_eq!(author.name, Some("Jane Doe".to_string()));
        assert_eq!(author.email, Some("jane@example.com".to_string()));
        assert_eq!(author.url, None);
    }

    #[test]
    fn test_parse_package_json_repository_string() {
        let content = r#"{
            "name": "test-plugin",
            "repository": "https://github.com/user/repo",
            "aleph": {}
        }"#;

        let manifest =
            parse_package_json_content(content, Path::new("/test/package.json")).unwrap();

        assert_eq!(
            manifest.repository,
            Some("https://github.com/user/repo".to_string())
        );
    }

    #[test]
    fn test_parse_package_json_repository_object() {
        let content = r#"{
            "name": "test-plugin",
            "repository": {
                "type": "git",
                "url": "https://github.com/user/repo.git"
            },
            "aleph": {}
        }"#;

        let manifest =
            parse_package_json_content(content, Path::new("/test/package.json")).unwrap();

        assert_eq!(
            manifest.repository,
            Some("https://github.com/user/repo.git".to_string())
        );
    }

    #[test]
    fn test_parse_package_json_missing_aleph() {
        let content = r#"{
            "name": "regular-package",
            "version": "1.0.0"
        }"#;

        let result = parse_package_json_content(content, Path::new("/test/package.json"));
        assert!(result.is_err());

        let err = result.unwrap_err();
        assert!(matches!(err, ExtensionError::InvalidManifest { .. }));
    }

    #[test]
    fn test_parse_package_json_missing_name() {
        let content = r#"{
            "aleph": {}
        }"#;

        let result = parse_package_json_content(content, Path::new("/test/package.json"));
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_package_json_config_schema() {
        let content = r#"{
            "name": "test-plugin",
            "aleph": {
                "configSchema": {
                    "type": "object",
                    "properties": {
                        "apiKey": { "type": "string" }
                    }
                },
                "configUiHints": {
                    "apiKey": {
                        "label": "API Key",
                        "sensitive": true
                    }
                }
            }
        }"#;

        let manifest =
            parse_package_json_content(content, Path::new("/test/package.json")).unwrap();

        assert!(manifest.config_schema.is_some());
        assert_eq!(manifest.config_ui_hints.len(), 1);

        let hint = manifest.config_ui_hints.get("apiKey").unwrap();
        assert_eq!(hint.label, Some("API Key".to_string()));
        assert_eq!(hint.sensitive, Some(true));
    }

    #[test]
    fn test_sanitize_npm_package_name() {
        // Test npm-specific sanitization (scope stripping)
        assert_eq!(sanitize_npm_package_name("my-plugin"), "my-plugin");
        assert_eq!(sanitize_npm_package_name("@org/my-plugin"), "my-plugin");
        assert_eq!(sanitize_npm_package_name("@scope/pkg-name"), "pkg-name");

        // These also test the underlying sanitize_plugin_id behavior
        assert_eq!(sanitize_npm_package_name("MyPlugin"), "myplugin");
        assert_eq!(sanitize_npm_package_name("my_plugin"), "my-plugin");
        assert_eq!(sanitize_npm_package_name("my--plugin"), "my-plugin");
        assert_eq!(sanitize_npm_package_name("-my-plugin-"), "my-plugin");
    }

    #[test]
    fn test_parse_package_json_scoped_package() {
        let content = r#"{
            "name": "@myorg/awesome-plugin",
            "version": "1.0.0",
            "aleph": {}
        }"#;

        let manifest =
            parse_package_json_content(content, Path::new("/test/package.json")).unwrap();

        // ID should be sanitized from scoped package name
        assert_eq!(manifest.id, "awesome-plugin");
        // Display name keeps the original
        assert_eq!(manifest.name, "@myorg/awesome-plugin");
    }

    use std::path::PathBuf;
}
