//! TOML manifest parsing functions

use std::path::Path;

use super::conversion::convert_permissions;
use super::types::AlephPluginToml;
use super::wasm_capabilities::convert_wasm_capabilities;
use super::ALEPH_PLUGIN_TOML;
use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::manifest::types::{AuthorInfo, PluginManifest};
use crate::extension::manifest::{sanitize_plugin_id, validate_plugin_id};
use crate::extension::types::PluginKind;

/// Parse an aleph.plugin.toml file into a PluginManifest (async)
///
/// # Arguments
/// * `dir` - Path to the plugin directory containing aleph.plugin.toml
///
/// # Returns
/// * `Ok(PluginManifest)` - Parsed manifest with root_dir set
/// * `Err(ExtensionError)` - If parsing fails or required fields are missing
pub async fn parse_aleph_plugin_toml(dir: &Path) -> ExtensionResult<PluginManifest> {
    let toml_path = dir.join(ALEPH_PLUGIN_TOML);
    let content = tokio::fs::read_to_string(&toml_path).await?;
    parse_aleph_plugin_toml_content(&content, dir)
}

/// Parse an aleph.plugin.toml file into a PluginManifest (sync)
///
/// # Arguments
/// * `dir` - Path to the plugin directory containing aleph.plugin.toml
///
/// # Returns
/// * `Ok(PluginManifest)` - Parsed manifest with root_dir set
/// * `Err(ExtensionError)` - If parsing fails or required fields are missing
pub fn parse_aleph_plugin_toml_sync(dir: &Path) -> ExtensionResult<PluginManifest> {
    let toml_path = dir.join(ALEPH_PLUGIN_TOML);
    let content = std::fs::read_to_string(&toml_path)?;
    parse_aleph_plugin_toml_content(&content, dir)
}

/// Parse TOML content into a PluginManifest
///
/// This is the core parsing function that converts TOML content to PluginManifest.
///
/// # Arguments
/// * `content` - TOML content string
/// * `plugin_dir` - Path to the plugin directory (for root_dir)
///
/// # Returns
/// * `Ok(PluginManifest)` - Parsed manifest
/// * `Err(ExtensionError)` - If parsing fails or validation fails
pub fn parse_aleph_plugin_toml_content(
    content: &str,
    plugin_dir: &Path,
) -> ExtensionResult<PluginManifest> {
    let toml_path = plugin_dir.join(ALEPH_PLUGIN_TOML);

    // Parse TOML
    let toml: AlephPluginToml = toml::from_str(content)
        .map_err(|e| ExtensionError::invalid_manifest(&toml_path, format!("TOML parse error: {}", e)))?;

    // Validate plugin ID
    let plugin_id = if toml.plugin.id.is_empty() {
        return Err(ExtensionError::missing_field(&toml_path, "plugin.id"));
    } else {
        // Sanitize the ID if needed
        let sanitized = sanitize_plugin_id(&toml.plugin.id);
        validate_plugin_id(&sanitized)
            .map_err(|reason| ExtensionError::invalid_plugin_name(&toml.plugin.id, reason))?;
        sanitized
    };

    // Determine display name
    let name = toml.plugin.name.unwrap_or_else(|| plugin_id.clone());

    // Determine plugin kind (default to Wasm)
    let kind = toml.plugin.kind.unwrap_or(PluginKind::Wasm);

    // Determine entry point based on kind
    let entry = toml.plugin.entry.unwrap_or_else(|| match kind {
        PluginKind::Wasm => "plugin.wasm".to_string(),
        PluginKind::NodeJs => "index.js".to_string(),
        PluginKind::Static => ".".to_string(),
    });

    // Convert permissions
    let permissions = convert_permissions(&toml.permissions);

    // Build manifest
    let manifest = PluginManifest {
        id: plugin_id,
        name,
        version: toml.plugin.version,
        description: toml.plugin.description,
        kind,
        entry: entry.into(),
        root_dir: plugin_dir.to_path_buf(),
        config_schema: toml.plugin.config_schema,
        config_ui_hints: toml.plugin.config_ui_hints.unwrap_or_default(),
        permissions,
        author: toml.plugin.author.map(AuthorInfo::from),
        homepage: toml.plugin.homepage,
        repository: toml.plugin.repository,
        license: toml.plugin.license,
        keywords: toml.plugin.keywords.unwrap_or_default(),
        extensions: toml.plugin.extensions.unwrap_or_default(),
        // V2 fields from TOML
        tools_v2: if toml.tools.is_empty() {
            None
        } else {
            Some(toml.tools)
        },
        hooks_v2: if toml.hooks.is_empty() {
            None
        } else {
            Some(toml.hooks)
        },
        commands_v2: if toml.commands.is_empty() {
            None
        } else {
            Some(toml.commands)
        },
        services_v2: if toml.services.is_empty() {
            None
        } else {
            Some(toml.services)
        },
        prompt_v2: toml.prompt,
        wasm_capabilities: convert_wasm_capabilities(&toml.capabilities),
        wasm_resource_limits: None, // Parsed from [plugin.limits] in future
        capabilities_v2: Some(toml.capabilities),
        // P2 fields from TOML
        channels_v2: if toml.channels.is_empty() {
            None
        } else {
            Some(toml.channels)
        },
        providers_v2: if toml.providers.is_empty() {
            None
        } else {
            Some(toml.providers)
        },
        http_routes_v2: if toml.http_routes.is_empty() {
            None
        } else {
            Some(toml.http_routes)
        },
        aleph_extensions: None,
    };

    Ok(manifest)
}
