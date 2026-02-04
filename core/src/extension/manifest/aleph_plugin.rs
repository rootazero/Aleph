//! aleph.plugin.json parser for WASM and static plugins
//!
//! This module parses the native Aleph plugin manifest format used for
//! WASM plugins and static content plugins.

use super::types::{AuthorInfo, ConfigUiHint, PluginManifest, PluginPermission};
use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::types::PluginKind;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::Path;

// =============================================================================
// aleph.plugin.json Types
// =============================================================================

/// Aleph plugin manifest structure
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AlephPluginJson {
    /// Plugin ID (required)
    id: String,

    /// Display name (optional, defaults to ID)
    #[serde(default)]
    name: Option<String>,

    /// Plugin version (semver)
    #[serde(default)]
    version: Option<String>,

    /// Plugin description
    #[serde(default)]
    description: Option<String>,

    /// Plugin kind (wasm, static)
    #[serde(default)]
    kind: Option<PluginKind>,

    /// Entry point (relative to manifest)
    #[serde(default)]
    entry: Option<String>,

    /// Configuration schema (JSON Schema)
    #[serde(default)]
    config_schema: Option<JsonValue>,

    /// UI hints for configuration fields
    #[serde(default)]
    config_ui_hints: Option<HashMap<String, ConfigUiHint>>,

    /// Required permissions
    #[serde(default)]
    permissions: Option<Vec<PluginPermission>>,

    /// Author information
    #[serde(default)]
    author: Option<AlephPluginAuthor>,

    /// Homepage URL
    #[serde(default)]
    homepage: Option<String>,

    /// Repository URL
    #[serde(default)]
    repository: Option<String>,

    /// License (SPDX identifier)
    #[serde(default)]
    license: Option<String>,

    /// Search keywords
    #[serde(default)]
    keywords: Option<Vec<String>>,

    /// Supported file extensions (for static plugins)
    #[serde(default)]
    extensions: Option<Vec<String>>,
}

/// Author information in aleph.plugin.json
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum AlephPluginAuthor {
    /// Simple string format
    String(String),
    /// Object format
    Object {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        email: Option<String>,
        #[serde(default)]
        url: Option<String>,
    },
}

impl From<AlephPluginAuthor> for AuthorInfo {
    fn from(author: AlephPluginAuthor) -> Self {
        match author {
            AlephPluginAuthor::String(s) => AuthorInfo::from(s.as_str()),
            AlephPluginAuthor::Object { name, email, url } => AuthorInfo { name, email, url },
        }
    }
}

// =============================================================================
// Plugin ID Validation and Sanitization
// =============================================================================

/// Sanitize a name to a valid plugin ID.
///
/// Converts the name to lowercase, replaces invalid characters with hyphens,
/// removes consecutive hyphens, and trims leading/trailing hyphens.
///
/// # Arguments
/// * `name` - The name to sanitize
///
/// # Returns
/// A sanitized plugin ID string
///
/// # Example
/// ```
/// use alephcore::extension::manifest::sanitize_plugin_id;
///
/// assert_eq!(sanitize_plugin_id("My Plugin"), "my-plugin");
/// assert_eq!(sanitize_plugin_id("my_plugin"), "my-plugin");
/// assert_eq!(sanitize_plugin_id("Plugin 123"), "plugin-123");
/// ```
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

    // Remove consecutive hyphens
    while id.contains("--") {
        id = id.replace("--", "-");
    }

    // Remove leading/trailing hyphens
    id.trim_matches('-').to_string()
}

/// Validate a plugin ID
///
/// Plugin IDs must:
/// - Not be empty
/// - Start with a lowercase letter
/// - Contain only lowercase letters, numbers, and hyphens
/// - Not contain consecutive hyphens
/// - Not start or end with a hyphen
/// - Be at most 64 characters
///
/// # Arguments
/// * `id` - The plugin ID to validate
///
/// # Returns
/// * `Ok(())` if valid
/// * `Err(String)` with reason if invalid
pub fn validate_plugin_id(id: &str) -> Result<(), String> {
    // Check empty
    if id.is_empty() {
        return Err("Plugin ID cannot be empty".to_string());
    }

    // Check length
    if id.len() > 64 {
        return Err(format!(
            "Plugin ID too long ({} chars, max 64)",
            id.len()
        ));
    }

    // Check first character
    let first_char = id.chars().next().unwrap();
    if !first_char.is_ascii_lowercase() {
        return Err("Plugin ID must start with a lowercase letter".to_string());
    }

    // Check all characters
    for ch in id.chars() {
        if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() && ch != '-' {
            return Err(format!(
                "Invalid character '{}'. Only lowercase letters, numbers, and hyphens allowed",
                ch
            ));
        }
    }

    // Check for consecutive hyphens
    if id.contains("--") {
        return Err("Plugin ID cannot contain consecutive hyphens".to_string());
    }

    // Check for leading/trailing hyphens
    if id.starts_with('-') || id.ends_with('-') {
        return Err("Plugin ID cannot start or end with a hyphen".to_string());
    }

    Ok(())
}

// =============================================================================
// Parser
// =============================================================================

/// Parse an aleph.plugin.json file into a PluginManifest
///
/// # Arguments
/// * `path` - Path to the aleph.plugin.json file
///
/// # Returns
/// * `Ok(PluginManifest)` - Parsed manifest
/// * `Err(ExtensionError)` - If parsing fails or required fields are missing
pub async fn parse_aleph_plugin(path: &Path) -> ExtensionResult<PluginManifest> {
    let content = tokio::fs::read_to_string(path).await?;
    parse_aleph_plugin_content(&content, path)
}

/// Parse aleph.plugin.json content into a PluginManifest
///
/// This is the sync version for use in tests and when content is already loaded.
pub fn parse_aleph_plugin_content(content: &str, path: &Path) -> ExtensionResult<PluginManifest> {
    let plugin: AlephPluginJson = serde_json::from_str(content)
        .map_err(|e| ExtensionError::invalid_manifest(path, format!("JSON parse error: {}", e)))?;

    // Validate plugin ID
    if plugin.id.is_empty() {
        return Err(ExtensionError::missing_field(path, "id"));
    }

    validate_plugin_id(&plugin.id).map_err(|reason| {
        ExtensionError::invalid_plugin_name(&plugin.id, reason)
    })?;

    // Determine display name
    let name = plugin.name.unwrap_or_else(|| plugin.id.clone());

    // Determine plugin kind (default to Wasm)
    let kind = plugin.kind.unwrap_or(PluginKind::Wasm);

    // Determine entry point based on kind
    let entry = plugin.entry.unwrap_or_else(|| match kind {
        PluginKind::Wasm => "plugin.wasm".to_string(),
        PluginKind::NodeJs => "index.js".to_string(),
        PluginKind::Static => ".".to_string(),
    });

    // Build manifest
    let manifest = PluginManifest {
        id: plugin.id,
        name,
        version: plugin.version,
        description: plugin.description,
        kind,
        entry: entry.into(),
        root_dir: path.parent().map(|p| p.to_path_buf()).unwrap_or_default(),
        config_schema: plugin.config_schema,
        config_ui_hints: plugin.config_ui_hints.unwrap_or_default(),
        permissions: plugin.permissions.unwrap_or_default(),
        author: plugin.author.map(AuthorInfo::from),
        homepage: plugin.homepage,
        repository: plugin.repository,
        license: plugin.license,
        keywords: plugin.keywords.unwrap_or_default(),
        extensions: plugin.extensions.unwrap_or_default(),
        // V2 fields not available in JSON format
        tools_v2: None,
        hooks_v2: None,
        commands_v2: None,
        services_v2: None,
        prompt_v2: None,
        capabilities_v2: None,
        // P2 fields not available in JSON format
        channels_v2: None,
        providers_v2: None,
        http_routes_v2: None,
    };

    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_validate_plugin_id_valid() {
        assert!(validate_plugin_id("my-plugin").is_ok());
        assert!(validate_plugin_id("plugin123").is_ok());
        assert!(validate_plugin_id("a").is_ok());
        assert!(validate_plugin_id("my-awesome-plugin").is_ok());
        assert!(validate_plugin_id("plugin-1-2-3").is_ok());
    }

    #[test]
    fn test_validate_plugin_id_invalid_empty() {
        let err = validate_plugin_id("").unwrap_err();
        assert!(err.contains("empty"));
    }

    #[test]
    fn test_validate_plugin_id_invalid_uppercase() {
        let err = validate_plugin_id("MyPlugin").unwrap_err();
        assert!(err.contains("lowercase"));
    }

    #[test]
    fn test_validate_plugin_id_invalid_start_number() {
        let err = validate_plugin_id("123plugin").unwrap_err();
        assert!(err.contains("lowercase letter"));
    }

    #[test]
    fn test_validate_plugin_id_invalid_underscore() {
        let err = validate_plugin_id("my_plugin").unwrap_err();
        assert!(err.contains("Invalid character"));
    }

    #[test]
    fn test_validate_plugin_id_invalid_double_hyphen() {
        let err = validate_plugin_id("my--plugin").unwrap_err();
        assert!(err.contains("consecutive"));
    }

    #[test]
    fn test_validate_plugin_id_invalid_leading_hyphen() {
        let err = validate_plugin_id("-plugin").unwrap_err();
        // Leading hyphen is caught by "must start with lowercase letter" check first
        assert!(
            err.contains("start or end") || err.contains("lowercase letter"),
            "Expected error about leading hyphen, got: {}",
            err
        );
    }

    #[test]
    fn test_validate_plugin_id_invalid_trailing_hyphen() {
        let err = validate_plugin_id("plugin-").unwrap_err();
        assert!(err.contains("start or end"));
    }

    #[test]
    fn test_validate_plugin_id_too_long() {
        let long_id = "a".repeat(65);
        let err = validate_plugin_id(&long_id).unwrap_err();
        assert!(err.contains("too long"));
    }

    // =========================================================================
    // sanitize_plugin_id tests
    // =========================================================================

    #[test]
    fn test_sanitize_plugin_id_basic() {
        assert_eq!(sanitize_plugin_id("my-plugin"), "my-plugin");
        assert_eq!(sanitize_plugin_id("plugin123"), "plugin123");
    }

    #[test]
    fn test_sanitize_plugin_id_uppercase() {
        assert_eq!(sanitize_plugin_id("MyPlugin"), "myplugin");
        assert_eq!(sanitize_plugin_id("MY-PLUGIN"), "my-plugin");
    }

    #[test]
    fn test_sanitize_plugin_id_spaces() {
        assert_eq!(sanitize_plugin_id("My Plugin"), "my-plugin");
        assert_eq!(sanitize_plugin_id("my plugin name"), "my-plugin-name");
    }

    #[test]
    fn test_sanitize_plugin_id_underscores() {
        assert_eq!(sanitize_plugin_id("my_plugin"), "my-plugin");
        assert_eq!(sanitize_plugin_id("my_awesome_plugin"), "my-awesome-plugin");
    }

    #[test]
    fn test_sanitize_plugin_id_consecutive_hyphens() {
        assert_eq!(sanitize_plugin_id("my--plugin"), "my-plugin");
        assert_eq!(sanitize_plugin_id("my---plugin"), "my-plugin");
    }

    #[test]
    fn test_sanitize_plugin_id_leading_trailing() {
        assert_eq!(sanitize_plugin_id("-my-plugin"), "my-plugin");
        assert_eq!(sanitize_plugin_id("my-plugin-"), "my-plugin");
        assert_eq!(sanitize_plugin_id("-my-plugin-"), "my-plugin");
    }

    #[test]
    fn test_sanitize_plugin_id_special_chars() {
        assert_eq!(sanitize_plugin_id("my@plugin"), "my-plugin");
        assert_eq!(sanitize_plugin_id("my!plugin"), "my-plugin");
        assert_eq!(sanitize_plugin_id("Plugin 123"), "plugin-123");
    }

    // =========================================================================
    // parse_aleph_plugin tests
    // =========================================================================

    #[test]
    fn test_parse_aleph_plugin_basic() {
        let content = r#"{
            "id": "my-plugin",
            "version": "1.0.0",
            "description": "A test plugin"
        }"#;

        let manifest =
            parse_aleph_plugin_content(content, Path::new("/test/aleph.plugin.json")).unwrap();

        assert_eq!(manifest.id, "my-plugin");
        assert_eq!(manifest.name, "my-plugin");
        assert_eq!(manifest.version, Some("1.0.0".to_string()));
        assert_eq!(manifest.description, Some("A test plugin".to_string()));
        assert_eq!(manifest.kind, PluginKind::Wasm);
        assert_eq!(manifest.entry, PathBuf::from("plugin.wasm"));
    }

    #[test]
    fn test_parse_aleph_plugin_with_name() {
        let content = r#"{
            "id": "my-plugin",
            "name": "My Awesome Plugin"
        }"#;

        let manifest =
            parse_aleph_plugin_content(content, Path::new("/test/aleph.plugin.json")).unwrap();

        assert_eq!(manifest.id, "my-plugin");
        assert_eq!(manifest.name, "My Awesome Plugin");
    }

    #[test]
    fn test_parse_aleph_plugin_static_kind() {
        let content = r#"{
            "id": "my-static-plugin",
            "kind": "static",
            "extensions": [".md", ".txt"]
        }"#;

        let manifest =
            parse_aleph_plugin_content(content, Path::new("/test/aleph.plugin.json")).unwrap();

        assert_eq!(manifest.kind, PluginKind::Static);
        assert_eq!(manifest.entry, PathBuf::from("."));
        assert_eq!(manifest.extensions, vec![".md", ".txt"]);
    }

    #[test]
    fn test_parse_aleph_plugin_nodejs_kind() {
        let content = r#"{
            "id": "my-nodejs-plugin",
            "kind": "nodejs"
        }"#;

        let manifest =
            parse_aleph_plugin_content(content, Path::new("/test/aleph.plugin.json")).unwrap();

        assert_eq!(manifest.kind, PluginKind::NodeJs);
        assert_eq!(manifest.entry, PathBuf::from("index.js"));
    }

    #[test]
    fn test_parse_aleph_plugin_custom_entry() {
        let content = r#"{
            "id": "my-plugin",
            "entry": "dist/main.wasm"
        }"#;

        let manifest =
            parse_aleph_plugin_content(content, Path::new("/test/aleph.plugin.json")).unwrap();

        assert_eq!(manifest.entry, PathBuf::from("dist/main.wasm"));
    }

    #[test]
    fn test_parse_aleph_plugin_with_permissions() {
        let content = r#"{
            "id": "my-plugin",
            "permissions": ["network", "filesystem:read", "env"]
        }"#;

        let manifest =
            parse_aleph_plugin_content(content, Path::new("/test/aleph.plugin.json")).unwrap();

        assert_eq!(manifest.permissions.len(), 3);
        assert!(manifest.permissions.contains(&PluginPermission::Network));
        assert!(manifest
            .permissions
            .contains(&PluginPermission::FilesystemRead));
        assert!(manifest.permissions.contains(&PluginPermission::Env));
    }

    #[test]
    fn test_parse_aleph_plugin_with_author_string() {
        let content = r#"{
            "id": "my-plugin",
            "author": "John Doe <john@example.com>"
        }"#;

        let manifest =
            parse_aleph_plugin_content(content, Path::new("/test/aleph.plugin.json")).unwrap();

        let author = manifest.author.unwrap();
        assert_eq!(author.name, Some("John Doe".to_string()));
        assert_eq!(author.email, Some("john@example.com".to_string()));
    }

    #[test]
    fn test_parse_aleph_plugin_with_author_object() {
        let content = r#"{
            "id": "my-plugin",
            "author": {
                "name": "Jane Doe",
                "email": "jane@example.com",
                "url": "https://example.com"
            }
        }"#;

        let manifest =
            parse_aleph_plugin_content(content, Path::new("/test/aleph.plugin.json")).unwrap();

        let author = manifest.author.unwrap();
        assert_eq!(author.name, Some("Jane Doe".to_string()));
        assert_eq!(author.email, Some("jane@example.com".to_string()));
        assert_eq!(author.url, Some("https://example.com".to_string()));
    }

    #[test]
    fn test_parse_aleph_plugin_with_config() {
        let content = r#"{
            "id": "my-plugin",
            "configSchema": {
                "type": "object",
                "properties": {
                    "apiKey": { "type": "string" }
                }
            },
            "configUiHints": {
                "apiKey": {
                    "label": "API Key",
                    "help": "Your API key for the service",
                    "sensitive": true
                }
            }
        }"#;

        let manifest =
            parse_aleph_plugin_content(content, Path::new("/test/aleph.plugin.json")).unwrap();

        assert!(manifest.config_schema.is_some());
        assert!(manifest.has_config());

        let hint = manifest.config_ui_hints.get("apiKey").unwrap();
        assert_eq!(hint.label, Some("API Key".to_string()));
        assert_eq!(hint.help, Some("Your API key for the service".to_string()));
        assert_eq!(hint.sensitive, Some(true));
    }

    #[test]
    fn test_parse_aleph_plugin_missing_id() {
        let content = r#"{
            "name": "My Plugin"
        }"#;

        let result = parse_aleph_plugin_content(content, Path::new("/test/aleph.plugin.json"));
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_aleph_plugin_invalid_id() {
        let content = r#"{
            "id": "My-Plugin"
        }"#;

        let result = parse_aleph_plugin_content(content, Path::new("/test/aleph.plugin.json"));
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_aleph_plugin_full() {
        let content = r#"{
            "id": "complete-plugin",
            "name": "Complete Plugin",
            "version": "2.0.0",
            "description": "A fully specified plugin",
            "kind": "wasm",
            "entry": "dist/plugin.wasm",
            "homepage": "https://example.com",
            "repository": "https://github.com/user/repo",
            "license": "MIT",
            "keywords": ["test", "example"],
            "author": {
                "name": "Test Author"
            }
        }"#;

        let manifest =
            parse_aleph_plugin_content(content, Path::new("/test/aleph.plugin.json")).unwrap();

        assert_eq!(manifest.id, "complete-plugin");
        assert_eq!(manifest.name, "Complete Plugin");
        assert_eq!(manifest.version, Some("2.0.0".to_string()));
        assert_eq!(
            manifest.description,
            Some("A fully specified plugin".to_string())
        );
        assert_eq!(manifest.kind, PluginKind::Wasm);
        assert_eq!(manifest.entry, PathBuf::from("dist/plugin.wasm"));
        assert_eq!(manifest.homepage, Some("https://example.com".to_string()));
        assert_eq!(
            manifest.repository,
            Some("https://github.com/user/repo".to_string())
        );
        assert_eq!(manifest.license, Some("MIT".to_string()));
        assert_eq!(manifest.keywords, vec!["test", "example"]);
        assert!(manifest.author.is_some());
    }
}
