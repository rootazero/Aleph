//! Auto-discover adapter
//!
//! Fallback adapter that detects plugins by well-known directory structure
//! (skills/, commands/, agents/, hooks.json, .mcp.json) without any manifest file.
//! Lowest priority (-100) — only matches if no other adapter claimed the directory.

use std::path::Path;

use anyhow::Result;

use crate::extension::capability::{CapabilitySource, SourceFormat};
use crate::extension::manifest::adapter::{AdapterOutput, ManifestAdapter};
use crate::extension::manifest::parsers;
use crate::extension::types::PluginOrigin;

pub struct AutoDiscoverAdapter;

impl ManifestAdapter for AutoDiscoverAdapter {
    fn detect(&self, dir: &Path) -> bool {
        dir.join("skills").is_dir()
            || dir.join("commands").is_dir()
            || dir.join("agents").is_dir()
            || dir.join("hooks").exists()
            || dir.join("hooks.json").exists()
            || dir.join(".mcp.json").exists()
    }

    fn parse(&self, dir: &Path) -> Result<AdapterOutput> {
        let raw_id = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        let mut plugin_id = crate::extension::manifest::sanitize_plugin_id(raw_id);
        if plugin_id.is_empty() {
            plugin_id = "unknown".to_string();
        }
        let mut caps = vec![];

        if dir.join("skills").is_dir() {
            parsers::extend_scanned(
                &mut caps,
                "skills",
                dir,
                parsers::parse_skills_dir(dir, "skills", &plugin_id),
            );
        }
        // Commands are merged into skills following OpenClaw convention
        if dir.join("commands").is_dir() {
            parsers::extend_scanned(
                &mut caps,
                "commands",
                dir,
                parsers::parse_commands_dir(dir, "commands", &plugin_id),
            );
        }
        if dir.join("agents").is_dir() {
            parsers::extend_scanned(
                &mut caps,
                "agents",
                dir,
                parsers::parse_agents_dir(dir, "agents", &plugin_id),
            );
        }
        // hooks can be hooks/hooks.json or hooks.json — first one that parses
        // wins. The fallback to the second path on a failed parse is kept
        // deliberately, but a failure is now logged: "hooks/hooks.json is
        // malformed" and "there is no hooks/hooks.json" produced the same
        // silence before, and only one of them is the author's fault.
        for hooks_path in &["hooks/hooks.json", "hooks.json"] {
            if !dir.join(hooks_path).exists() {
                continue;
            }
            match parsers::parse_hooks_file(dir, hooks_path, &plugin_id) {
                Ok(h) => {
                    caps.extend(h);
                    break;
                }
                Err(e) => tracing::warn!(
                    dir = %dir.display(),
                    hooks_path = %hooks_path,
                    error = %e,
                    "hooks file failed to parse; trying the next candidate path"
                ),
            }
        }
        if dir.join(".mcp.json").exists() {
            parsers::extend_scanned(
                &mut caps,
                "mcp",
                dir,
                parsers::parse_mcp_config_file(dir, ".mcp.json", &plugin_id),
            );
        }

        Ok(AdapterOutput {
            plugin_id: plugin_id.clone(),
            name: None,
            version: None,
            description: None,
            capabilities: caps,
            source: CapabilitySource {
                plugin_id,
                origin: PluginOrigin::Global,
                format: SourceFormat::AutoDiscovered,
            },
            permissions: vec![],
        })
    }

    fn format_name(&self) -> &str {
        "Auto-discovered"
    }

    fn priority(&self) -> i32 {
        -100
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::capability::CapabilityDeclaration;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_auto_detect_skills_dir() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("skills")).unwrap();

        assert!(AutoDiscoverAdapter.detect(dir.path()));
    }

    #[test]
    fn test_auto_detect_commands_dir() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("commands")).unwrap();

        assert!(AutoDiscoverAdapter.detect(dir.path()));
    }

    #[test]
    fn test_auto_detect_agents_dir() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("agents")).unwrap();

        assert!(AutoDiscoverAdapter.detect(dir.path()));
    }

    #[test]
    fn test_auto_detect_mcp_json() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(".mcp.json"), "{}").unwrap();

        assert!(AutoDiscoverAdapter.detect(dir.path()));
    }

    #[test]
    fn test_auto_detect_hooks_dir() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("hooks")).unwrap();

        assert!(AutoDiscoverAdapter.detect(dir.path()));
    }

    #[test]
    fn test_auto_detect_hooks_json() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("hooks.json"), "{}").unwrap();

        assert!(AutoDiscoverAdapter.detect(dir.path()));
    }

    #[test]
    fn test_auto_detect_negative() {
        let dir = tempdir().unwrap();
        assert!(!AutoDiscoverAdapter.detect(dir.path()));
    }

    #[test]
    fn test_auto_parse_skills() {
        let dir = tempdir().unwrap();
        let skill_dir = dir.path().join("skills").join("hello");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: hello\ndescription: Say hello\n---\nHello!",
        )
        .unwrap();

        let output = AutoDiscoverAdapter.parse(dir.path()).unwrap();

        assert_eq!(output.capabilities.len(), 1);
        match &output.capabilities[0] {
            CapabilityDeclaration::Skill(s) => {
                assert_eq!(s.name, "hello");
            }
            other => panic!("Expected Skill, got {:?}", other),
        }
        assert_eq!(output.source.format, SourceFormat::AutoDiscovered);
        assert!(output.name.is_none());
    }

    #[test]
    fn test_auto_parse_empty_dirs() {
        let dir = tempdir().unwrap();
        // Create empty skills dir — detects but produces no capabilities
        fs::create_dir_all(dir.path().join("skills")).unwrap();

        let output = AutoDiscoverAdapter.parse(dir.path()).unwrap();
        assert!(output.capabilities.is_empty());
    }

    #[test]
    fn test_auto_priority() {
        assert_eq!(AutoDiscoverAdapter.priority(), -100);
    }
}
