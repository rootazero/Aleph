//! CC-format JSON manifest parser (.claude-plugin/plugin.json)
//!
//! This parser handles Claude Code's native JSON format — read-only compatibility
//! for third-party CC plugins. The `[aleph]` section, if present, enables
//! Aleph-specific features while remaining invisible to Claude Code.

use std::path::Path;

use serde::Deserialize;
use serde_json::Value as JsonValue;

use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::manifest::component_source::{self, ComponentSource};
use crate::extension::manifest::declared_sections::AlephSuperset;
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

    // CC-native component fields (camelCase). Each accepts a path, an array
    // of paths, or an inlined object — see `component_source`.
    /// Skills directory (or directories).
    pub skills: Option<ComponentSource>,

    /// Commands directory (user-triggered `/command`s).
    pub commands: Option<ComponentSource>,

    /// Agents directory (or directories).
    pub agents: Option<ComponentSource>,

    /// Hooks file, files, or inlined hook configuration.
    pub hooks: Option<ComponentSource>,

    /// MCP servers configuration file, files, or inlined server map.
    pub mcp_servers: Option<ComponentSource>,

    /// Aleph-specific extensions (optional, ignored by Claude Code)
    pub aleph: Option<JsonValue>,
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
    let mut superset = AlephSuperset::default();
    let (kind, entry, aleph_ext, permissions) = if let Some(aleph_val) = json.aleph {
        // The superset reads the same object. Deserializing twice rather than
        // widening `AlephExtensions` keeps the runtime type free of parse-only
        // sections, and unknown keys are ignored by both, so neither view can
        // reject a manifest the other accepts.
        superset = serde_json::from_value(aleph_val.clone()).map_err(|e| {
            ExtensionError::invalid_manifest(
                &manifest_path,
                format!("Invalid [aleph] superset section: {e}"),
            )
        })?;
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
        config_schema: superset.config_schema,
        config_ui_hints: superset.config_ui_hints,
        permissions,
        author: json.author.map(AuthorInfo::from),
        homepage: json.homepage,
        repository,
        license: json.license,
        keywords: json.keywords.unwrap_or_default(),
        extensions: Vec::new(),
        // Superset sections, carried in the `aleph` object (which Claude Code
        // ignores). `services_v2` stays `None`: services round-trip through
        // `aleph_extensions.services`, and a second copy would be a second
        // answer to "what services did this plugin declare".
        tools_v2: AlephSuperset::non_empty(superset.tools),
        hooks_v2: AlephSuperset::non_empty(superset.hooks),
        commands_v2: AlephSuperset::non_empty(superset.commands),
        services_v2: None,
        prompt_v2: superset.prompt,
        capabilities_v2: None,
        wasm_capabilities,
        wasm_resource_limits: None,
        // CC-compat extensions
        aleph_extensions: aleph_ext,
        memory_manifest: superset.memory,
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

        // Component fields accept a path, an array of paths, or (for hooks and
        // mcpServers) an inlined object — see `component_source`.
        capabilities.extend(component_source::resolve_dirs(
            raw.skills.as_ref(),
            "skills",
            plugin_dir,
            &plugin_id,
            "skills",
            parsers::parse_skills_dir,
        )?);
        capabilities.extend(component_source::resolve_dirs(
            raw.commands.as_ref(),
            "commands",
            plugin_dir,
            &plugin_id,
            "commands",
            parsers::parse_commands_dir,
        )?);
        capabilities.extend(component_source::resolve_dirs(
            raw.agents.as_ref(),
            "agents",
            plugin_dir,
            &plugin_id,
            "agents",
            parsers::parse_agents_dir,
        )?);
        capabilities.extend(component_source::resolve_hooks(
            raw.hooks.as_ref(),
            plugin_dir,
            &plugin_id,
        )?);
        capabilities.extend(component_source::resolve_mcp_servers(
            raw.mcp_servers.as_ref(),
            plugin_dir,
            &plugin_id,
        )?);

        // Manifest-declared sections from the `aleph` object — the same
        // translation both the native and CC-TOML dialects use.
        let no_services: Vec<crate::extension::manifest::ServiceSection> = Vec::new();
        capabilities.extend(
            crate::extension::manifest::declared_sections::declared_capabilities(
                plugin_dir,
                &plugin_id,
                &crate::extension::manifest::declared_sections::DeclaredSections {
                    prompt: manifest.prompt_v2.as_ref(),
                    tools: manifest.tools_v2.as_deref().unwrap_or(&[]),
                    hooks: manifest.hooks_v2.as_deref().unwrap_or(&[]),
                    commands: manifest.commands_v2.as_deref().unwrap_or(&[]),
                    services: manifest
                        .aleph_extensions
                        .as_ref()
                        .map_or(&no_services, |ext| &ext.services),
                },
                &manifest.permissions,
            ),
        );

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

    /// The two CC dialects must mean the same thing by the `aleph` superset.
    ///
    /// They necessarily have two deserialization structs — `AlephExtensionsToml`
    /// with typed TOML sections, and a `serde_json::from_value` into
    /// `AlephSuperset` — so nothing structural forces them to agree.
    /// `serde(flatten)` would not fix it either: it is unsafe for TOML
    /// arrays-of-tables. This test is the thing that holds them together.
    #[test]
    fn both_cc_dialects_agree_on_the_superset() {
        let dir = tempfile::tempdir().unwrap();

        let from_json = parse_cc_plugin_json_content(
            r#"{
              "name": "twin",
              "aleph": {
                "runtime": "wasm",
                "tools": [{"name": "t", "handler": "h"}],
                "commands": [{"name": "c", "handler": "h"}],
                "prompt": {"file": "SYSTEM.md", "scope": "system"},
                "config_schema": {"type": "object"},
                "config_ui_hints": {"k": {"label": "K"}}
              }
            }"#,
            dir.path(),
        )
        .unwrap();

        let from_toml = crate::extension::manifest::cc_plugin_toml::parse_cc_plugin_toml_content(
            r#"
name = "twin"

[aleph]
runtime = "wasm"

[[aleph.tools]]
name = "t"
handler = "h"

[[aleph.commands]]
name = "c"
handler = "h"

[aleph.prompt]
file = "SYSTEM.md"
scope = "system"

[aleph.config_schema]
type = "object"

[aleph.config_ui_hints.k]
label = "K"
"#,
            dir.path(),
        )
        .unwrap();

        assert_eq!(from_json.kind, from_toml.kind);
        assert_eq!(
            from_json.tools_v2.as_ref().map(|t| t
                .iter()
                .map(|x| (x.name.clone(), x.handler.clone()))
                .collect::<Vec<_>>()),
            from_toml.tools_v2.as_ref().map(|t| t
                .iter()
                .map(|x| (x.name.clone(), x.handler.clone()))
                .collect::<Vec<_>>()),
        );
        assert_eq!(
            from_json.commands_v2.as_ref().map(Vec::len),
            from_toml.commands_v2.as_ref().map(Vec::len)
        );
        assert_eq!(
            from_json.prompt_v2.as_ref().map(|p| p.file.clone()),
            from_toml.prompt_v2.as_ref().map(|p| p.file.clone())
        );
        assert_eq!(from_json.config_schema, from_toml.config_schema);
        assert_eq!(
            from_json.config_ui_hints.keys().collect::<Vec<_>>(),
            from_toml.config_ui_hints.keys().collect::<Vec<_>>()
        );
    }

    /// camelCase is Claude Code's convention, so a JSON author writing
    /// `configSchema` must not silently lose the schema.
    #[test]
    fn the_json_superset_accepts_camel_case_keys() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = parse_cc_plugin_json_content(
            r#"{"name":"camel","aleph":{"configSchema":{"type":"object"},
                "configUiHints":{"k":{"label":"K"}}}}"#,
            dir.path(),
        )
        .unwrap();
        assert!(manifest.config_schema.is_some());
        assert!(manifest.config_ui_hints.contains_key("k"));
    }

    /// Two of Anthropic's own plugin manifests inline `mcpServers`. Aleph
    /// declared it `Option<String>`, and serde fails the *whole* struct on a
    /// type mismatch — so those plugins were registered with
    /// `PluginStatus::Error` and zero capabilities. Loud, but all-or-nothing.
    #[test]
    fn an_inline_mcp_servers_object_no_longer_rejects_the_whole_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = parse_cc_plugin_json_content(
            r#"{
              "name": "chrome-devtools-mcp",
              "version": "1.0.0",
              "mcpServers": {
                "chrome-devtools": {"command": "npx", "args": ["-y", "chrome-devtools-mcp"]}
              }
            }"#,
            dir.path(),
        );
        assert!(
            manifest.is_ok(),
            "inline mcpServers must not reject the manifest: {:?}",
            manifest.err()
        );

        // The oracle for why the field is a union: the shape it replaced
        // cannot read this manifest at all.
        #[derive(serde::Deserialize)]
        struct OldShape {
            #[allow(dead_code)]
            name: String,
            #[serde(rename = "mcpServers")]
            #[allow(dead_code)]
            mcp_servers: Option<String>,
        }
        assert!(
            serde_json::from_str::<OldShape>(
                r#"{"name":"x","mcpServers":{"s":{"command":"npx"}}}"#
            )
            .is_err(),
            "if `Option<String>` parses this, the union is no longer load-bearing"
        );
    }

    /// An array of component paths is the other shape Claude Code accepts.
    #[test]
    fn an_array_of_component_paths_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        assert!(parse_cc_plugin_json_content(
            r#"{"name":"multi","skills":["./a/skills","./b/skills"]}"#,
            dir.path(),
        )
        .is_ok());
    }

    /// The inline arm must reach a parser, not merely deserialize — otherwise
    /// widening the type trades a loud rejection for a silent zero-capability
    /// load, which is worse.
    #[test]
    fn an_inline_mcp_servers_object_actually_registers_a_server() {
        use crate::extension::capability::CapabilityDeclaration;
        use crate::extension::manifest::adapter::ManifestAdapter;

        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".claude-plugin")).unwrap();
        std::fs::write(
            dir.path().join(CC_PLUGIN_JSON),
            r#"{
              "name": "inline-mcp",
              "mcpServers": {"srv": {"command": "node", "args": ["server.js"]}}
            }"#,
        )
        .unwrap();

        let out = ClaudeCodeJsonAdapter.parse(dir.path()).unwrap();
        assert!(
            out.capabilities
                .iter()
                .any(|c| matches!(c, CapabilityDeclaration::McpServer(_))),
            "inline mcpServers must produce an McpServer capability"
        );
    }
}
