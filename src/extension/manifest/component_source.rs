//! Claude Code's component fields are a union, not a path.
//!
//! In `.claude-plugin/plugin.json` (and its TOML twin) each of `skills`,
//! `commands`, `agents`, `hooks` and `mcpServers` accepts more than one shape:
//! a path string, an array of paths, or — for `hooks` and `mcpServers` — the
//! configuration inlined as an object. Two of Anthropic's own plugin manifests
//! use the inline `mcpServers` form.
//!
//! Aleph declared all five as `Option<String>` until 2026-08-19. `serde` does
//! not degrade on a type mismatch: it fails the *whole* struct, so
//! `parse_cc_plugin_json_content` returned `invalid_manifest` and the plugin
//! landed in the registry with `PluginStatus::Error` and **zero**
//! capabilities. Not silent — `mod.rs` warns and gives it a visible row — but
//! all-or-nothing, and it failed loudest for the multi-component plugins most
//! worth having.
//!
//! Claude Code is deliberately lenient at the top level so a newer manifest
//! never bricks an older client. This module is Aleph's half of that contract.
//!
//! # The inline arms need consumers, not just deserializers
//!
//! Widening the type alone would convert a loud "manifest rejected" into a
//! quiet "loaded, 6 skills, zero MCP servers" — strictly worse by this repo's
//! own criteria. So each arm here resolves through a real parser:
//! `parse_hooks_content` and `parse_mcp_config_content` exist for exactly this
//! reason.

use std::path::Path;

use anyhow::Result;
use serde::Deserialize;

use crate::extension::capability::CapabilityDeclaration;
use crate::extension::manifest::parsers;

/// One of Claude Code's accepted shapes for a component field.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ComponentSource {
    /// A single path, relative to the plugin root.
    Path(String),
    /// Several paths.
    Paths(Vec<String>),
    /// The configuration inlined. Only `hooks` and `mcpServers` define a
    /// meaning for this; for the other three it is reported and skipped rather
    /// than guessed at.
    Inline(serde_json::Value),
}

impl ComponentSource {
    /// The paths this field names, if any. Empty for the inline form.
    #[must_use]
    pub fn paths(&self) -> Vec<&str> {
        match self {
            Self::Path(p) => vec![p.as_str()],
            Self::Paths(ps) => ps.iter().map(String::as_str).collect(),
            Self::Inline(_) => Vec::new(),
        }
    }

    /// The inlined configuration, if this field carries one.
    #[must_use]
    pub const fn inline(&self) -> Option<&serde_json::Value> {
        match self {
            Self::Inline(v) => Some(v),
            _ => None,
        }
    }
}

/// Resolve a directory-shaped component field (`skills` / `commands` /
/// `agents`) using `parse` for each named path.
///
/// `default_rel` is used when the field is absent, preserving the
/// convention-over-configuration default.
pub fn resolve_dirs(
    field: Option<&ComponentSource>,
    default_rel: &str,
    plugin_dir: &Path,
    plugin_id: &str,
    field_name: &str,
    parse: impl Fn(&Path, &str, &str) -> Result<Vec<CapabilityDeclaration>>,
) -> Result<Vec<CapabilityDeclaration>> {
    let Some(field) = field else {
        return parse(plugin_dir, default_rel, plugin_id);
    };
    if let Some(inline) = field.inline() {
        // Claude Code's record form for `commands`
        // (`{name: {source|content}}`) has no Aleph consumer yet. Say so —
        // a plugin that loads with a component silently missing is the
        // failure this module exists to avoid.
        tracing::warn!(
            plugin = %plugin_id,
            field = field_name,
            keys = inline.as_object().map_or(0, serde_json::Map::len),
            "inline object form for this component field is not supported — component skipped"
        );
        return Ok(Vec::new());
    }
    let mut caps = Vec::new();
    for rel in field.paths() {
        caps.extend(parse(plugin_dir, rel, plugin_id)?);
    }
    Ok(caps)
}

/// Resolve the `hooks` field: path(s), or hooks configuration inlined.
pub fn resolve_hooks(
    field: Option<&ComponentSource>,
    plugin_dir: &Path,
    plugin_id: &str,
) -> Result<Vec<CapabilityDeclaration>> {
    let Some(field) = field else {
        return parsers::parse_hooks_file(plugin_dir, "hooks/hooks.json", plugin_id);
    };
    if let Some(inline) = field.inline() {
        return parsers::parse_hooks_content(&inline.to_string(), plugin_dir, plugin_id);
    }
    let mut caps = Vec::new();
    for rel in field.paths() {
        caps.extend(parsers::parse_hooks_file(plugin_dir, rel, plugin_id)?);
    }
    Ok(caps)
}

/// Resolve the `mcpServers` field: path(s), or server configuration inlined.
pub fn resolve_mcp_servers(
    field: Option<&ComponentSource>,
    plugin_dir: &Path,
    plugin_id: &str,
) -> Result<Vec<CapabilityDeclaration>> {
    let Some(field) = field else {
        return parsers::parse_mcp_config_file(plugin_dir, ".mcp.json", plugin_id);
    };
    if let Some(inline) = field.inline() {
        return parsers::parse_mcp_config_content(&inline.to_string(), plugin_dir);
    }
    let mut caps = Vec::new();
    for rel in field.paths() {
        caps.extend(parsers::parse_mcp_config_file(plugin_dir, rel, plugin_id)?);
    }
    Ok(caps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_claude_code_shape_deserializes() {
        let single: ComponentSource = serde_json::from_str(r#""./skills""#).unwrap();
        assert_eq!(single.paths(), vec!["./skills"]);

        let many: ComponentSource = serde_json::from_str(r#"["./a", "./b"]"#).unwrap();
        assert_eq!(many.paths(), vec!["./a", "./b"]);

        let inline: ComponentSource =
            serde_json::from_str(r#"{"srv": {"command": "node"}}"#).unwrap();
        assert!(inline.paths().is_empty());
        assert!(inline.inline().is_some());
    }

    /// The inline `mcpServers` form is the one real Anthropic manifests use,
    /// and it must produce actual server capabilities — not merely parse.
    #[test]
    fn an_inline_mcp_object_registers_servers() {
        let dir = tempfile::tempdir().unwrap();
        let field: ComponentSource =
            serde_json::from_str(r#"{"chrome": {"command": "npx", "args": ["-y", "x"]}}"#).unwrap();
        let caps = resolve_mcp_servers(Some(&field), dir.path(), "p").unwrap();
        assert_eq!(
            caps.len(),
            1,
            "inline mcpServers must reach parse_mcp_config_content"
        );
    }

    /// An unsupported inline form must not look like "the author declared
    /// none" — the warning is the whole difference between a loud gap and a
    /// silent one.
    #[test]
    fn an_unsupported_inline_form_yields_nothing_rather_than_guessing() {
        let dir = tempfile::tempdir().unwrap();
        let field: ComponentSource =
            serde_json::from_str(r#"{"review": {"content": "x"}}"#).unwrap();
        let caps = resolve_dirs(
            Some(&field),
            "commands",
            dir.path(),
            "p",
            "commands",
            parsers::parse_commands_dir,
        )
        .unwrap();
        assert!(caps.is_empty());
    }
}
