//! Native `aleph.plugin.toml` adapter.
//!
//! The native (deprecated) manifest is recognised by the metadata path
//! (`parse_manifest_from_dir_sync`) but was missing from the capability
//! `AdapterRegistry`, so native plugins lost their declared prompt/tool-prompt
//! capabilities and got a directory-name id instead of the manifest id. This
//! adapter closes that gap, extracting the same component-directory capabilities
//! as auto-discover plus the manifest-declared `[prompt]` and `[[tools]]`
//! instruction files, keyed by the canonical `plugin.id`.

use std::path::Path;

use crate::extension::manifest::inputs;

use anyhow::Result;

use crate::extension::capability::{CapabilitySource, SourceFormat};
use crate::extension::manifest::adapter::{AdapterOutput, ManifestAdapter};
use crate::extension::manifest::toml_types::{manifest_from_toml, AlephPluginToml};
use crate::extension::manifest::{declared_sections, parsers, ALEPH_PLUGIN_TOML};
use crate::extension::types::PluginOrigin;

pub struct AlephTomlAdapter;

impl ManifestAdapter for AlephTomlAdapter {
    fn detect(&self, plugin_dir: &Path) -> bool {
        inputs::exists(plugin_dir.join(ALEPH_PLUGIN_TOML))
    }

    fn parse(&self, plugin_dir: &Path) -> Result<AdapterOutput> {
        let toml_path = plugin_dir.join(ALEPH_PLUGIN_TOML);
        let content = inputs::read_to_string(&toml_path)?;
        let raw: AlephPluginToml = toml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("aleph.plugin.toml parse error: {e}"))?;

        let name = raw.plugin.name.clone();
        let manifest = manifest_from_toml(raw, plugin_dir)?;
        let plugin_id = manifest.id.clone();
        let mut capabilities = Vec::new();

        // Component directories (same convention as auto-discover).
        if inputs::is_dir(plugin_dir.join("skills")) {
            capabilities.extend(parsers::parse_skills_dir(plugin_dir, "skills", &plugin_id)?);
        }
        // Commands are merged into skills following the OpenClaw convention.
        if inputs::is_dir(plugin_dir.join("commands")) {
            capabilities.extend(parsers::parse_commands_dir(
                plugin_dir, "commands", &plugin_id,
            )?);
        }
        if inputs::is_dir(plugin_dir.join("agents")) {
            capabilities.extend(parsers::parse_agents_dir(plugin_dir, "agents", &plugin_id)?);
        }
        for hooks_path in &["hooks/hooks.json", "hooks.json"] {
            if inputs::exists(plugin_dir.join(hooks_path)) {
                capabilities.extend(parsers::parse_hooks_file(
                    plugin_dir, hooks_path, &plugin_id,
                )?);
                break;
            }
        }
        if inputs::exists(plugin_dir.join(".mcp.json")) {
            capabilities.extend(parsers::parse_mcp_config_file(
                plugin_dir,
                ".mcp.json",
                &plugin_id,
            )?);
        }

        // Manifest-declared sections ([prompt] / [[tools]] / [[commands]] /
        // [[hooks]] / [[services]]) go through the one translation shared with
        // the CC dialects, so the two formats cannot mean different things by
        // the same section.
        capabilities.extend(declared_sections::declared_capabilities(
            plugin_dir,
            &plugin_id,
            &declared_sections::DeclaredSections {
                prompt: manifest.prompt_v2.as_ref(),
                tools: manifest.tools_v2.as_deref().unwrap_or_default(),
                hooks: manifest.hooks_v2.as_deref().unwrap_or_default(),
                commands: manifest.commands_v2.as_deref().unwrap_or_default(),
                services: manifest.services_v2.as_deref().unwrap_or_default(),
            },
            &manifest.permissions,
        ));

        Ok(AdapterOutput {
            plugin_id: plugin_id.clone(),
            name,
            version: manifest.version.clone(),
            description: manifest.description.clone(),
            capabilities,
            source: CapabilitySource {
                plugin_id,
                origin: PluginOrigin::Global,
                format: SourceFormat::AlephToml,
            },
            permissions: manifest.permissions.clone(),
            runtime_manifest: Some(manifest),
            diagnostics: Vec::new(),
        })
    }

    fn format_name(&self) -> &str {
        "Aleph (native TOML)"
    }

    fn priority(&self) -> i32 {
        85
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::capability::CapabilityDeclaration;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_detect_native_toml() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(ALEPH_PLUGIN_TOML), "[plugin]\nid = \"x\"\n").unwrap();
        assert!(AlephTomlAdapter.detect(dir.path()));
    }

    #[test]
    fn test_parse_uses_manifest_id_and_skills() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join(ALEPH_PLUGIN_TOML),
            r#"
[plugin]
id = "native-plugin"
name = "Native Plugin"
version = "1.2.3"

[permissions]
shell = true
"#,
        )
        .unwrap();
        let skill_dir = dir.path().join("skills").join("hello");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: hello\ndescription: Say hi\n---\nHi!",
        )
        .unwrap();

        let out = AlephTomlAdapter.parse(dir.path()).unwrap();
        assert_eq!(out.plugin_id, "native-plugin");
        assert_eq!(out.name.as_deref(), Some("Native Plugin"));
        assert_eq!(out.version.as_deref(), Some("1.2.3"));
        assert_eq!(out.source.format, SourceFormat::AlephToml);
        assert!(out
            .permissions
            .contains(&crate::extension::manifest::PluginPermission::Shell));
        assert!(out
            .capabilities
            .iter()
            .any(|c| matches!(c, CapabilityDeclaration::Skill(s) if s.name == "hello")));
    }

    #[test]
    fn test_parse_services_with_background_permission() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join(ALEPH_PLUGIN_TOML),
            r#"
[plugin]
id = "svc-plugin"
name = "Service Plugin"

[permissions]
background = true

[[services]]
name = "worker"
start_handler = "start_worker"
stop_handler = "stop_worker"

[[services]]
name = "no-handlers"
"#,
        )
        .unwrap();

        let out = AlephTomlAdapter.parse(dir.path()).unwrap();
        assert!(out
            .permissions
            .contains(&crate::extension::manifest::PluginPermission::Background));

        let services: Vec<_> = out
            .capabilities
            .iter()
            .filter_map(|c| match c {
                CapabilityDeclaration::Service(s) => Some(s),
                _ => None,
            })
            .collect();
        // "no-handlers" lacks start/stop handlers and must be skipped.
        assert_eq!(services.len(), 1);
        assert_eq!(services[0].id, "worker");
        assert_eq!(services[0].start_handler, "start_worker");
        assert_eq!(services[0].stop_handler, "stop_worker");
        assert!(services[0].auto_start, "auto_start defaults to true");
    }

    #[test]
    fn test_parse_services_skipped_without_background_permission() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join(ALEPH_PLUGIN_TOML),
            r#"
[plugin]
id = "svc-plugin"
name = "Service Plugin"

[[services]]
name = "worker"
start_handler = "start_worker"
stop_handler = "stop_worker"
"#,
        )
        .unwrap();

        let out = AlephTomlAdapter.parse(dir.path()).unwrap();
        // Without permissions.background the service is skipped (warn), and
        // the rest of the plugin still parses.
        assert!(!out
            .capabilities
            .iter()
            .any(|c| matches!(c, CapabilityDeclaration::Service(_))));
    }

    #[test]
    fn test_service_autostart_opt_out() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join(ALEPH_PLUGIN_TOML),
            r#"
[plugin]
id = "svc-plugin"
name = "Service Plugin"

[permissions]
background = true

[[services]]
name = "manual-worker"
start_handler = "start"
stop_handler = "stop"
auto_start = false
"#,
        )
        .unwrap();

        let out = AlephTomlAdapter.parse(dir.path()).unwrap();
        let svc = out
            .capabilities
            .iter()
            .find_map(|c| match c {
                CapabilityDeclaration::Service(s) => Some(s),
                _ => None,
            })
            .expect("service capability emitted");
        assert!(!svc.auto_start);
    }
}
