//! Cursor IDE manifest adapter
//! Detects .cursor-plugin/plugin.json, .cursorrules, or .cursor/rules/

use std::path::Path;

use anyhow::Result;

use crate::extension::capability::{CapabilityDeclaration, CapabilitySource, SourceFormat};
use crate::extension::manifest::adapter::{AdapterOutput, ManifestAdapter};
use crate::extension::manifest::parsers;
use crate::extension::registry::SkillRegistration;
use crate::extension::types::PluginOrigin;

pub struct CursorAdapter;

impl ManifestAdapter for CursorAdapter {
    fn detect(&self, dir: &Path) -> bool {
        dir.join(".cursor-plugin/plugin.json").exists()
            || dir.join(".cursorrules").exists()
            || dir.join(".cursor/rules").is_dir()
    }

    fn parse(&self, dir: &Path) -> Result<AdapterOutput> {
        let mut caps = vec![];
        let mut plugin_id = crate::extension::manifest::sanitize_plugin_id(
            dir.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown"),
        );
        if plugin_id.is_empty() {
            plugin_id = "unknown".to_string();
        }
        let mut name = None;
        let mut version = None;
        let mut description = None;

        // Try .cursor-plugin/plugin.json first
        if let Ok(content) = std::fs::read_to_string(dir.join(".cursor-plugin/plugin.json")) {
            match serde_json::from_str::<serde_json::Value>(&content) {
                Err(e) => tracing::warn!(
                    "Malformed .cursor-plugin/plugin.json in {}: {}",
                    dir.display(),
                    e
                ),
                Ok(json) => {
                    if let Some(n) = json.get("name").and_then(|v| v.as_str()) {
                        // Sanitize so a third-party manifest cannot claim an
                        // arbitrary/colliding capability-registry id.
                        let sanitized = crate::extension::manifest::sanitize_plugin_id(n);
                        if !sanitized.is_empty() {
                            plugin_id = sanitized;
                        }
                    }
                    name = json.get("name").and_then(|v| v.as_str()).map(String::from);
                    version = json
                        .get("version")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    description = json
                        .get("description")
                        .and_then(|v| v.as_str())
                        .map(String::from);

                    // Parse standard components
                    if let Some(skills) = json.get("skills").and_then(|v| v.as_str()) {
                        parsers::extend_scanned(
                            &mut caps,
                            "skills",
                            dir,
                            parsers::parse_skills_dir(dir, skills, &plugin_id),
                        );
                    }
                    // Commands are merged into skills following OpenClaw convention
                    if let Some(commands) = json.get("commands").and_then(|v| v.as_str()) {
                        parsers::extend_scanned(
                            &mut caps,
                            "commands",
                            dir,
                            parsers::parse_commands_dir(dir, commands, &plugin_id),
                        );
                    }
                    if let Some(agents) = json.get("agents").and_then(|v| v.as_str()) {
                        parsers::extend_scanned(
                            &mut caps,
                            "agents",
                            dir,
                            parsers::parse_agents_dir(dir, agents, &plugin_id),
                        );
                    }
                    if let Some(hooks) = json.get("hooks").and_then(|v| v.as_str()) {
                        parsers::extend_scanned(
                            &mut caps,
                            "hooks",
                            dir,
                            parsers::parse_hooks_file(dir, hooks, &plugin_id),
                        );
                    }
                    if let Some(mcp) = json.get("mcpServers").and_then(|v| v.as_str()) {
                        parsers::extend_scanned(
                            &mut caps,
                            "mcp",
                            dir,
                            parsers::parse_mcp_config_file(dir, mcp, &plugin_id),
                        );
                    }

                    // Cursor-specific: rules → skills
                    if let Some(rules_path) = json.get("rules").and_then(|v| v.as_str()) {
                        parsers::extend_scanned(
                            &mut caps,
                            "skills",
                            dir,
                            parsers::parse_skills_dir(dir, rules_path, &plugin_id),
                        );
                    }
                }
            }
        }

        // Scan default directories only when NOT already specified in manifest
        let has_manifest = dir.join(".cursor-plugin/plugin.json").exists();
        let json_ref = if has_manifest {
            std::fs::read_to_string(dir.join(".cursor-plugin/plugin.json"))
                .ok()
                .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        } else {
            None
        };
        let json_has =
            |field: &str| -> bool { json_ref.as_ref().and_then(|j| j.get(field)).is_some() };

        if !json_has("skills") && dir.join("skills").is_dir() {
            parsers::extend_scanned(
                &mut caps,
                "skills",
                dir,
                parsers::parse_skills_dir(dir, "skills", &plugin_id),
            );
        }
        // .cursor/commands/ → skills (commands as skills)
        if !json_has("commands") && dir.join(".cursor/commands").is_dir() {
            parsers::extend_scanned(
                &mut caps,
                "commands",
                dir,
                parsers::parse_commands_dir(dir, ".cursor/commands", &plugin_id),
            );
        }
        // .cursor/agents/ → agents
        if !json_has("agents") && !json_has("subagents") && dir.join(".cursor/agents").is_dir() {
            parsers::extend_scanned(
                &mut caps,
                "agents",
                dir,
                parsers::parse_agents_dir(dir, ".cursor/agents", &plugin_id),
            );
        }
        // .cursor/hooks.json → hooks
        if !json_has("hooks") && dir.join(".cursor/hooks.json").exists() {
            parsers::extend_scanned(
                &mut caps,
                "hooks",
                dir,
                parsers::parse_hooks_file(dir, ".cursor/hooks.json", &plugin_id),
            );
        }
        // .mcp.json → MCP servers
        if !json_has("mcpServers") && dir.join(".mcp.json").exists() {
            parsers::extend_scanned(
                &mut caps,
                "mcp",
                dir,
                parsers::parse_mcp_config_file(dir, ".mcp.json", &plugin_id),
            );
        }

        // .cursorrules file → single skill
        let cursorrules_path = dir.join(".cursorrules");
        if cursorrules_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&cursorrules_path) {
                caps.push(CapabilityDeclaration::Skill(SkillRegistration {
                    name: "cursorrules".to_string(),
                    description: "Cursor rules imported as skill".to_string(),
                    content,
                    triggers: vec![],
                    category: Some("rules".to_string()),
                    plugin_id: plugin_id.clone(),
                    ..Default::default()
                }));
            }
        }

        // .cursor/rules/ directory → scan for rule files as skills
        let cursor_rules_dir = dir.join(".cursor/rules");
        if cursor_rules_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&cursor_rules_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let ext = path.extension().and_then(|e| e.to_str());
                    if ext == Some("md") || ext == Some("mdc") {
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            let rule_name = path
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("rule")
                                .to_string();
                            caps.push(CapabilityDeclaration::Skill(SkillRegistration {
                                name: rule_name,
                                description: "Cursor rule imported as skill".to_string(),
                                content,
                                triggers: vec![],
                                category: Some("rules".to_string()),
                                plugin_id: plugin_id.clone(),
                                ..Default::default()
                            }));
                        }
                    }
                }
            }
        }

        Ok(AdapterOutput {
            plugin_id: plugin_id.clone(),
            name,
            version,
            description,
            capabilities: caps,
            source: CapabilitySource {
                plugin_id,
                origin: PluginOrigin::Global,
                format: SourceFormat::Cursor,
            },
            permissions: vec![],
        })
    }

    fn format_name(&self) -> &str {
        "Cursor IDE"
    }

    fn priority(&self) -> i32 {
        70
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_cursor_detect_plugin_json() {
        let dir = tempdir().unwrap();
        let cursor_dir = dir.path().join(".cursor-plugin");
        fs::create_dir_all(&cursor_dir).unwrap();
        fs::write(cursor_dir.join("plugin.json"), r#"{"name": "test"}"#).unwrap();

        assert!(CursorAdapter.detect(dir.path()));
    }

    #[test]
    fn test_cursor_detect_cursorrules() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(".cursorrules"), "some rules").unwrap();

        assert!(CursorAdapter.detect(dir.path()));
    }

    #[test]
    fn test_cursor_detect_rules_dir() {
        let dir = tempdir().unwrap();
        let rules_dir = dir.path().join(".cursor/rules");
        fs::create_dir_all(&rules_dir).unwrap();

        assert!(CursorAdapter.detect(dir.path()));
    }

    #[test]
    fn test_cursor_detect_negative() {
        let dir = tempdir().unwrap();
        assert!(!CursorAdapter.detect(dir.path()));
    }

    #[test]
    fn test_cursor_parse_cursorrules() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(".cursorrules"), "Always use TypeScript").unwrap();

        let output = CursorAdapter.parse(dir.path()).unwrap();

        assert_eq!(output.capabilities.len(), 1);
        match &output.capabilities[0] {
            CapabilityDeclaration::Skill(s) => {
                assert_eq!(s.name, "cursorrules");
                assert_eq!(s.content, "Always use TypeScript");
                assert_eq!(s.category, Some("rules".to_string()));
            }
            other => panic!("Expected Skill, got {:?}", other),
        }
        assert_eq!(output.source.format, SourceFormat::Cursor);
    }

    #[test]
    fn test_cursor_parse_rules_dir() {
        let dir = tempdir().unwrap();
        let rules_dir = dir.path().join(".cursor/rules");
        fs::create_dir_all(&rules_dir).unwrap();
        fs::write(rules_dir.join("style.md"), "Use semicolons").unwrap();
        fs::write(rules_dir.join("lint.mdc"), "No unused vars").unwrap();
        // Non-matching extension should be ignored
        fs::write(rules_dir.join("readme.txt"), "Ignore me").unwrap();

        let output = CursorAdapter.parse(dir.path()).unwrap();

        assert_eq!(output.capabilities.len(), 2);
        for cap in &output.capabilities {
            match cap {
                CapabilityDeclaration::Skill(s) => {
                    assert!(s.name == "style" || s.name == "lint");
                    assert_eq!(s.category, Some("rules".to_string()));
                }
                other => panic!("Expected Skill, got {:?}", other),
            }
        }
    }

    #[test]
    fn test_cursor_parse_plugin_json() {
        let dir = tempdir().unwrap();
        let cursor_dir = dir.path().join(".cursor-plugin");
        fs::create_dir_all(&cursor_dir).unwrap();
        fs::write(
            cursor_dir.join("plugin.json"),
            r#"{"name": "cursor-plugin", "version": "2.0.0", "description": "A Cursor plugin"}"#,
        )
        .unwrap();

        let output = CursorAdapter.parse(dir.path()).unwrap();

        assert_eq!(output.plugin_id, "cursor-plugin");
        assert_eq!(output.name, Some("cursor-plugin".to_string()));
        assert_eq!(output.version, Some("2.0.0".to_string()));
    }

    #[test]
    fn test_cursor_priority() {
        assert_eq!(CursorAdapter.priority(), 70);
    }
}
