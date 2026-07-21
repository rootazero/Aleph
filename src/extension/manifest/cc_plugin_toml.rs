//! CC-format TOML manifest parser (.claude-plugin/plugin.toml)
//!
//! This module parses the NEW preferred format `.claude-plugin/plugin.toml`.
//! It has flat top-level fields (name, version, description) plus an optional
//! `[aleph]` section for Aleph-specific extensions.

use std::path::Path;

use serde::Deserialize;

use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::manifest::toml_types::{
    convert_permissions, CapabilitiesSection, PermissionsSection, ServiceSection,
};
use crate::extension::manifest::types::{
    AlephExtensions, AlephRuntime, AuthorInfo, PluginManifest,
};
use crate::extension::manifest::{sanitize_plugin_id, validate_plugin_id};
use crate::extension::types::PluginKind;

/// Filename for CC-format TOML manifest
pub const CC_PLUGIN_TOML: &str = ".claude-plugin/plugin.toml";

// =============================================================================
// CC plugin.toml Types
// =============================================================================

/// Root structure for .claude-plugin/plugin.toml
///
/// Flat top-level fields follow Claude Code conventions. The optional `[aleph]`
/// section holds Aleph-specific extensions that CC ignores.
#[derive(Debug, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct CcPluginToml {
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

    /// Repository URL
    pub repository: Option<String>,

    /// Author section
    #[serde(rename = "author")]
    pub author: Option<CcPluginAuthorToml>,

    // Flat component paths (CC-native)
    /// Path to skills directory
    pub skills: Option<String>,

    /// Path to commands directory
    pub commands: Option<String>,

    /// Path to agents directory
    pub agents: Option<String>,

    /// Path to hooks file/directory
    pub hooks: Option<String>,

    /// Path to MCP servers configuration
    #[serde(rename = "mcp-servers")]
    pub mcp_servers: Option<String>,

    /// Aleph-specific extensions (optional, ignored by Claude Code)
    pub aleph: Option<AlephExtensionsToml>,
}

/// Author section in CC plugin.toml
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum CcPluginAuthorToml {
    /// Simple string format: "Name <email>"
    String(String),
    /// Object format with optional fields
    Object {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        email: Option<String>,
        #[serde(default)]
        url: Option<String>,
    },
}

impl From<CcPluginAuthorToml> for AuthorInfo {
    fn from(author: CcPluginAuthorToml) -> Self {
        match author {
            CcPluginAuthorToml::String(s) => Self::from(s.as_str()),
            CcPluginAuthorToml::Object { name, email, url } => Self { name, email, url },
        }
    }
}

/// Aleph-specific extensions inside [aleph] section
#[derive(Debug, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct AlephExtensionsToml {
    /// Runtime type: "wasm", "mcp", "static"
    pub runtime: Option<String>,

    /// Entry point override
    pub entry: Option<String>,

    /// Permission grants
    #[serde(default)]
    pub permissions: PermissionsSection,

    /// WASM capabilities
    #[serde(default)]
    pub capabilities: CapabilitiesSection,

    /// Background services
    #[serde(default)]
    pub services: Vec<ServiceSection>,
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

fn runtime_to_aleph_runtime(runtime: &str) -> AlephRuntime {
    match runtime {
        "wasm" => AlephRuntime::Wasm,
        "mcp" => AlephRuntime::Mcp,
        _ => AlephRuntime::Static,
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

/// Parse .claude-plugin/plugin.toml content into a `PluginManifest`
///
/// # Arguments
/// * `content` - TOML content string
/// * `plugin_dir` - Path to the plugin root directory
pub fn parse_cc_plugin_toml_content(
    content: &str,
    plugin_dir: &Path,
) -> ExtensionResult<PluginManifest> {
    let manifest_path = plugin_dir.join(CC_PLUGIN_TOML);

    let toml: CcPluginToml = toml::from_str(content).map_err(|e| {
        ExtensionError::invalid_manifest(&manifest_path, format!("TOML parse error: {e}"))
    })?;

    // `name` is required
    let raw_name = toml
        .name
        .ok_or_else(|| ExtensionError::missing_field(&manifest_path, "name"))?;
    if raw_name.is_empty() {
        return Err(ExtensionError::missing_field(&manifest_path, "name"));
    }

    // Derive plugin ID from name
    let plugin_id = sanitize_plugin_id(&raw_name);
    validate_plugin_id(&plugin_id)
        .map_err(|reason| ExtensionError::invalid_plugin_name(&raw_name, reason))?;

    // Determine kind and entry from [aleph] section
    let (kind, entry, aleph_extensions) = if let Some(aleph) = toml.aleph {
        let runtime_str = aleph.runtime.as_deref().unwrap_or("static");
        let kind = runtime_to_kind(runtime_str);
        let entry = aleph.entry.unwrap_or_else(|| default_entry_for_kind(kind));
        let permissions = convert_permissions(&aleph.permissions);

        let aleph_ext = AlephExtensions {
            runtime: runtime_to_aleph_runtime(runtime_str),
            entry: Some(entry.clone()),
            services: aleph.services,
            permissions: if aleph.permissions.network
                || aleph.permissions.env
                || aleph.permissions.shell
                || aleph.permissions.filesystem != Default::default()
            {
                Some(aleph.permissions)
            } else {
                None
            },
            capabilities: if aleph.capabilities.dynamic_tools
                || aleph.capabilities.dynamic_hooks
                || aleph.capabilities.workspace.is_some()
                || aleph.capabilities.http.is_some()
                || aleph.capabilities.tool_invoke.is_some()
                || aleph.capabilities.secrets.is_some()
            {
                Some(aleph.capabilities)
            } else {
                None
            },
        };

        (kind, entry, Some((aleph_ext, permissions)))
    } else {
        (PluginKind::Static, ".".to_string(), None)
    };

    let (aleph_ext, permissions) =
        aleph_extensions.map_or((None, Vec::new()), |(ext, perms)| (Some(ext), perms));
    let wasm_capabilities = aleph_ext
        .as_ref()
        .and_then(|ext| ext.capabilities.as_ref())
        .and_then(crate::extension::manifest::toml_types::convert_wasm_capabilities);

    let manifest = PluginManifest {
        id: plugin_id,
        name: raw_name,
        version: toml.version,
        description: toml.description,
        kind,
        entry: entry.into(),
        root_dir: plugin_dir.to_path_buf(),
        config_schema: None,
        config_ui_hints: Default::default(),
        permissions,
        author: toml.author.map(AuthorInfo::from),
        homepage: toml.homepage,
        repository: toml.repository,
        license: toml.license,
        keywords: toml.keywords.unwrap_or_default(),
        extensions: Vec::new(),
        // V2 fields not available in CC TOML format
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
        // Memory extension manifest — not supported in CC flat format (only in aleph.plugin.toml)
        memory_manifest: None,
    };

    Ok(manifest)
}

/// Parse .claude-plugin/plugin.toml from a plugin directory (sync)
///
/// # Arguments
/// * `dir` - Path to the plugin root directory
pub fn parse_cc_plugin_toml_sync(dir: &Path) -> ExtensionResult<PluginManifest> {
    let toml_path = dir.join(CC_PLUGIN_TOML);
    let content = std::fs::read_to_string(&toml_path)?;
    parse_cc_plugin_toml_content(&content, dir)
}

/// Parse .claude-plugin/plugin.toml from a plugin directory (async)
///
/// # Arguments
/// * `dir` - Path to the plugin root directory
pub async fn parse_cc_plugin_toml(dir: &Path) -> ExtensionResult<PluginManifest> {
    let toml_path = dir.join(CC_PLUGIN_TOML);
    let content = tokio::fs::read_to_string(&toml_path).await?;
    parse_cc_plugin_toml_content(&content, dir)
}

// =============================================================================
// ManifestAdapter implementation
// =============================================================================

use super::adapter::{AdapterOutput, ManifestAdapter};
use super::parsers;
use crate::extension::capability::{CapabilitySource, SourceFormat};
use crate::extension::types::PluginOrigin;

/// `ManifestAdapter` for `.claude-plugin/plugin.toml` format.
///
/// Priority 100 (highest among CC adapters) — TOML is preferred over JSON.
pub struct ClaudeCodeTomlAdapter;

impl ManifestAdapter for ClaudeCodeTomlAdapter {
    fn detect(&self, plugin_dir: &Path) -> bool {
        plugin_dir.join(CC_PLUGIN_TOML).exists()
    }

    fn parse(&self, plugin_dir: &Path) -> anyhow::Result<AdapterOutput> {
        let manifest = parse_cc_plugin_toml_sync(plugin_dir)
            .map_err(|e| anyhow::anyhow!("CC TOML parse error: {e}"))?;

        let plugin_id = manifest.id.clone();
        let mut capabilities = Vec::new();

        // Read component paths from the raw TOML to get skills/agents/hooks/mcp paths
        let toml_path = plugin_dir.join(CC_PLUGIN_TOML);
        let content = std::fs::read_to_string(&toml_path)?;
        let raw: CcPluginToml =
            toml::from_str(&content).map_err(|e| anyhow::anyhow!("TOML re-parse error: {e}"))?;

        // Parse skills
        let skills_rel = raw.skills.as_deref().unwrap_or("skills");
        capabilities.extend(parsers::parse_skills_dir(
            plugin_dir, skills_rel, &plugin_id,
        )?);

        // Parse commands
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

        // Parse v2 prompt configuration from manifest (if present)
        if let Some(ref prompt) = manifest.prompt_v2 {
            match parsers::parse_v2_prompt(plugin_dir, prompt, &plugin_id) {
                Ok(cap) => capabilities.push(cap),
                Err(e) => tracing::debug!("Failed to parse v2 prompt for {}: {}", plugin_id, e),
            }
        }

        // Parse v2 tool instruction files (if present)
        if let Some(ref tools) = manifest.tools_v2 {
            match parsers::parse_v2_tool_prompts(plugin_dir, tools, &plugin_id) {
                Ok(caps) => capabilities.extend(caps),
                Err(e) => {
                    tracing::debug!("Failed to parse v2 tool prompts for {}: {}", plugin_id, e)
                }
            }
        }

        // Background services declared in the [aleph] section's [[services]] —
        // gated on the `background` permission so a missing grant degrades to
        // a warning instead of failing the whole plugin at the registrar.
        if let Some(ref aleph_ext) = manifest.aleph_extensions {
            if !aleph_ext.services.is_empty() {
                if manifest
                    .permissions
                    .contains(&crate::extension::manifest::PluginPermission::Background)
                {
                    capabilities
                        .extend(parsers::parse_v2_services(&aleph_ext.services, &plugin_id));
                } else {
                    tracing::warn!(
                        plugin = %plugin_id,
                        "[[services]] declared but permissions.background is not granted — services skipped"
                    );
                }
            }
        }

        Ok(AdapterOutput {
            plugin_id: plugin_id.clone(),
            name: Some(manifest.name),
            version: manifest.version.clone(),
            description: manifest.description.clone(),
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
        "Claude Code (TOML)"
    }

    fn priority(&self) -> i32 {
        100
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
    fn test_parse_minimal_cc_toml() {
        let content = r#"
name = "My Plugin"
version = "1.0.0"
"#;
        let manifest = parse_cc_plugin_toml_content(content, &test_dir()).unwrap();

        assert_eq!(manifest.id, "my-plugin");
        assert_eq!(manifest.name, "My Plugin");
        assert_eq!(manifest.version, Some("1.0.0".to_string()));
        assert_eq!(manifest.kind, PluginKind::Static);
        assert_eq!(manifest.entry, PathBuf::from("."));
        assert!(manifest.aleph_extensions.is_none());
    }

    #[test]
    fn test_parse_cc_toml_with_aleph_extensions() {
        let content = r#"
name = "wasm-plugin"
version = "2.0.0"
description = "A WASM plugin"

[aleph]
runtime = "wasm"
entry = "dist/plugin.wasm"

[aleph.permissions]
network = true
env = true
"#;
        let manifest = parse_cc_plugin_toml_content(content, &test_dir()).unwrap();

        assert_eq!(manifest.id, "wasm-plugin");
        assert_eq!(manifest.kind, PluginKind::Wasm);
        assert_eq!(manifest.entry, PathBuf::from("dist/plugin.wasm"));
        assert!(manifest.aleph_extensions.is_some());

        let ext = manifest.aleph_extensions.unwrap();
        assert_eq!(ext.runtime, AlephRuntime::Wasm);
        assert_eq!(ext.entry, Some("dist/plugin.wasm".to_string()));

        // permissions were set, so they should be in the manifest
        assert!(manifest
            .permissions
            .contains(&crate::extension::manifest::types::PluginPermission::Network));
        assert!(manifest
            .permissions
            .contains(&crate::extension::manifest::types::PluginPermission::Env));
    }

    #[test]
    fn test_cc_toml_name_required() {
        let content = r#"
version = "1.0.0"
description = "No name here"
"#;
        let result = parse_cc_plugin_toml_content(content, &test_dir());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("name") || err.contains("missing"),
            "Expected missing name error, got: {}",
            err
        );
    }

    #[test]
    fn test_cc_toml_runtime_determines_kind() {
        // wasm → Wasm
        let content = r#"
name = "wasm-test"
[aleph]
runtime = "wasm"
"#;
        let manifest = parse_cc_plugin_toml_content(content, &test_dir()).unwrap();
        assert_eq!(manifest.kind, PluginKind::Wasm);

        // mcp → Mcp
        let content = r#"
name = "mcp-test"
[aleph]
runtime = "mcp"
"#;
        let manifest = parse_cc_plugin_toml_content(content, &test_dir()).unwrap();
        assert_eq!(manifest.kind, PluginKind::Mcp);

        // static → Static
        let content = r#"
name = "static-test"
[aleph]
runtime = "static"
"#;
        let manifest = parse_cc_plugin_toml_content(content, &test_dir()).unwrap();
        assert_eq!(manifest.kind, PluginKind::Static);

        // no aleph section → Static
        let content = r#"
name = "no-aleph"
"#;
        let manifest = parse_cc_plugin_toml_content(content, &test_dir()).unwrap();
        assert_eq!(manifest.kind, PluginKind::Static);
    }

    #[test]
    fn test_parse_cc_toml_author_string() {
        let content = r#"
name = "author-test"
author = "Alice <alice@example.com>"
"#;
        let manifest = parse_cc_plugin_toml_content(content, &test_dir()).unwrap();
        let author = manifest.author.unwrap();
        assert_eq!(author.name, Some("Alice".to_string()));
        assert_eq!(author.email, Some("alice@example.com".to_string()));
    }

    #[test]
    fn test_parse_cc_toml_mcp_default_entry() {
        let content = r#"
name = "mcp-defaults"
[aleph]
runtime = "mcp"
"#;
        let manifest = parse_cc_plugin_toml_content(content, &test_dir()).unwrap();
        assert_eq!(manifest.entry, PathBuf::from(".mcp.json"));
    }

    #[test]
    fn test_parse_cc_toml_wasm_default_entry() {
        let content = r#"
name = "wasm-defaults"
[aleph]
runtime = "wasm"
"#;
        let manifest = parse_cc_plugin_toml_content(content, &test_dir()).unwrap();
        assert_eq!(manifest.entry, PathBuf::from("plugin.wasm"));
    }

    #[test]
    fn test_parse_cc_toml_root_dir_set() {
        let content = r#"name = "root-dir-test""#;
        let dir = PathBuf::from("/some/plugin/dir");
        let manifest = parse_cc_plugin_toml_content(content, &dir).unwrap();
        assert_eq!(manifest.root_dir, dir);
    }

    // Integration tests: end-to-end discovery via parse_manifest_from_dir_sync

    #[test]
    fn test_full_cc_plugin_discovery() {
        use std::fs;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let plugin_dir = dir.path().join("my-cc-plugin");
        let cc_dir = plugin_dir.join(".claude-plugin");
        let skills_dir = plugin_dir.join("skills/hello");

        fs::create_dir_all(&cc_dir).unwrap();
        fs::create_dir_all(&skills_dir).unwrap();

        // Write CC-format plugin.toml
        fs::write(
            cc_dir.join("plugin.toml"),
            r#"
name = "my-cc-plugin"
version = "0.1.0"
description = "Test CC plugin"
skills = "./skills/"
"#,
        )
        .unwrap();

        fs::write(
            skills_dir.join("SKILL.md"),
            "---\nname: hello\n---\nHello world",
        )
        .unwrap();

        // Parse via the main discovery function
        let manifest =
            crate::extension::manifest::parse_manifest_from_dir_sync(&plugin_dir).unwrap();
        assert_eq!(manifest.id, "my-cc-plugin");
        assert_eq!(manifest.version, Some("0.1.0".to_string()));
    }

    #[test]
    fn test_old_format_still_loads() {
        use std::fs;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let plugin_dir = dir.path().join("old-plugin");
        fs::create_dir_all(&plugin_dir).unwrap();

        fs::write(
            plugin_dir.join("aleph.plugin.toml"),
            r#"
[plugin]
id = "old-plugin"
name = "Old Plugin"
version = "0.1.0"
kind = "static"
entry = "."
"#,
        )
        .unwrap();

        let manifest =
            crate::extension::manifest::parse_manifest_from_dir_sync(&plugin_dir).unwrap();
        assert_eq!(manifest.id, "old-plugin");
    }

    #[test]
    fn test_cc_format_takes_priority_over_old() {
        use std::fs;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let plugin_dir = dir.path().join("dual-plugin");
        let cc_dir = plugin_dir.join(".claude-plugin");
        fs::create_dir_all(&cc_dir).unwrap();

        // Write both old and new format
        fs::write(
            plugin_dir.join("aleph.plugin.toml"),
            r#"
[plugin]
id = "old-id"
name = "Old Name"
kind = "static"
entry = "."
"#,
        )
        .unwrap();

        fs::write(
            cc_dir.join("plugin.toml"),
            r#"
name = "new-id"
version = "2.0.0"
"#,
        )
        .unwrap();

        // CC format should win
        let manifest =
            crate::extension::manifest::parse_manifest_from_dir_sync(&plugin_dir).unwrap();
        assert_eq!(manifest.id, "new-id");
        assert_eq!(manifest.version, Some("2.0.0".to_string()));
    }
}
