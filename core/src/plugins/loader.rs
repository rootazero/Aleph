//! Plugin loader
//!
//! Loads plugin components from disk.

use std::path::Path;

use crate::plugins::components::{AgentLoader, HookLoader, McpLoader, SkillLoader};
use crate::plugins::error::PluginResult;
use crate::plugins::manifest::parse_manifest;
use crate::plugins::types::{
    LoadedPlugin, PluginAgent, PluginHooksConfig, PluginManifest, PluginMcpConfig, PluginSkill,
};

/// Plugin loader
#[derive(Debug, Default)]
pub struct PluginLoader {
    skill_loader: SkillLoader,
    hook_loader: HookLoader,
    agent_loader: AgentLoader,
    mcp_loader: McpLoader,
}

impl PluginLoader {
    /// Create a new plugin loader
    pub fn new() -> Self {
        Self::default()
    }

    /// Load a plugin from a directory
    pub fn load(&self, plugin_dir: &Path) -> PluginResult<LoadedPlugin> {
        tracing::info!("Loading plugin from: {:?}", plugin_dir);

        // Parse manifest
        let manifest = parse_manifest(plugin_dir)?;
        let plugin_name = manifest.name.clone();

        tracing::debug!("Parsed manifest for plugin: {}", plugin_name);

        // Load components
        let skills = self.load_skills(plugin_dir, &manifest)?;
        let hooks = self.load_hooks(plugin_dir, &manifest)?;
        let agents = self.load_agents(plugin_dir, &manifest)?;
        let mcp_servers = self.load_mcp_servers(plugin_dir, &manifest)?;

        tracing::info!(
            "Loaded plugin '{}': {} skills, {} hook events, {} agents, {} MCP servers",
            plugin_name,
            skills.len(),
            hooks.hooks.len(),
            agents.len(),
            mcp_servers.mcp_servers.len()
        );

        Ok(LoadedPlugin {
            manifest,
            path: plugin_dir.to_path_buf(),
            enabled: true, // Default to enabled, will be overridden by state
            skills,
            hooks,
            agents,
            mcp_servers,
        })
    }

    /// Load skills from commands/ and skills/ directories
    fn load_skills(
        &self,
        plugin_dir: &Path,
        manifest: &PluginManifest,
    ) -> PluginResult<Vec<PluginSkill>> {
        let mut skills = Vec::new();

        // Load commands
        let commands_dir = manifest
            .commands
            .as_ref()
            .map(|p| plugin_dir.join(p))
            .unwrap_or_else(|| plugin_dir.join("commands"));

        if commands_dir.exists() {
            let commands = self
                .skill_loader
                .load_commands(&commands_dir, &manifest.name)?;
            tracing::debug!(
                "Loaded {} commands from {:?}",
                commands.len(),
                commands_dir
            );
            skills.extend(commands);
        }

        // Load skills
        let skills_dir = manifest
            .skills
            .as_ref()
            .map(|p| plugin_dir.join(p))
            .unwrap_or_else(|| plugin_dir.join("skills"));

        if skills_dir.exists() {
            let agent_skills = self
                .skill_loader
                .load_skills(&skills_dir, &manifest.name)?;
            tracing::debug!(
                "Loaded {} skills from {:?}",
                agent_skills.len(),
                skills_dir
            );
            skills.extend(agent_skills);
        }

        Ok(skills)
    }

    /// Load hooks from hooks/hooks.json
    fn load_hooks(
        &self,
        plugin_dir: &Path,
        manifest: &PluginManifest,
    ) -> PluginResult<PluginHooksConfig> {
        let hooks_path = manifest
            .hooks
            .as_ref()
            .map(|p| plugin_dir.join(p))
            .unwrap_or_else(|| plugin_dir.join("hooks").join("hooks.json"));

        if hooks_path.exists() {
            self.hook_loader.load(&hooks_path)
        } else {
            Ok(PluginHooksConfig::default())
        }
    }

    /// Load agents from agents/ directory
    fn load_agents(
        &self,
        plugin_dir: &Path,
        manifest: &PluginManifest,
    ) -> PluginResult<Vec<PluginAgent>> {
        let agents_dir = manifest
            .agents
            .as_ref()
            .map(|p| plugin_dir.join(p))
            .unwrap_or_else(|| plugin_dir.join("agents"));

        if agents_dir.exists() {
            self.agent_loader.load_all(&agents_dir, &manifest.name)
        } else {
            Ok(Vec::new())
        }
    }

    /// Load MCP server configurations from .mcp.json
    fn load_mcp_servers(
        &self,
        plugin_dir: &Path,
        manifest: &PluginManifest,
    ) -> PluginResult<PluginMcpConfig> {
        let mcp_path = manifest
            .mcp_servers
            .as_ref()
            .map(|p| plugin_dir.join(p))
            .unwrap_or_else(|| plugin_dir.join(".mcp.json"));

        if mcp_path.exists() {
            self.mcp_loader.load(&mcp_path)
        } else {
            Ok(PluginMcpConfig::default())
        }
    }
}

/// Load a plugin from a directory (convenience function)
pub fn load_plugin(plugin_dir: &Path) -> PluginResult<LoadedPlugin> {
    PluginLoader::new().load(plugin_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_minimal_plugin(temp: &TempDir) -> std::path::PathBuf {
        let plugin_dir = temp.path().join("test-plugin");
        fs::create_dir_all(plugin_dir.join(".claude-plugin")).unwrap();
        fs::write(
            plugin_dir.join(".claude-plugin").join("plugin.json"),
            r#"{"name": "test-plugin", "version": "1.0.0"}"#,
        )
        .unwrap();
        plugin_dir
    }

    #[test]
    fn test_load_minimal_plugin() {
        let temp = TempDir::new().unwrap();
        let plugin_dir = create_minimal_plugin(&temp);

        let loader = PluginLoader::new();
        let plugin = loader.load(&plugin_dir).unwrap();

        assert_eq!(plugin.manifest.name, "test-plugin");
        assert_eq!(plugin.manifest.version, Some("1.0.0".to_string()));
        assert!(plugin.skills.is_empty());
        assert!(plugin.hooks.hooks.is_empty());
        assert!(plugin.agents.is_empty());
    }

    #[test]
    fn test_load_plugin_with_skills() {
        let temp = TempDir::new().unwrap();
        let plugin_dir = create_minimal_plugin(&temp);

        // Create a command
        let cmd_dir = plugin_dir.join("commands").join("hello");
        fs::create_dir_all(&cmd_dir).unwrap();
        fs::write(
            cmd_dir.join("SKILL.md"),
            r#"---
description: Say hello
---

# Hello Command

Say hello to $ARGUMENTS
"#,
        )
        .unwrap();

        let loader = PluginLoader::new();
        let plugin = loader.load(&plugin_dir).unwrap();

        assert_eq!(plugin.skills.len(), 1);
        assert_eq!(plugin.skills[0].skill_name, "hello");
        assert_eq!(plugin.skills[0].description, "Say hello");
    }

    #[test]
    fn test_load_nonexistent_plugin() {
        let loader = PluginLoader::new();
        let result = loader.load(Path::new("/nonexistent/path"));

        assert!(result.is_err());
    }
}
