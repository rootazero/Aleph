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

use anyhow::Result;

use crate::extension::capability::{CapabilitySource, SourceFormat};
use crate::extension::manifest::adapter::{AdapterOutput, ManifestAdapter};
use crate::extension::manifest::toml_types::AlephPluginToml;
use crate::extension::manifest::{convert_permissions, parsers, ALEPH_PLUGIN_TOML};
use crate::extension::types::PluginOrigin;

pub struct AlephTomlAdapter;

impl ManifestAdapter for AlephTomlAdapter {
    fn detect(&self, plugin_dir: &Path) -> bool {
        plugin_dir.join(ALEPH_PLUGIN_TOML).exists()
    }

    fn parse(&self, plugin_dir: &Path) -> Result<AdapterOutput> {
        let toml_path = plugin_dir.join(ALEPH_PLUGIN_TOML);
        let content = std::fs::read_to_string(&toml_path)?;
        let raw: AlephPluginToml = toml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("aleph.plugin.toml parse error: {}", e))?;

        let plugin_id = raw.plugin.id.clone();
        let mut capabilities = Vec::new();

        // Component directories (same convention as auto-discover).
        if plugin_dir.join("skills").is_dir() {
            capabilities.extend(parsers::parse_skills_dir(plugin_dir, "skills", &plugin_id)?);
        }
        // Commands are merged into skills following the OpenClaw convention.
        if plugin_dir.join("commands").is_dir() {
            capabilities.extend(parsers::parse_skills_dir(plugin_dir, "commands", &plugin_id)?);
        }
        if plugin_dir.join("agents").is_dir() {
            capabilities.extend(parsers::parse_agents_dir(plugin_dir, "agents", &plugin_id)?);
        }
        for hooks_path in &["hooks/hooks.json", "hooks.json"] {
            if plugin_dir.join(hooks_path).exists() {
                capabilities.extend(parsers::parse_hooks_file(plugin_dir, hooks_path, &plugin_id)?);
                break;
            }
        }
        if plugin_dir.join(".mcp.json").exists() {
            capabilities.extend(parsers::parse_mcp_config_file(
                plugin_dir,
                ".mcp.json",
                &plugin_id,
            )?);
        }

        // System prompt declared in [prompt].
        if let Some(ref prompt) = raw.prompt {
            match parsers::parse_v2_prompt(plugin_dir, prompt, &plugin_id) {
                Ok(cap) => capabilities.push(cap),
                Err(e) => tracing::debug!("Failed to parse [prompt] for {}: {}", plugin_id, e),
            }
        }

        // Tool instruction files declared in [[tools]].
        if !raw.tools.is_empty() {
            match parsers::parse_v2_tool_prompts(plugin_dir, &raw.tools, &plugin_id) {
                Ok(caps) => capabilities.extend(caps),
                Err(e) => tracing::debug!("Failed to parse [[tools]] for {}: {}", plugin_id, e),
            }
        }

        Ok(AdapterOutput {
            plugin_id: plugin_id.clone(),
            name: raw.plugin.name.clone(),
            version: raw.plugin.version.clone(),
            description: raw.plugin.description.clone(),
            capabilities,
            source: CapabilitySource {
                plugin_id,
                origin: PluginOrigin::Global,
                format: SourceFormat::AlephToml,
            },
            permissions: convert_permissions(&raw.permissions),
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
        fs::write(
            dir.path().join(ALEPH_PLUGIN_TOML),
            "[plugin]\nid = \"x\"\n",
        )
        .unwrap();
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
}
