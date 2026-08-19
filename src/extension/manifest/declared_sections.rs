//! The one translation from *manifest-declared* sections to capabilities.
//!
//! `[[tools]]`, `[[hooks]]`, `[[commands]]`, `[[services]]` and `[prompt]` are
//! the Aleph superset — the things a plugin states in its manifest rather than
//! implies by shipping a directory. Two manifest dialects carry them:
//!
//! * `aleph.plugin.toml` (native, deprecated) — at the document root;
//! * `.claude-plugin/plugin.toml` / `.json` (the documented-preferred CC
//!   dialects) — inside the `[aleph]` block Claude Code ignores by design.
//!
//! # Why this module exists
//!
//! Until 2026-08-19 only the deprecated dialect could express any of it.
//! `parse_cc_plugin_toml_content` hardcoded `tools_v2: None`, `prompt_v2:
//! None`, `config_schema: None` and `memory_manifest: None`, so the adapter's
//! `if let Some(ref tools) = manifest.tools_v2` branches were unreachable and
//! a plugin using the preferred format could not declare a tool at all — the
//! bundled `plugins/memory-analytics` exports `memory_stats` / `memory_timeline`
//! from WASM and ships a `.claude-plugin/plugin.toml`, so its entire tool
//! surface was unreachable. Meanwhile the guide told authors to use
//! `aleph.plugin.toml`, which the runtime warns is deprecated on every load.
//!
//! Both dialects now feed this function, so "what a declared section means" has
//! one answer and the two formats cannot drift.

use std::path::Path;

use crate::extension::capability::CapabilityDeclaration;
use crate::extension::manifest::toml_types::{
    CommandSection, HookSection, PromptSection, ServiceSection, ToolSection,
};
use crate::extension::manifest::{parsers, PluginPermission};

/// The `[aleph]` superset, owned.
///
/// The TOML dialect fills this from `AlephExtensionsToml`'s typed fields; the
/// JSON dialect deserializes the `aleph` object straight into it. Both spell
/// the keys in snake_case, with camelCase aliases so a JSON author can follow
/// Claude Code's convention throughout the file.
///
/// `cc_plugin_json::tests::both_cc_dialects_agree_on_the_superset` holds the
/// two paths to the same meaning — a behavioural guard, because the two
/// dialects necessarily have two deserialization structs and a structural
/// trick (`serde(flatten)`) is not safe for TOML arrays-of-tables.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct AlephSuperset {
    /// `[[tools]]` — handler-backed tools plus their instruction files.
    pub tools: Vec<ToolSection>,
    /// `[[hooks]]` — event hooks declared inline.
    pub hooks: Vec<HookSection>,
    /// `[[commands]]` — handler-backed `/commands`.
    pub commands: Vec<CommandSection>,
    /// `[prompt]` — a prompt file injected into the agent context.
    pub prompt: Option<PromptSection>,
    /// JSON Schema for this plugin's user configuration.
    #[serde(alias = "configSchema")]
    pub config_schema: Option<serde_json::Value>,
    /// Per-field presentation hints for `config_schema`.
    #[serde(alias = "configUiHints")]
    pub config_ui_hints:
        std::collections::HashMap<String, crate::extension::manifest::ConfigUiHint>,
    /// Memory extension manifest.
    pub memory: Option<crate::memory::extensions::manifest::MemoryManifestSection>,
}

impl AlephSuperset {
    /// `Vec` fields become `None` when empty so `PluginManifest`'s
    /// `Option<Vec<_>>` shape keeps meaning "the author declared none".
    #[must_use]
    pub fn non_empty<T>(items: Vec<T>) -> Option<Vec<T>> {
        (!items.is_empty()).then_some(items)
    }
}

/// The manifest-declared sections, borrowed from whichever dialect parsed them.
#[derive(Default)]
pub struct DeclaredSections<'a> {
    /// `[prompt]` — a system/user prompt file injected into the agent context.
    pub prompt: Option<&'a PromptSection>,
    /// `[[tools]]` — handler-backed tools plus their instruction files.
    pub tools: &'a [ToolSection],
    /// `[[hooks]]` — event hooks.
    pub hooks: &'a [HookSection],
    /// `[[commands]]` — handler-backed `/commands`.
    pub commands: &'a [CommandSection],
    /// `[[services]]` — background services, gated on `permissions.background`.
    pub services: &'a [ServiceSection],
}

/// Translate manifest-declared sections into capability declarations.
///
/// `permissions` is the plugin's granted set; `[[services]]` are skipped with a
/// warning when `background` is absent, which keeps a missing grant a
/// degradation rather than a whole-plugin failure at the registrar.
#[must_use]
pub fn declared_capabilities(
    plugin_dir: &Path,
    plugin_id: &str,
    sections: &DeclaredSections<'_>,
    permissions: &[PluginPermission],
) -> Vec<CapabilityDeclaration> {
    let mut capabilities = Vec::new();

    // [prompt] — system prompt file.
    if let Some(prompt) = sections.prompt {
        match parsers::parse_v2_prompt(plugin_dir, prompt, plugin_id) {
            Ok(cap) => capabilities.push(cap),
            Err(e) => tracing::debug!("Failed to parse [prompt] for {}: {}", plugin_id, e),
        }
    }

    // [[tools]] — instruction files first, then the handler-backed tools
    // themselves. A tool with no handler is prompt-only and registers no
    // callable tool; that is the `instruction_file` form.
    if !sections.tools.is_empty() {
        match parsers::parse_v2_tool_prompts(plugin_dir, sections.tools, plugin_id) {
            Ok(caps) => capabilities.extend(caps),
            Err(e) => tracing::debug!("Failed to parse [[tools]] for {}: {}", plugin_id, e),
        }
        for tool in sections.tools {
            let Some(handler) = tool.handler.clone().filter(|handler| !handler.is_empty()) else {
                continue;
            };
            capabilities.push(CapabilityDeclaration::Tool(
                crate::extension::ToolRegistration {
                    name: tool.name.clone(),
                    description: tool.description.clone().unwrap_or_default(),
                    parameters: tool
                        .parameters
                        .clone()
                        .unwrap_or_else(|| serde_json::json!({"type": "object"})),
                    handler,
                    plugin_id: plugin_id.to_string(),
                },
            ));
        }
    }

    // [[commands]] — handler-backed commands. Markdown `commands/` files are a
    // separate, directory-implied path (`parsers::parse_commands_dir`).
    for command in sections.commands {
        let Some(handler) = command
            .handler
            .clone()
            .filter(|handler| !handler.is_empty())
        else {
            continue;
        };
        capabilities.push(CapabilityDeclaration::Skill(
            crate::extension::SkillRegistration {
                name: command.name.clone(),
                description: command.description.clone().unwrap_or_default(),
                content: handler,
                skill_type: crate::extension::SkillType::Command,
                plugin_id: plugin_id.to_string(),
                ..Default::default()
            },
        ));
    }

    // [[hooks]] — event hooks.
    if !sections.hooks.is_empty() {
        capabilities.extend(parsers::parse_v2_hooks(sections.hooks, plugin_id));
    }

    // [[services]] — background services.
    if !sections.services.is_empty() {
        if permissions.contains(&PluginPermission::Background) {
            capabilities.extend(parsers::parse_v2_services(sections.services, plugin_id));
        } else {
            tracing::warn!(
                plugin = %plugin_id,
                "[[services]] declared but permissions.background is not granted — services skipped"
            );
        }
    }

    capabilities
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, handler: Option<&str>) -> ToolSection {
        ToolSection {
            name: name.to_string(),
            description: Some("d".to_string()),
            handler: handler.map(str::to_string),
            instruction_file: None,
            parameters: None,
        }
    }

    #[test]
    fn a_declared_tool_with_a_handler_becomes_a_callable_tool() {
        let dir = tempfile::tempdir().unwrap();
        let tools = vec![tool("stats", Some("memory_stats"))];
        let caps = declared_capabilities(
            dir.path(),
            "memory-analytics",
            &DeclaredSections {
                tools: &tools,
                ..Default::default()
            },
            &[],
        );
        let names: Vec<&str> = caps
            .iter()
            .filter_map(|c| match c {
                CapabilityDeclaration::Tool(t) => Some(t.name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["stats"]);
    }

    #[test]
    fn a_handlerless_tool_declares_no_callable_tool() {
        let dir = tempfile::tempdir().unwrap();
        let tools = vec![tool("prose-only", None)];
        let caps = declared_capabilities(
            dir.path(),
            "p",
            &DeclaredSections {
                tools: &tools,
                ..Default::default()
            },
            &[],
        );
        assert!(!caps
            .iter()
            .any(|c| matches!(c, CapabilityDeclaration::Tool(_))));
    }

    /// Services are the one section whose translation is permission-gated, and
    /// the gate must degrade rather than fail.
    #[test]
    fn services_need_the_background_grant() {
        let dir = tempfile::tempdir().unwrap();
        let services = vec![ServiceSection {
            name: "worker".to_string(),
            description: None,
            start_handler: Some("start".to_string()),
            stop_handler: Some("stop".to_string()),
            auto_start: true,
        }];
        let sections = DeclaredSections {
            services: &services,
            ..Default::default()
        };

        let denied = declared_capabilities(dir.path(), "p", &sections, &[]);
        assert!(denied.is_empty(), "no grant ⇒ no service capability");

        let granted =
            declared_capabilities(dir.path(), "p", &sections, &[PluginPermission::Background]);
        assert_eq!(granted.len(), 1);
    }
}
