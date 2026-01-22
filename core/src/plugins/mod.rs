//! Claude Code Compatible Plugin System
//!
//! This module provides a plugin system that is fully compatible with Claude Code CLI plugins.
//! Plugins can extend Aether's capabilities with custom skills, hooks, agents, and MCP servers.
//!
//! # Plugin Structure
//!
//! A Claude Code compatible plugin has the following structure:
//!
//! ```text
//! plugin-root/
//! ├── .claude-plugin/
//! │   └── plugin.json        # Required manifest
//! ├── commands/              # User-triggered commands
//! │   └── hello/
//! │       └── SKILL.md
//! ├── skills/                # AI-invocable skills
//! │   └── code-review/
//! │       └── SKILL.md
//! ├── agents/                # Custom agents
//! │   └── reviewer/
//! │       └── agent.md
//! ├── hooks/
//! │   └── hooks.json         # Event hooks
//! └── .mcp.json              # MCP server configurations
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use aethecore::plugins::{PluginManager, default_plugins_dir};
//!
//! // Create plugin manager
//! let mut manager = PluginManager::new(default_plugins_dir());
//!
//! // Load all plugins
//! manager.load_all().await?;
//!
//! // List plugins
//! for plugin in manager.list_plugins() {
//!     println!("{}: {} skills", plugin.name, plugin.skills_count);
//! }
//!
//! // Execute a skill
//! let result = manager.execute_skill("my-plugin", "hello", "World").await?;
//! ```

pub mod claude_md;
pub mod components;
pub mod error;
pub mod integrator;
pub mod loader;
pub mod manifest;
pub mod registry;
pub mod scanner;
pub mod types;

// Re-exports
pub use error::{PluginError, PluginResult};
pub use loader::{load_plugin, PluginLoader};
pub use manifest::{parse_manifest, validate_manifest};
pub use registry::{default_state_path, PluginRegistry};
pub use scanner::{default_plugins_dir, is_valid_plugin_dir, PluginScanner};
pub use types::{
    // Core types
    LoadedPlugin,
    PluginAgent,
    PluginInfo,
    PluginManifest,
    PluginSkill,
    SkillType,
    // Hook types
    HookAction,
    HookEvent,
    HookMatcher,
    PluginHooksConfig,
    // MCP types
    PluginMcpConfig,
    PluginMcpServer,
    // State types
    PluginState,
    PluginStateFile,
};

// Component re-exports
pub use components::skill::substitute_arguments;
pub use components::hook::{matches_pattern, substitute_variables};

// CLAUDE.md re-exports
pub use claude_md::{ClaudeMdLoader, ClaudeMdSummary};

// Integrator re-exports
pub use integrator::{
    build_skill_instructions, build_skill_prompt, plugin_agent_to_agent_def,
    register_plugin_agents, resolve_mcp_command, HookContext, HookExecutor, HookResult,
    PluginHookHandler,
};

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::runtimes::RuntimeRegistry;

/// Plugin Manager - main entry point for the plugin system
///
/// Manages the lifecycle of plugins: discovery, loading, registration, and integration.
pub struct PluginManager {
    scanner: PluginScanner,
    loader: PluginLoader,
    registry: PluginRegistry,
    runtime: Option<Arc<RuntimeRegistry>>,
}

impl std::fmt::Debug for PluginManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginManager")
            .field("scanner", &self.scanner)
            .field("loader", &self.loader)
            .field("registry", &self.registry)
            .field("runtime", &self.runtime.is_some())
            .finish()
    }
}

impl PluginManager {
    /// Create a new plugin manager with default paths
    pub fn new(plugins_dir: PathBuf) -> Self {
        let state_path = plugins_dir
            .parent()
            .map(|p| p.join("plugins.json"))
            .unwrap_or_else(|| plugins_dir.join("plugins.json"));

        Self {
            scanner: PluginScanner::new(plugins_dir),
            loader: PluginLoader::new(),
            registry: PluginRegistry::new(state_path),
            runtime: None,
        }
    }

    /// Create with custom state path
    pub fn with_state_path(plugins_dir: PathBuf, state_path: PathBuf) -> Self {
        Self {
            scanner: PluginScanner::new(plugins_dir),
            loader: PluginLoader::new(),
            registry: PluginRegistry::new(state_path),
            runtime: None,
        }
    }

    /// Set the runtime registry for MCP server integration
    pub fn with_runtime(mut self, runtime: Arc<RuntimeRegistry>) -> Self {
        self.runtime = Some(runtime);
        self
    }

    /// Add development plugin paths
    pub fn with_dev_paths(mut self, paths: Vec<PathBuf>) -> Self {
        self.scanner = self.scanner.with_dev_paths(paths);
        self
    }

    /// Load all plugins from the plugins directory
    pub fn load_all(&mut self) -> PluginResult<Vec<PluginInfo>> {
        let plugin_paths = self.scanner.scan()?;
        let mut loaded = Vec::new();

        for path in plugin_paths {
            match self.load_plugin_internal(&path) {
                Ok(info) => loaded.push(info),
                Err(e) => {
                    tracing::error!("Failed to load plugin from {:?}: {}", path, e);
                }
            }
        }

        tracing::info!("Loaded {} plugins", loaded.len());
        Ok(loaded)
    }

    /// Load a single plugin (useful for development)
    pub fn load_plugin(&mut self, path: &Path) -> PluginResult<PluginInfo> {
        let validated_path = self.scanner.scan_single(path)?;
        self.load_plugin_internal(&validated_path)
    }

    /// Internal plugin loading
    fn load_plugin_internal(&mut self, path: &Path) -> PluginResult<PluginInfo> {
        let plugin = self.loader.load(path)?;
        let info = PluginInfo::from(&plugin);
        self.registry.register(plugin)?;
        Ok(info)
    }

    /// Unload a plugin
    pub fn unload_plugin(&mut self, name: &str) -> PluginResult<()> {
        self.registry.unregister(name)?;
        tracing::info!("Unloaded plugin: {}", name);
        Ok(())
    }

    /// Enable or disable a plugin
    pub fn set_enabled(&mut self, name: &str, enabled: bool) -> PluginResult<()> {
        self.registry.set_enabled(name, enabled)
    }

    /// List all loaded plugins
    pub fn list_plugins(&self) -> Vec<PluginInfo> {
        self.registry.list()
    }

    /// Get a specific plugin
    pub fn get_plugin(&self, name: &str) -> Option<LoadedPlugin> {
        self.registry.get(name)
    }

    /// Get all skills from enabled plugins
    pub fn get_all_skills(&self) -> Vec<PluginSkill> {
        self.registry.get_all_skills()
    }

    /// Get auto-invocable skills (for LLM prompt injection)
    pub fn get_auto_invocable_skills(&self) -> Vec<PluginSkill> {
        self.registry.get_auto_invocable_skills()
    }

    /// Get a specific skill
    pub fn get_skill(&self, plugin: &str, skill: &str) -> Option<PluginSkill> {
        self.registry.get_skill(plugin, skill)
    }

    /// Get all agents from enabled plugins
    pub fn get_all_agents(&self) -> Vec<PluginAgent> {
        self.registry.get_all_agents()
    }

    /// Get the plugins directory
    pub fn plugins_dir(&self) -> &Path {
        self.scanner.plugins_dir()
    }

    /// Get the registry (for integration)
    pub fn registry(&self) -> &PluginRegistry {
        &self.registry
    }

    /// Get the runtime registry (for MCP integration)
    pub fn runtime(&self) -> Option<&Arc<RuntimeRegistry>> {
        self.runtime.as_ref()
    }

    /// Execute a plugin skill
    ///
    /// This substitutes $ARGUMENTS and returns the processed skill content.
    pub fn prepare_skill_execution(
        &self,
        plugin: &str,
        skill: &str,
        arguments: &str,
    ) -> PluginResult<String> {
        let skill_def = self
            .registry
            .get_skill(plugin, skill)
            .ok_or_else(|| PluginError::SkillNotFound {
                plugin: plugin.to_string(),
                skill: skill.to_string(),
            })?;

        Ok(substitute_arguments(&skill_def.content, arguments))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_plugin(dir: &Path, name: &str) -> PathBuf {
        let plugin_dir = dir.join(name);
        fs::create_dir_all(plugin_dir.join(".claude-plugin")).unwrap();
        fs::write(
            plugin_dir.join(".claude-plugin").join("plugin.json"),
            format!(r#"{{"name": "{}", "version": "1.0.0"}}"#, name),
        )
        .unwrap();

        // Add a command
        let cmd_dir = plugin_dir.join("commands").join("hello");
        fs::create_dir_all(&cmd_dir).unwrap();
        fs::write(
            cmd_dir.join("SKILL.md"),
            r#"---
description: Say hello
---

Say hello to $ARGUMENTS"#,
        )
        .unwrap();

        plugin_dir
    }

    #[test]
    fn test_plugin_manager_load_all() {
        let temp = TempDir::new().unwrap();
        let plugins_dir = temp.path().join("plugins");
        fs::create_dir(&plugins_dir).unwrap();

        create_test_plugin(&plugins_dir, "plugin-a");
        create_test_plugin(&plugins_dir, "plugin-b");

        let mut manager = PluginManager::new(plugins_dir);
        let loaded = manager.load_all().unwrap();

        assert_eq!(loaded.len(), 2);
    }

    #[test]
    fn test_plugin_manager_get_skills() {
        let temp = TempDir::new().unwrap();
        let plugins_dir = temp.path().join("plugins");
        fs::create_dir(&plugins_dir).unwrap();

        create_test_plugin(&plugins_dir, "my-plugin");

        let mut manager = PluginManager::new(plugins_dir);
        manager.load_all().unwrap();

        let skills = manager.get_all_skills();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].qualified_name(), "my-plugin:hello");
    }

    #[test]
    fn test_plugin_manager_enable_disable() {
        let temp = TempDir::new().unwrap();
        let plugins_dir = temp.path().join("plugins");
        fs::create_dir(&plugins_dir).unwrap();

        create_test_plugin(&plugins_dir, "my-plugin");

        let mut manager = PluginManager::new(plugins_dir);
        manager.load_all().unwrap();

        // Disable plugin
        manager.set_enabled("my-plugin", false).unwrap();
        let plugin = manager.get_plugin("my-plugin").unwrap();
        assert!(!plugin.enabled);

        // Skills should not be returned for disabled plugins
        let skills = manager.get_all_skills();
        assert!(skills.is_empty());
    }

    #[test]
    fn test_prepare_skill_execution() {
        let temp = TempDir::new().unwrap();
        let plugins_dir = temp.path().join("plugins");
        fs::create_dir(&plugins_dir).unwrap();

        create_test_plugin(&plugins_dir, "my-plugin");

        let mut manager = PluginManager::new(plugins_dir);
        manager.load_all().unwrap();

        let content = manager
            .prepare_skill_execution("my-plugin", "hello", "World")
            .unwrap();
        assert!(content.contains("Say hello to World"));
    }
}
