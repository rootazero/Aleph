//! CC-format JSON manifest parser (.claude-plugin/plugin.json)
//!
//! This parser handles Claude Code's native JSON format — read-only compatibility
//! for third-party CC plugins. The `[aleph]` section, if present, enables
//! Aleph-specific features while remaining invisible to Claude Code.

use std::path::Path;

use serde::Deserialize;
use serde_json::Value as JsonValue;

use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::manifest::types::{
    AlephExtensions, AlephRuntime, AuthorInfo, PluginManifest,
};
use crate::extension::manifest::{sanitize_plugin_id, validate_plugin_id};
use crate::extension::types::PluginKind;

/// Filename path for CC-format JSON manifest
pub const CC_PLUGIN_JSON: &str = ".claude-plugin/plugin.json";

// =============================================================================
// CC plugin.json Types
// =============================================================================

/// Root structure for .claude-plugin/plugin.json
///
/// Uses camelCase field names to match Claude Code's conventions.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[derive(Default)]
pub struct CcPluginJson {
    /// Plugin name (required — used as plugin ID source)
    pub name: Option<String>,

    /// Plugin version (semver)
    pub version: Option<String>,

    /// Plugin description
    pub description: Option<String>,

    /// License identifier (SPDX)
    pub license: Option<String>,

    /// Keywords for search
    pub keywords: Option<Vec<String>>,

    /// Homepage URL
    pub homepage: Option<String>,

    /// Repository information (string URL or object)
    pub repository: Option<CcPluginRepository>,

    /// Author information (string or object)
    pub author: Option<CcPluginAuthor>,

    // CC-native component path fields (camelCase)
    /// Path to skills directory
    pub skills: Option<String>,

    /// Path to commands directory (user-triggered `/command`s)
    pub commands: Option<String>,

    /// Path to agents directory
    pub agents: Option<String>,

    /// Path to hooks file/directory
    pub hooks: Option<String>,

    /// Path to MCP servers configuration
    pub mcp_servers: Option<String>,

    /// Aleph-specific extensions (optional, ignored by Claude Code)
    pub aleph: Option<JsonValue>,

    /// P3.5 — lazy activation hints. Plugin author can declare which
    /// surfaces they serve so `ActivationPlanner` doesn't have to load
    /// every plugin eagerly at boot. Mirrors openclaw's `activation`
    /// block. Optional field; absence = legacy "always load".
    #[serde(default)]
    pub activation: Option<crate::extension::activation::ActivationHints>,
}

/// Author in CC plugin.json — either a plain string or an object
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum CcPluginAuthor {
    /// Simple string format: "Name <email> (url)"
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

impl From<CcPluginAuthor> for AuthorInfo {
    fn from(author: CcPluginAuthor) -> Self {
        match author {
            CcPluginAuthor::String(s) => Self::from(s.as_str()),
            CcPluginAuthor::Object { name, email, url } => Self { name, email, url },
        }
    }
}

/// Repository in CC plugin.json — either a string URL or an object with `url`
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum CcPluginRepository {
    /// Plain URL string
    Url(String),
    /// Object with url field
    Object {
        #[serde(default)]
        url: Option<String>,
    },
}

impl CcPluginRepository {
    fn into_url(self) -> Option<String> {
        match self {
            Self::Url(url) => Some(url),
            Self::Object { url } => url,
        }
    }
}

// =============================================================================
// Runtime → PluginKind mapping
// =============================================================================

fn runtime_to_kind(runtime: &str) -> PluginKind {
    match runtime {
        "wasm" => PluginKind::Wasm,
        "mcp" => PluginKind::Mcp,
        "static" => PluginKind::Static,
        _ => PluginKind::Static,
    }
}

fn default_entry_for_kind(kind: PluginKind) -> String {
    match kind {
        PluginKind::Wasm => "plugin.wasm".to_string(),
        PluginKind::Mcp => ".mcp.json".to_string(),
        PluginKind::Static => ".".to_string(),
    }
}

// =============================================================================
// Parser
// =============================================================================

/// Parse .claude-plugin/plugin.json content into a `PluginManifest`
///
/// # Arguments
/// * `content` - JSON content string
/// * `plugin_dir` - Path to the plugin root directory
pub fn parse_cc_plugin_json_content(
    content: &str,
    plugin_dir: &Path,
) -> ExtensionResult<PluginManifest> {
    let manifest_path = plugin_dir.join(CC_PLUGIN_JSON);

    let json: CcPluginJson = serde_json::from_str(content).map_err(|e| {
        ExtensionError::invalid_manifest(&manifest_path, format!("JSON parse error: {e}"))
    })?;

    // `name` is required
    let raw_name = json
        .name
        .ok_or_else(|| ExtensionError::missing_field(&manifest_path, "name"))?;
    if raw_name.is_empty() {
        return Err(ExtensionError::missing_field(&manifest_path, "name"));
    }

    // Derive plugin ID from name
    let plugin_id = sanitize_plugin_id(&raw_name);
    validate_plugin_id(&plugin_id)
        .map_err(|reason| ExtensionError::invalid_plugin_name(&raw_name, reason))?;

    // Parse [aleph] section from raw JSON value
    let (kind, entry, aleph_ext, permissions) = if let Some(aleph_val) = json.aleph {
        let aleph_ext: AlephExtensions = serde_json::from_value(aleph_val).map_err(|e| {
            ExtensionError::invalid_manifest(
                &manifest_path,
                format!("Invalid [aleph] section: {e}"),
            )
        })?;

        let runtime_str = match aleph_ext.runtime {
            AlephRuntime::Wasm => "wasm",
            AlephRuntime::Mcp => "mcp",
            AlephRuntime::Static => "static",
        };
        let kind = runtime_to_kind(runtime_str);
        let entry = aleph_ext
            .entry
            .clone()
            .unwrap_or_else(|| default_entry_for_kind(kind));

        // Permissions are stored inside AlephExtensions.permissions (PermissionsSection)
        let permissions = aleph_ext
            .permissions
            .as_ref()
            .map(|p| {
                use crate::extension::manifest::toml_types::convert_permissions;
                convert_permissions(p)
            })
            .unwrap_or_default();

        (kind, entry, Some(aleph_ext), permissions)
    } else {
        (PluginKind::Static, ".".to_string(), None, Vec::new())
    };

    let repository = json.repository.and_then(|r| r.into_url());
    let wasm_capabilities = aleph_ext
        .as_ref()
        .and_then(|ext| ext.capabilities.as_ref())
        .and_then(crate::extension::manifest::toml_types::convert_wasm_capabilities);

    let manifest = PluginManifest {
        id: plugin_id,
        name: raw_name,
        version: json.version,
        description: json.description,
        kind,
        entry: entry.into(),
        root_dir: plugin_dir.to_path_buf(),
        config_schema: None,
        config_ui_hints: Default::default(),
        permissions,
        author: json.author.map(AuthorInfo::from),
        homepage: json.homepage,
        repository,
        license: json.license,
        keywords: json.keywords.unwrap_or_default(),
        extensions: Vec::new(),
        // V2 fields not available in CC JSON format
        tools_v2: None,
        hooks_v2: None,
        commands_v2: None,
        services_v2: None,
        prompt_v2: None,
        capabilities_v2: None,
        wasm_capabilities,
        wasm_resource_limits: None,
        // CC-compat extensions
        aleph_extensions: aleph_ext,
        // Memory extension manifest — not available in CC JSON format
        memory_manifest: None,
        activation: None,
    };

    Ok(manifest)
}

/// Parse .claude-plugin/plugin.json from a plugin directory (sync)
///
/// # Arguments
/// * `dir` - Path to the plugin root directory
pub fn parse_cc_plugin_json_sync(dir: &Path) -> ExtensionResult<PluginManifest> {
    let json_path = dir.join(CC_PLUGIN_JSON);
    let content = std::fs::read_to_string(&json_path)?;
    parse_cc_plugin_json_content(&content, dir)
}

/// Parse .claude-plugin/plugin.json from a plugin directory (async)
///
/// # Arguments
/// * `dir` - Path to the plugin root directory
pub async fn parse_cc_plugin_json(dir: &Path) -> ExtensionResult<PluginManifest> {
    let json_path = dir.join(CC_PLUGIN_JSON);
    let content = tokio::fs::read_to_string(&json_path).await?;
    parse_cc_plugin_json_content(&content, dir)
}

// =============================================================================
// ManifestAdapter implementation
// =============================================================================

use super::adapter::{AdapterOutput, ManifestAdapter};
use super::parsers;
use crate::extension::capability::{CapabilitySource, SourceFormat};
use crate::extension::types::PluginOrigin;

/// `ManifestAdapter` for `.claude-plugin/plugin.json` format.
///
/// Priority 90 — JSON is tried after TOML.
pub struct ClaudeCodeJsonAdapter;

impl ManifestAdapter for ClaudeCodeJsonAdapter {
    fn detect(&self, plugin_dir: &Path) -> bool {
        plugin_dir.join(CC_PLUGIN_JSON).exists()
    }

    fn parse(&self, plugin_dir: &Path) -> anyhow::Result<AdapterOutput> {
        let manifest = parse_cc_plugin_json_sync(plugin_dir)
            .map_err(|e| anyhow::anyhow!("CC JSON parse error: {e}"))?;

        let plugin_id = manifest.id.clone();
        let mut capabilities = Vec::new();

        // Re-parse the raw JSON to get component paths
        let json_path = plugin_dir.join(CC_PLUGIN_JSON);
        let content = std::fs::read_to_string(&json_path)?;
        let raw: CcPluginJson = serde_json::from_str(&content)
            .map_err(|e| anyhow::anyhow!("JSON re-parse error: {e}"))?;

        // Parse skills
        let skills_rel = raw.skills.as_deref().unwrap_or("skills");
        capabilities.extend(parsers::parse_skills_dir(
            plugin_dir, skills_rel, &plugin_id,
        )?);

        // Parse commands (user-triggered /commands → Command-typed skills)
        let commands_rel = raw.commands.as_deref().unwrap_or("commands");
        capabilities.extend(parsers::parse_commands_dir(
            plugin_dir,
            commands_rel,
            &plugin_id,
        )?);

        // Parse agents
        let agents_rel = raw.agents.as_deref().unwrap_or("agents");
        capabilities.extend(parsers::parse_agents_dir(
            plugin_dir, agents_rel, &plugin_id,
        )?);

        // Parse hooks
        let hooks_rel = raw.hooks.as_deref().unwrap_or("hooks/hooks.json");
        capabilities.extend(parsers::parse_hooks_file(
            plugin_dir, hooks_rel, &plugin_id,
        )?);

        // Parse MCP servers
        let mcp_rel = raw.mcp_servers.as_deref().unwrap_or(".mcp.json");
        capabilities.extend(parsers::parse_mcp_config_file(
            plugin_dir, mcp_rel, &plugin_id,
        )?);

        Ok(AdapterOutput {
            plugin_id: plugin_id.clone(),
            name: Some(manifest.name),
            version: manifest.version,
            description: manifest.description,
            capabilities,
            source: CapabilitySource {
                plugin_id,
                origin: PluginOrigin::Global,
                format: SourceFormat::ClaudeCode,
            },
            permissions: manifest.permissions,
        })
    }

    fn format_name(&self) -> &str {
        "Claude Code (JSON)"
    }

    fn priority(&self) -> i32 {
        90
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_dir() -> PathBuf {
        PathBuf::from("/test/my-plugin")
    }

    #[test]
    fn test_parse_minimal_cc_json() {
        let content = r#"{"name": "My Plugin"}"#;
        let manifest = parse_cc_plugin_json_content(content, &test_dir()).unwrap();

        assert_eq!(manifest.id, "my-plugin");
        assert_eq!(manifest.name, "My Plugin");
        assert_eq!(manifest.kind, PluginKind::Static);
        assert!(manifest.aleph_extensions.is_none());
    }

    #[test]
    fn test_parse_cc_json_with_camel_case() {
        let content = r#"{
            "name": "camel-case-plugin",
            "version": "1.2.3",
            "skills": "skills/",
            "agents": "agents/",
            "mcpServers": "mcp-servers.json"
        }"#;
        let manifest = parse_cc_plugin_json_content(content, &test_dir()).unwrap();

        assert_eq!(manifest.id, "camel-case-plugin");
        assert_eq!(manifest.version, Some("1.2.3".to_string()));
        // Component paths are stored in raw JSON struct, not in PluginManifest directly
        // Just verify the manifest parsed correctly
        assert_eq!(manifest.kind, PluginKind::Static);
    }

    #[test]
    fn test_parse_cc_json_with_aleph_section() {
        let content = r#"{
            "name": "wasm-plugin",
            "version": "2.0.0",
            "aleph": {
                "runtime": "wasm",
                "entry": "dist/plugin.wasm"
            }
        }"#;
        let manifest = parse_cc_plugin_json_content(content, &test_dir()).unwrap();

        assert_eq!(manifest.id, "wasm-plugin");
        assert_eq!(manifest.kind, PluginKind::Wasm);
        assert_eq!(manifest.entry, PathBuf::from("dist/plugin.wasm"));

        let ext = manifest.aleph_extensions.unwrap();
        assert_eq!(ext.runtime, AlephRuntime::Wasm);
        assert_eq!(ext.entry, Some("dist/plugin.wasm".to_string()));
    }

    #[test]
    fn test_cc_json_name_required() {
        // No name field
        let result = parse_cc_plugin_json_content(r#"{"version": "1.0.0"}"#, &test_dir());
        assert!(result.is_err());

        // Empty name field
        let result = parse_cc_plugin_json_content(r#"{"name": ""}"#, &test_dir());
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_cc_json_author_string() {
        let content = r#"{
            "name": "author-test",
            "author": "Bob <bob@example.com>"
        }"#;
        let manifest = parse_cc_plugin_json_content(content, &test_dir()).unwrap();
        let author = manifest.author.unwrap();
        assert_eq!(author.name, Some("Bob".to_string()));
        assert_eq!(author.email, Some("bob@example.com".to_string()));
    }

    #[test]
    fn test_parse_cc_json_author_object() {
        let content = r#"{
            "name": "author-obj-test",
            "author": {
                "name": "Carol",
                "email": "carol@example.com",
                "url": "https://carol.dev"
            }
        }"#;
        let manifest = parse_cc_plugin_json_content(content, &test_dir()).unwrap();
        let author = manifest.author.unwrap();
        assert_eq!(author.name, Some("Carol".to_string()));
        assert_eq!(author.url, Some("https://carol.dev".to_string()));
    }

    #[test]
    fn test_parse_cc_json_repository_string() {
        let content = r#"{
            "name": "repo-test",
            "repository": "https://github.com/user/repo"
        }"#;
        let manifest = parse_cc_plugin_json_content(content, &test_dir()).unwrap();
        assert_eq!(
            manifest.repository,
            Some("https://github.com/user/repo".to_string())
        );
    }

    #[test]
    fn test_parse_cc_json_repository_object() {
        let content = r#"{
            "name": "repo-obj-test",
            "repository": {"url": "https://github.com/user/repo"}
        }"#;
        let manifest = parse_cc_plugin_json_content(content, &test_dir()).unwrap();
        assert_eq!(
            manifest.repository,
            Some("https://github.com/user/repo".to_string())
        );
    }

    #[test]
    fn test_parse_cc_json_mcp_runtime() {
        let content = r#"{
            "name": "mcp-plugin",
            "aleph": {"runtime": "mcp"}
        }"#;
        let manifest = parse_cc_plugin_json_content(content, &test_dir()).unwrap();
        assert_eq!(manifest.kind, PluginKind::Mcp);
        assert_eq!(manifest.entry, PathBuf::from(".mcp.json"));
    }

    #[test]
    fn test_parse_cc_json_root_dir_set() {
        let content = r#"{"name": "root-test"}"#;
        let dir = PathBuf::from("/plugins/root-test");
        let manifest = parse_cc_plugin_json_content(content, &dir).unwrap();
        assert_eq!(manifest.root_dir, dir);
    }
}
