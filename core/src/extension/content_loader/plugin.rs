//! Plugin loading logic

use super::*;

impl ContentLoader {
    /// Load a plugin from a directory
    pub async fn load_plugin(&self, path: &Path) -> ExtensionResult<ExtensionPlugin> {
        debug!("Loading plugin from: {:?}", path);

        // Parse manifest
        let manifest_path = path.join(PLUGIN_MANIFEST_DIR).join(PLUGIN_MANIFEST_FILE);
        if !manifest_path.exists() {
            return Err(ExtensionError::invalid_manifest(
                path,
                "Missing .claude-plugin/plugin.json",
            ));
        }

        let manifest: LegacyPluginManifest = parse_plugin_manifest(&manifest_path).await?;

        // Validate plugin name
        validate_plugin_name(&manifest.name)?;

        // Load skills
        let mut skills = Vec::new();
        let skills_dir = manifest
            .skills
            .as_ref()
            .map(|p| path.join(p))
            .unwrap_or_else(|| path.join(SKILLS_DIR));

        if skills_dir.exists() {
            skills.extend(
                self.load_skills_from_dir(&skills_dir, Some(manifest.name.clone()))
                    .await?,
            );
        }

        // Load commands
        let mut commands = Vec::new();
        let commands_dir = manifest
            .commands
            .as_ref()
            .map(|p| path.join(p))
            .unwrap_or_else(|| path.join(COMMANDS_DIR));

        if commands_dir.exists() {
            commands.extend(
                self.load_commands_from_dir(&commands_dir, Some(manifest.name.clone()))
                    .await?,
            );
        }

        // Load agents
        let mut agents = Vec::new();
        let agents_dir = manifest
            .agents
            .as_ref()
            .map(|p| path.join(p))
            .unwrap_or_else(|| path.join(AGENTS_DIR));

        if agents_dir.exists() {
            agents.extend(
                self.load_agents_from_dir(&agents_dir, Some(manifest.name.clone()))
                    .await?,
            );
        }

        // Load hooks
        let hooks = self.load_hooks(path, &manifest, &manifest.name).await?;

        // Load MCP servers
        let mcp_servers = self.load_mcp_servers(path, &manifest).await?;

        let plugin = ExtensionPlugin {
            name: manifest.name,
            version: manifest.version,
            description: manifest.description,
            path: path.to_path_buf(),
            enabled: true,
            skills,
            commands,
            agents,
            hooks,
            mcp_servers,
        };

        debug!("Loaded plugin: {} with {} components", plugin.name, {
            plugin.skills.len() + plugin.commands.len() + plugin.agents.len()
        });

        Ok(plugin)
    }

    /// Load hooks from a plugin
    pub(super) async fn load_hooks(
        &self,
        plugin_path: &Path,
        manifest: &LegacyPluginManifest,
        plugin_name: &str,
    ) -> ExtensionResult<Vec<HookConfig>> {
        let hooks_path = manifest
            .hooks
            .as_ref()
            .map(|p| plugin_path.join(p))
            .unwrap_or_else(|| plugin_path.join(HOOKS_DIR).join(HOOKS_FILE));

        if !hooks_path.exists() {
            return Ok(Vec::new());
        }

        let content = tokio::fs::read_to_string(&hooks_path).await?;
        let config: HooksFileConfig = serde_json::from_str(&content).map_err(|e| {
            ExtensionError::config_parse(&hooks_path, format!("Invalid hooks.json: {}", e))
        })?;

        let mut hooks = Vec::new();
        for (event, matchers) in config.hooks {
            for matcher in matchers {
                hooks.push(HookConfig {
                    event,
                    kind: HookKind::default(),
                    priority: HookPriority::default(),
                    matcher: matcher.matcher,
                    actions: matcher.hooks,
                    plugin_name: plugin_name.to_string(),
                    plugin_root: plugin_path.to_path_buf(),
                    handler: None,
                });
            }
        }

        Ok(hooks)
    }

    /// Load MCP servers from a plugin
    pub(super) async fn load_mcp_servers(
        &self,
        plugin_path: &Path,
        manifest: &LegacyPluginManifest,
    ) -> ExtensionResult<HashMap<String, McpServerConfig>> {
        let mcp_path = manifest
            .mcp_servers
            .as_ref()
            .map(|p| plugin_path.join(p))
            .unwrap_or_else(|| plugin_path.join(MCP_CONFIG_FILE));

        if !mcp_path.exists() {
            return Ok(HashMap::new());
        }

        let content = tokio::fs::read_to_string(&mcp_path).await?;
        let config: McpFileConfig = serde_json::from_str(&content).map_err(|e| {
            ExtensionError::config_parse(&mcp_path, format!("Invalid .mcp.json: {}", e))
        })?;

        Ok(config.mcp_servers)
    }
}
