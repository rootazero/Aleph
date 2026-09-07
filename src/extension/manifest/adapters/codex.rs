//! Codex CLI manifest adapter
//! Detects .codex-plugin/plugin.json format

use std::path::Path;

use anyhow::Result;

use crate::extension::capability::{CapabilityDeclaration, CapabilitySource, SourceFormat};
use crate::extension::manifest::adapter::{AdapterOutput, ManifestAdapter};
use crate::extension::manifest::parsers;
use crate::extension::registry::SkillRegistration;
use crate::extension::types::PluginOrigin;

pub struct CodexAdapter;

impl ManifestAdapter for CodexAdapter {
    fn detect(&self, dir: &Path) -> bool {
        dir.join(".codex-plugin/plugin.json").exists()
    }

    fn parse(&self, dir: &Path) -> Result<AdapterOutput> {
        let manifest_path = dir.join(".codex-plugin/plugin.json");
        let content = std::fs::read_to_string(&manifest_path)?;
        let json: serde_json::Value = serde_json::from_str(&content)?;

        // Sanitize the declared name into a valid plugin id so a third-party
        // manifest cannot claim an arbitrary/colliding id (e.g. "network",
        // "../x", "Other/Name") as the capability-registry key.
        let raw_id = json
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| {
                dir.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
            });
        let mut plugin_id = crate::extension::manifest::sanitize_plugin_id(raw_id);
        if plugin_id.is_empty() {
            plugin_id = "unknown".to_string();
        }

        let mut caps = vec![];

        // Codex supports: skills, hooks, mcpServers
        if let Some(skills_path) = json.get("skills").and_then(|v| v.as_str()) {
            parsers::extend_scanned(
                &mut caps,
                "skills",
                dir,
                parsers::parse_skills_dir(dir, skills_path, &plugin_id),
            );
        }
        if let Some(hooks_path) = json.get("hooks").and_then(|v| v.as_str()) {
            parsers::extend_scanned(
                &mut caps,
                "hooks",
                dir,
                parsers::parse_hooks_file(dir, hooks_path, &plugin_id),
            );
        }
        if let Some(mcp_path) = json.get("mcpServers").and_then(|v| v.as_str()) {
            parsers::extend_scanned(
                &mut caps,
                "mcp",
                dir,
                parsers::parse_mcp_config_file(dir, mcp_path, &plugin_id),
            );
        }

        // Codex-specific: instructions field → skill
        if let Some(instructions) = json.get("instructions").and_then(|v| v.as_str()) {
            caps.push(CapabilityDeclaration::Skill(SkillRegistration {
                name: "codex-instructions".to_string(),
                description: "Codex plugin instructions".to_string(),
                content: instructions.to_string(),
                triggers: vec![],
                category: Some("instructions".to_string()),
                plugin_id: plugin_id.clone(),
                ..Default::default()
            }));
        }

        // Scan default directories when manifest fields are absent
        if json.get("skills").is_none() && dir.join("skills").is_dir() {
            parsers::extend_scanned(
                &mut caps,
                "skills",
                dir,
                parsers::parse_skills_dir(dir, "skills", &plugin_id),
            );
        }
        if json.get("hooks").is_none() {
            let hooks_json = dir.join("hooks").join("hooks.json");
            if hooks_json.exists() {
                parsers::extend_scanned(
                    &mut caps,
                    "hooks",
                    dir,
                    parsers::parse_hooks_file(dir, "hooks/hooks.json", &plugin_id),
                );
            }
        }
        if json.get("mcpServers").is_none() && dir.join(".mcp.json").exists() {
            parsers::extend_scanned(
                &mut caps,
                "mcp",
                dir,
                parsers::parse_mcp_config_file(dir, ".mcp.json", &plugin_id),
            );
        }

        // Handle Codex `apps` field: log and add metadata note
        if let Some(apps) = json.get("apps") {
            tracing::debug!(plugin_id = %plugin_id, "Codex plugin has 'apps' field: {:?}", apps);
        }

        Ok(AdapterOutput {
            plugin_id: plugin_id.clone(),
            name: json.get("name").and_then(|v| v.as_str()).map(String::from),
            version: json
                .get("version")
                .and_then(|v| v.as_str())
                .map(String::from),
            description: json.get("description").and_then(|v| v.as_str()).map(|d| {
                if json.get("apps").is_some() {
                    format!("{d} [has Codex apps]")
                } else {
                    d.to_string()
                }
            }),
            capabilities: caps,
            source: CapabilitySource {
                plugin_id,
                origin: PluginOrigin::Global,
                format: SourceFormat::Codex,
            },
            permissions: vec![],
        })
    }

    fn format_name(&self) -> &str {
        "Codex CLI"
    }

    fn priority(&self) -> i32 {
        80
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_codex_detect_positive() {
        let dir = tempdir().unwrap();
        let codex_dir = dir.path().join(".codex-plugin");
        fs::create_dir_all(&codex_dir).unwrap();
        fs::write(codex_dir.join("plugin.json"), r#"{"name": "test"}"#).unwrap();

        let adapter = CodexAdapter;
        assert!(adapter.detect(dir.path()));
    }

    #[test]
    fn test_codex_detect_negative() {
        let dir = tempdir().unwrap();
        let adapter = CodexAdapter;
        assert!(!adapter.detect(dir.path()));
    }

    #[test]
    fn test_codex_parse_minimal() {
        let dir = tempdir().unwrap();
        let codex_dir = dir.path().join(".codex-plugin");
        fs::create_dir_all(&codex_dir).unwrap();
        fs::write(
            codex_dir.join("plugin.json"),
            r#"{"name": "my-codex-plugin", "version": "1.0.0", "description": "A Codex plugin"}"#,
        )
        .unwrap();

        let adapter = CodexAdapter;
        let output = adapter.parse(dir.path()).unwrap();

        assert_eq!(output.plugin_id, "my-codex-plugin");
        assert_eq!(output.name, Some("my-codex-plugin".to_string()));
        assert_eq!(output.version, Some("1.0.0".to_string()));
        assert_eq!(output.description, Some("A Codex plugin".to_string()));
        assert_eq!(output.source.format, SourceFormat::Codex);
    }

    #[test]
    fn test_codex_parse_with_instructions() {
        let dir = tempdir().unwrap();
        let codex_dir = dir.path().join(".codex-plugin");
        fs::create_dir_all(&codex_dir).unwrap();
        fs::write(
            codex_dir.join("plugin.json"),
            r#"{"name": "inst-plugin", "instructions": "Always be helpful"}"#,
        )
        .unwrap();

        let adapter = CodexAdapter;
        let output = adapter.parse(dir.path()).unwrap();

        assert_eq!(output.capabilities.len(), 1);
        match &output.capabilities[0] {
            CapabilityDeclaration::Skill(s) => {
                assert_eq!(s.name, "codex-instructions");
                assert_eq!(s.content, "Always be helpful");
            }
            other => panic!("Expected Skill, got {:?}", other),
        }
    }

    #[test]
    fn test_codex_priority() {
        assert_eq!(CodexAdapter.priority(), 80);
    }
}
