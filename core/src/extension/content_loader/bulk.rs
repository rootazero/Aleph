//! Bulk loading - coordinates loading all extension components

use super::*;

impl ContentLoader {
    /// Load all discovered skills, commands, agents, plugins, and hooks.
    ///
    /// This is the primary coordination method for the loading phase.
    /// Called by `ExtensionManager::load_all()` as a single delegation point,
    /// keeping the manager a thin facade over the loader.
    ///
    /// Returns a `LoadResult` containing all loaded components. The caller
    /// (ExtensionManager) is responsible for storing them.
    pub async fn load_all(
        &self,
        discovery: &DiscoveryManager,
    ) -> ExtensionResult<LoadResult> {
        let mut result = LoadResult::default();

        // 1. Load skills
        let skill_dirs = discovery.discover_skill_dirs()?;
        for dir in skill_dirs {
            match self.load_skill(&dir.path).await {
                Ok(skill) => {
                    result.skills.push(skill);
                    result.summary.skills_loaded += 1;
                }
                Err(e) => {
                    tracing::debug!("Failed to load skill from {:?}: {}", dir.path, e);
                    result.summary.errors.push(format!("{}: {}", dir.path.display(), e));
                }
            }
        }

        // 2. Load commands
        let command_dirs = discovery.discover_command_dirs()?;
        for dir in command_dirs {
            match self.load_command(&dir.path).await {
                Ok(cmd) => {
                    result.commands.push(cmd);
                    result.summary.commands_loaded += 1;
                }
                Err(e) => {
                    tracing::debug!("Failed to load command from {:?}: {}", dir.path, e);
                    result.summary.errors.push(format!("{}: {}", dir.path.display(), e));
                }
            }
        }

        // 3. Load agents
        let agent_dirs = discovery.discover_agent_dirs()?;
        for dir in agent_dirs {
            match self.load_agent(&dir.path).await {
                Ok(agent) => {
                    result.agents.push(agent);
                    result.summary.agents_loaded += 1;
                }
                Err(e) => {
                    tracing::debug!("Failed to load agent from {:?}: {}", dir.path, e);
                    result.summary.errors.push(format!("{}: {}", dir.path.display(), e));
                }
            }
        }

        // 4. Load plugins (including their embedded skills, commands, agents, and hooks)
        let plugin_dirs = discovery.discover_plugins()?;
        for dir in plugin_dirs {
            match self.load_plugin(&dir.path).await {
                Ok(plugin) => {
                    // Collect plugin hooks
                    for hook in &plugin.hooks {
                        result.hooks.push(hook.clone());
                        result.summary.hooks_loaded += 1;
                    }

                    // Collect plugin components
                    for skill in &plugin.skills {
                        result.skills.push(skill.clone());
                        result.summary.skills_loaded += 1;
                    }
                    for cmd in &plugin.commands {
                        result.commands.push(cmd.clone());
                        result.summary.commands_loaded += 1;
                    }
                    for agent in &plugin.agents {
                        result.agents.push(agent.clone());
                        result.summary.agents_loaded += 1;
                    }
                    result.summary.plugins_loaded += 1;
                }
                Err(e) => {
                    tracing::debug!("Failed to load plugin from {:?}: {}", dir.path, e);
                    result.summary.errors.push(format!("{}: {}", dir.path.display(), e));
                }
            }
        }

        // 5. Runtime plugins (Node.js, WASM) are discovered separately via PluginLoader.
        // They register with PluginRegistry and are handled through separate API
        // calls on ExtensionManager.

        tracing::info!(
            "Extension loading complete: {} skills, {} commands, {} agents, {} plugins, {} hooks",
            result.summary.skills_loaded,
            result.summary.commands_loaded,
            result.summary.agents_loaded,
            result.summary.plugins_loaded,
            result.summary.hooks_loaded
        );

        Ok(result)
    }
}
