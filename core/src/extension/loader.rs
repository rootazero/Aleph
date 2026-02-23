//! Component loader - loads skills, commands, agents, and plugins

use super::error::*;
use super::manifest::{
    parse_frontmatter, parse_plugin_manifest, validate_plugin_name, LegacyPluginManifest,
    PluginManifest,
};
use super::types::*;
use crate::discovery::{
    DiscoveryManager, DiscoverySource, AGENTS_DIR, CLAUDE_HOME_DIR, COMMANDS_DIR, HOOKS_DIR,
    HOOKS_FILE, MCP_CONFIG_FILE, PLUGIN_MANIFEST_DIR, PLUGIN_MANIFEST_FILE, SKILLS_DIR,
    SKILL_FILE, AGENT_FILE,
};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, trace};

/// Component loader
#[derive(Debug, Default)]
pub struct ComponentLoader {
    // Future: could hold caches, runtime references, etc.
}

impl ComponentLoader {
    /// Create a new component loader
    pub fn new() -> Self {
        Self::default()
    }

    /// Load a skill from a directory or file
    pub async fn load_skill(&self, path: &Path) -> ExtensionResult<ExtensionSkill> {
        self.load_skill_internal(path, None, SkillType::Skill).await
    }

    /// Load a command from a directory or file
    pub async fn load_command(&self, path: &Path) -> ExtensionResult<ExtensionCommand> {
        self.load_skill_internal(path, None, SkillType::Command)
            .await
    }

    /// Load a skill/command with optional plugin name
    async fn load_skill_internal(
        &self,
        path: &Path,
        plugin_name: Option<String>,
        skill_type: SkillType,
    ) -> ExtensionResult<ExtensionSkill> {
        // Determine the markdown file path
        let (md_path, name) = if path.is_dir() {
            // Directory format: look for SKILL.md
            let skill_md = path.join(SKILL_FILE);
            if !skill_md.exists() {
                return Err(ExtensionError::missing_field(path, "SKILL.md"));
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            (skill_md, name)
        } else if path.extension().map(|e| e == "md").unwrap_or(false) {
            // Direct .md file
            let name = path
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            (path.to_path_buf(), name)
        } else {
            return Err(ExtensionError::invalid_manifest(
                path,
                "Expected directory with SKILL.md or .md file",
            ));
        };

        debug!("Loading skill from: {:?}", md_path);

        // Read and parse markdown
        let content = tokio::fs::read_to_string(&md_path).await?;
        let (frontmatter, body) = parse_frontmatter::<SkillFrontmatter>(&content, &md_path)?;

        // Build skill
        let skill = ExtensionSkill {
            name: frontmatter.name.unwrap_or(name),
            plugin_name,
            skill_type,
            description: frontmatter.description.unwrap_or_default(),
            content: body,
            disable_model_invocation: frontmatter.disable_model_invocation,
            scope: frontmatter.scope.unwrap_or_default(),
            bound_tool: frontmatter.bound_tool,
            source_path: path.to_path_buf(),
            source: determine_source(path),
        };

        trace!("Loaded skill: {:?}", skill.qualified_name());
        Ok(skill)
    }

    /// Load an agent from a directory or file
    pub async fn load_agent(&self, path: &Path) -> ExtensionResult<ExtensionAgent> {
        self.load_agent_internal(path, None).await
    }

    /// Load an agent with optional plugin name
    async fn load_agent_internal(
        &self,
        path: &Path,
        plugin_name: Option<String>,
    ) -> ExtensionResult<ExtensionAgent> {
        // Determine the markdown file path
        let (md_path, name) = if path.is_dir() {
            // Directory format: look for agent.md
            let agent_md = path.join(AGENT_FILE);
            if !agent_md.exists() {
                return Err(ExtensionError::missing_field(path, "agent.md"));
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            (agent_md, name)
        } else if path.extension().map(|e| e == "md").unwrap_or(false) {
            // Direct .md file
            let name = path
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();
            (path.to_path_buf(), name)
        } else {
            return Err(ExtensionError::invalid_manifest(
                path,
                "Expected directory with agent.md or .md file",
            ));
        };

        debug!("Loading agent from: {:?}", md_path);

        // Read and parse markdown
        let content = tokio::fs::read_to_string(&md_path).await?;
        let (frontmatter, body) = parse_frontmatter::<AgentFrontmatter>(&content, &md_path)?;

        // Build agent
        let agent = ExtensionAgent {
            name,
            plugin_name,
            mode: frontmatter.mode.unwrap_or_default(),
            description: frontmatter.description,
            hidden: frontmatter.hidden.unwrap_or(false),
            color: frontmatter.color,
            model: frontmatter.model,
            temperature: frontmatter.temperature,
            top_p: frontmatter.top_p,
            steps: frontmatter.steps,
            tools: frontmatter.tools,
            permission: frontmatter.permission,
            options: frontmatter.options.unwrap_or_default(),
            system_prompt: body,
            source_path: path.to_path_buf(),
            source: determine_source(path),
        };

        trace!("Loaded agent: {:?}", agent.qualified_name());
        Ok(agent)
    }

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

    /// Load all skills from a directory
    async fn load_skills_from_dir(
        &self,
        dir: &Path,
        plugin_name: Option<String>,
    ) -> ExtensionResult<Vec<ExtensionSkill>> {
        let mut skills = Vec::new();

        let mut entries = tokio::fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();

            // Skip hidden entries
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }

            // Load skill
            match self
                .load_skill_internal(&path, plugin_name.clone(), SkillType::Skill)
                .await
            {
                Ok(skill) => skills.push(skill),
                Err(e) => {
                    tracing::warn!("Failed to load skill from {:?}: {}", path, e);
                }
            }
        }

        Ok(skills)
    }

    /// Load all commands from a directory
    async fn load_commands_from_dir(
        &self,
        dir: &Path,
        plugin_name: Option<String>,
    ) -> ExtensionResult<Vec<ExtensionCommand>> {
        let mut commands = Vec::new();

        let mut entries = tokio::fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();

            // Skip hidden entries
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }

            // Load command
            match self
                .load_skill_internal(&path, plugin_name.clone(), SkillType::Command)
                .await
            {
                Ok(cmd) => commands.push(cmd),
                Err(e) => {
                    tracing::warn!("Failed to load command from {:?}: {}", path, e);
                }
            }
        }

        Ok(commands)
    }

    /// Load all agents from a directory
    async fn load_agents_from_dir(
        &self,
        dir: &Path,
        plugin_name: Option<String>,
    ) -> ExtensionResult<Vec<ExtensionAgent>> {
        let mut agents = Vec::new();

        let mut entries = tokio::fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();

            // Skip hidden entries
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }

            // Load agent
            match self.load_agent_internal(&path, plugin_name.clone()).await {
                Ok(agent) => agents.push(agent),
                Err(e) => {
                    tracing::warn!("Failed to load agent from {:?}: {}", path, e);
                }
            }
        }

        Ok(agents)
    }

    /// Load hooks from a plugin
    async fn load_hooks(
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
    async fn load_mcp_servers(
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

    // =========================================================================
    // V2 Prompt Loading Methods
    // =========================================================================

    /// Load V2 global prompt from manifest's `[prompt]` section
    ///
    /// This method reads the prompt file specified in the TOML manifest and
    /// creates an ExtensionSkill with the appropriate scope settings.
    ///
    /// # Arguments
    /// * `manifest` - The parsed V2 plugin manifest
    /// * `plugin_dir` - The plugin directory containing the prompt file
    ///
    /// # Returns
    /// * `Ok(Some(skill))` - If a prompt is configured and loaded successfully
    /// * `Ok(None)` - If no prompt is configured or it's disabled
    /// * `Err(ExtensionError)` - If the prompt file cannot be read or parsed
    pub async fn load_v2_prompt(
        &self,
        manifest: &PluginManifest,
        plugin_dir: &Path,
    ) -> ExtensionResult<Option<ExtensionSkill>> {
        let prompt_config = match &manifest.prompt_v2 {
            Some(p) => p,
            None => return Ok(None),
        };

        // Check if disabled
        if prompt_config.scope == "disabled" {
            return Ok(None);
        }

        // Read prompt file
        let prompt_path = plugin_dir.join(&prompt_config.file);
        let content = tokio::fs::read_to_string(&prompt_path).await.map_err(|e| {
            ExtensionError::invalid_manifest(
                &prompt_path,
                format!("Failed to read prompt file: {}", e),
            )
        })?;

        // Parse frontmatter if present
        let (frontmatter, body) = if content.starts_with("---") {
            parse_frontmatter::<SkillFrontmatter>(&content, &prompt_path)?
        } else {
            (SkillFrontmatter::default(), content)
        };

        let skill = ExtensionSkill {
            name: frontmatter.name.unwrap_or_else(|| manifest.id.clone()),
            plugin_name: Some(manifest.id.clone()),
            skill_type: SkillType::Skill,
            description: frontmatter.description.unwrap_or_default(),
            content: body,
            disable_model_invocation: frontmatter.disable_model_invocation,
            source_path: prompt_path,
            source: DiscoverySource::Plugin,
            scope: PromptScope::from_str_or_default(&prompt_config.scope),
            bound_tool: None,
        };

        debug!("Loaded V2 prompt for plugin {}: scope={:?}", manifest.id, skill.scope);
        Ok(Some(skill))
    }

    /// Load V2 tool-bound prompts (instruction files) from manifest's `[[tools]]` sections
    ///
    /// This method loads instruction files specified in tool definitions and creates
    /// ExtensionSkills bound to specific tools. These are automatically injected
    /// when the associated tool is available.
    ///
    /// # Arguments
    /// * `manifest` - The parsed V2 plugin manifest
    /// * `plugin_dir` - The plugin directory containing the instruction files
    ///
    /// # Returns
    /// * `Ok(Vec<ExtensionSkill>)` - Tool-bound skills loaded from instruction files
    /// * `Err(ExtensionError)` - If an instruction file cannot be read
    pub async fn load_v2_tool_prompts(
        &self,
        manifest: &PluginManifest,
        plugin_dir: &Path,
    ) -> ExtensionResult<Vec<ExtensionSkill>> {
        let tools = match &manifest.tools_v2 {
            Some(t) => t,
            None => return Ok(vec![]),
        };

        let mut skills = Vec::new();

        for tool in tools {
            if let Some(ref instruction_file) = tool.instruction_file {
                let path = plugin_dir.join(instruction_file);
                if path.exists() {
                    let content = tokio::fs::read_to_string(&path).await.map_err(|e| {
                        ExtensionError::invalid_manifest(
                            &path,
                            format!("Failed to read instruction file: {}", e),
                        )
                    })?;

                    let skill = ExtensionSkill {
                        name: format!("{}_instructions", tool.name),
                        plugin_name: Some(manifest.id.clone()),
                        skill_type: SkillType::Skill,
                        description: format!("Instructions for {} tool", tool.name),
                        content,
                        disable_model_invocation: true, // Tool-bound, not direct invoke
                        source_path: path.clone(),
                        source: DiscoverySource::Plugin,
                        scope: PromptScope::Tool,
                        bound_tool: Some(tool.name.clone()),
                    };

                    debug!(
                        "Loaded V2 tool prompt for {}/{}: {}",
                        manifest.id, tool.name, path.display()
                    );
                    skills.push(skill);
                } else {
                    trace!(
                        "Tool instruction file not found for {}/{}: {}",
                        manifest.id, tool.name, path.display()
                    );
                }
            }
        }

        Ok(skills)
    }

    // =========================================================================
    // Bulk Loading
    // =========================================================================

    /// Load all discovered skills, commands, agents, plugins, and hooks.
    ///
    /// This is the primary coordination method for the loading phase.
    /// Called by `ExtensionManager::load_all()` as a single delegation point,
    /// keeping the manager a thin facade over the loader.
    pub async fn load_all(
        &self,
        discovery: &DiscoveryManager,
        registry: &Arc<RwLock<super::registry::ComponentRegistry>>,
        hook_executor: &Arc<RwLock<super::hooks::HookExecutor>>,
    ) -> ExtensionResult<LoadSummary> {
        let mut summary = LoadSummary::default();

        // 1. Load skills
        let skill_dirs = discovery.discover_skill_dirs()?;
        for dir in skill_dirs {
            match self.load_skill(&dir.path).await {
                Ok(skill) => {
                    registry.write().await.register_skill(skill);
                    summary.skills_loaded += 1;
                }
                Err(e) => {
                    tracing::warn!("Failed to load skill from {:?}: {}", dir.path, e);
                    summary.errors.push(format!("{}: {}", dir.path.display(), e));
                }
            }
        }

        // 2. Load commands
        let command_dirs = discovery.discover_command_dirs()?;
        for dir in command_dirs {
            match self.load_command(&dir.path).await {
                Ok(cmd) => {
                    registry.write().await.register_command(cmd);
                    summary.commands_loaded += 1;
                }
                Err(e) => {
                    tracing::warn!("Failed to load command from {:?}: {}", dir.path, e);
                    summary.errors.push(format!("{}: {}", dir.path.display(), e));
                }
            }
        }

        // 3. Load agents
        let agent_dirs = discovery.discover_agent_dirs()?;
        for dir in agent_dirs {
            match self.load_agent(&dir.path).await {
                Ok(agent) => {
                    registry.write().await.register_agent(agent);
                    summary.agents_loaded += 1;
                }
                Err(e) => {
                    tracing::warn!("Failed to load agent from {:?}: {}", dir.path, e);
                    summary.errors.push(format!("{}: {}", dir.path.display(), e));
                }
            }
        }

        // 4. Load plugins (including their embedded skills, commands, agents, and hooks)
        let plugin_dirs = discovery.discover_plugin_dirs()?;
        for dir in plugin_dirs {
            match self.load_plugin(&dir.path).await {
                Ok(plugin) => {
                    // Register plugin hooks
                    if !plugin.hooks.is_empty() {
                        let mut executor = hook_executor.write().await;
                        for hook in plugin.hooks.clone() {
                            executor.add_hook(hook);
                            summary.hooks_loaded += 1;
                        }
                    }

                    // Register plugin components
                    let reg = &mut *registry.write().await;
                    for skill in plugin.skills.clone() {
                        reg.register_skill(skill);
                        summary.skills_loaded += 1;
                    }
                    for cmd in plugin.commands.clone() {
                        reg.register_command(cmd);
                        summary.commands_loaded += 1;
                    }
                    for agent in plugin.agents.clone() {
                        reg.register_agent(agent);
                        summary.agents_loaded += 1;
                    }
                    reg.register_plugin(plugin);
                    summary.plugins_loaded += 1;
                }
                Err(e) => {
                    tracing::warn!("Failed to load plugin from {:?}: {}", dir.path, e);
                    summary.errors.push(format!("{}: {}", dir.path.display(), e));
                }
            }
        }

        // 5. Runtime plugins (Node.js, WASM) are discovered separately via PluginLoader.
        // They register with PluginRegistry (not ComponentRegistry) and are handled
        // through separate API calls on ExtensionManager.

        tracing::info!(
            "Extension loading complete: {} skills, {} commands, {} agents, {} plugins, {} hooks",
            summary.skills_loaded,
            summary.commands_loaded,
            summary.agents_loaded,
            summary.plugins_loaded,
            summary.hooks_loaded
        );

        Ok(summary)
    }
}

/// Determine discovery source from path
fn determine_source(path: &Path) -> DiscoverySource {
    let path_str = path.to_string_lossy();

    if path_str.contains("/.claude/") {
        if path_str.contains(&format!("/{}/", CLAUDE_HOME_DIR)) {
            DiscoverySource::ClaudeGlobal
        } else {
            DiscoverySource::Project
        }
    } else if path_str.contains("/.aleph/") {
        DiscoverySource::AlephGlobal
    } else {
        DiscoverySource::Project
    }
}

/// Hooks file configuration
#[derive(Debug, serde::Deserialize)]
struct HooksFileConfig {
    #[serde(default)]
    hooks: HashMap<HookEvent, Vec<HookMatcher>>,
}

#[derive(Debug, serde::Deserialize)]
struct HookMatcher {
    #[serde(default)]
    matcher: Option<String>,
    hooks: Vec<HookAction>,
}

/// MCP file configuration
#[derive(Debug, serde::Deserialize)]
struct McpFileConfig {
    #[serde(rename = "mcpServers", default)]
    mcp_servers: HashMap<String, McpServerConfig>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_load_skill_from_directory() {
        let temp = TempDir::new().unwrap();
        let skill_dir = temp.path().join("my-skill");
        std::fs::create_dir(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            r#"---
description: Test skill
---

Hello $ARGUMENTS!"#,
        )
        .unwrap();

        let loader = ComponentLoader::new();
        let skill = loader.load_skill(&skill_dir).await.unwrap();

        assert_eq!(skill.name, "my-skill");
        assert_eq!(skill.description, "Test skill");
        assert!(skill.content.contains("Hello $ARGUMENTS!"));
    }

    #[tokio::test]
    async fn test_load_agent() {
        let temp = TempDir::new().unwrap();
        let agent_file = temp.path().join("reviewer.md");
        std::fs::write(
            &agent_file,
            r#"---
mode: subagent
description: Code reviewer
model: anthropic/claude-haiku
temperature: 0.3
---

You are a code reviewer..."#,
        )
        .unwrap();

        let loader = ComponentLoader::new();
        let agent = loader.load_agent(&agent_file).await.unwrap();

        assert_eq!(agent.name, "reviewer");
        assert_eq!(agent.mode, AgentMode::Subagent);
        assert_eq!(agent.model, Some("anthropic/claude-haiku".to_string()));
        assert_eq!(agent.temperature, Some(0.3));
    }

    #[tokio::test]
    async fn test_load_v2_prompt() {
        use crate::extension::manifest::PromptSection;
        use crate::extension::types::PluginKind;

        let temp = TempDir::new().unwrap();
        let plugin_dir = temp.path();

        // Write the prompt file
        std::fs::write(
            plugin_dir.join("SYSTEM.md"),
            r#"---
name: my-system-prompt
description: A system prompt
---

You are a helpful assistant."#,
        )
        .unwrap();

        // Create a manifest with prompt_v2
        let manifest = crate::extension::manifest::PluginManifest {
            id: "test-plugin".to_string(),
            name: "Test Plugin".to_string(),
            version: Some("1.0.0".to_string()),
            description: None,
            kind: PluginKind::Static,
            entry: ".".into(),
            root_dir: plugin_dir.to_path_buf(),
            config_schema: None,
            config_ui_hints: std::collections::HashMap::new(),
            permissions: vec![],
            author: None,
            homepage: None,
            repository: None,
            license: None,
            keywords: vec![],
            extensions: vec![],
            tools_v2: None,
            hooks_v2: None,
            commands_v2: None,
            services_v2: None,
            prompt_v2: Some(PromptSection {
                file: "SYSTEM.md".to_string(),
                scope: "system".to_string(),
            }),
            capabilities_v2: None,
            // P2 fields
            channels_v2: None,
            providers_v2: None,
            http_routes_v2: None,
        };

        let loader = ComponentLoader::new();
        let skill = loader.load_v2_prompt(&manifest, plugin_dir).await.unwrap();

        assert!(skill.is_some());
        let skill = skill.unwrap();
        assert_eq!(skill.name, "my-system-prompt");
        assert_eq!(skill.description, "A system prompt");
        assert_eq!(skill.plugin_name, Some("test-plugin".to_string()));
        assert_eq!(skill.scope, PromptScope::System);
        assert!(skill.content.contains("You are a helpful assistant."));
    }

    #[tokio::test]
    async fn test_load_v2_prompt_disabled() {
        use crate::extension::manifest::PromptSection;
        use crate::extension::types::PluginKind;

        let temp = TempDir::new().unwrap();
        let plugin_dir = temp.path();

        // Create a manifest with disabled prompt
        let manifest = crate::extension::manifest::PluginManifest {
            id: "test-plugin".to_string(),
            name: "Test Plugin".to_string(),
            version: None,
            description: None,
            kind: PluginKind::Static,
            entry: ".".into(),
            root_dir: plugin_dir.to_path_buf(),
            config_schema: None,
            config_ui_hints: std::collections::HashMap::new(),
            permissions: vec![],
            author: None,
            homepage: None,
            repository: None,
            license: None,
            keywords: vec![],
            extensions: vec![],
            tools_v2: None,
            hooks_v2: None,
            commands_v2: None,
            services_v2: None,
            prompt_v2: Some(PromptSection {
                file: "SYSTEM.md".to_string(),
                scope: "disabled".to_string(),
            }),
            capabilities_v2: None,
            // P2 fields
            channels_v2: None,
            providers_v2: None,
            http_routes_v2: None,
        };

        let loader = ComponentLoader::new();
        let skill = loader.load_v2_prompt(&manifest, plugin_dir).await.unwrap();

        // Should return None for disabled prompt
        assert!(skill.is_none());
    }

    #[tokio::test]
    async fn test_load_v2_prompt_no_frontmatter() {
        use crate::extension::manifest::PromptSection;
        use crate::extension::types::PluginKind;

        let temp = TempDir::new().unwrap();
        let plugin_dir = temp.path();

        // Write a prompt file without frontmatter
        std::fs::write(
            plugin_dir.join("SIMPLE.md"),
            "Just plain content without frontmatter.",
        )
        .unwrap();

        let manifest = crate::extension::manifest::PluginManifest {
            id: "simple-plugin".to_string(),
            name: "Simple Plugin".to_string(),
            version: None,
            description: None,
            kind: PluginKind::Static,
            entry: ".".into(),
            root_dir: plugin_dir.to_path_buf(),
            config_schema: None,
            config_ui_hints: std::collections::HashMap::new(),
            permissions: vec![],
            author: None,
            homepage: None,
            repository: None,
            license: None,
            keywords: vec![],
            extensions: vec![],
            tools_v2: None,
            hooks_v2: None,
            commands_v2: None,
            services_v2: None,
            prompt_v2: Some(PromptSection {
                file: "SIMPLE.md".to_string(),
                scope: "user".to_string(),
            }),
            capabilities_v2: None,
            // P2 fields
            channels_v2: None,
            providers_v2: None,
            http_routes_v2: None,
        };

        let loader = ComponentLoader::new();
        let skill = loader.load_v2_prompt(&manifest, plugin_dir).await.unwrap();

        assert!(skill.is_some());
        let skill = skill.unwrap();
        // Name should default to plugin id
        assert_eq!(skill.name, "simple-plugin");
        assert_eq!(skill.content, "Just plain content without frontmatter.");
    }

    #[tokio::test]
    async fn test_load_v2_tool_prompts() {
        use crate::extension::manifest::ToolSection;
        use crate::extension::types::PluginKind;

        let temp = TempDir::new().unwrap();
        let plugin_dir = temp.path();

        // Create instructions directory
        let instructions_dir = plugin_dir.join("instructions");
        std::fs::create_dir(&instructions_dir).unwrap();

        // Write instruction files
        std::fs::write(
            instructions_dir.join("my-tool.md"),
            "Instructions for using my-tool.",
        )
        .unwrap();
        std::fs::write(
            instructions_dir.join("other-tool.md"),
            "Instructions for using other-tool.",
        )
        .unwrap();

        let manifest = crate::extension::manifest::PluginManifest {
            id: "tool-plugin".to_string(),
            name: "Tool Plugin".to_string(),
            version: None,
            description: None,
            kind: PluginKind::Wasm,
            entry: "plugin.wasm".into(),
            root_dir: plugin_dir.to_path_buf(),
            config_schema: None,
            config_ui_hints: std::collections::HashMap::new(),
            permissions: vec![],
            author: None,
            homepage: None,
            repository: None,
            license: None,
            keywords: vec![],
            extensions: vec![],
            tools_v2: Some(vec![
                ToolSection {
                    name: "my-tool".to_string(),
                    description: Some("My tool".to_string()),
                    handler: Some("handle_my_tool".to_string()),
                    instruction_file: Some("instructions/my-tool.md".to_string()),
                    parameters: None,
                },
                ToolSection {
                    name: "other-tool".to_string(),
                    description: Some("Other tool".to_string()),
                    handler: Some("handle_other_tool".to_string()),
                    instruction_file: Some("instructions/other-tool.md".to_string()),
                    parameters: None,
                },
                ToolSection {
                    name: "no-instruction-tool".to_string(),
                    description: Some("Tool without instructions".to_string()),
                    handler: Some("handle_no_instruction".to_string()),
                    instruction_file: None,
                    parameters: None,
                },
            ]),
            hooks_v2: None,
            commands_v2: None,
            services_v2: None,
            prompt_v2: None,
            capabilities_v2: None,
            // P2 fields
            channels_v2: None,
            providers_v2: None,
            http_routes_v2: None,
        };

        let loader = ComponentLoader::new();
        let skills = loader.load_v2_tool_prompts(&manifest, plugin_dir).await.unwrap();

        // Should load 2 instruction files (the third tool has no instruction_file)
        assert_eq!(skills.len(), 2);

        // Check first skill
        let my_tool_skill = skills.iter().find(|s| s.name == "my-tool_instructions").unwrap();
        assert_eq!(my_tool_skill.plugin_name, Some("tool-plugin".to_string()));
        assert_eq!(my_tool_skill.scope, PromptScope::Tool);
        assert_eq!(my_tool_skill.bound_tool, Some("my-tool".to_string()));
        assert!(my_tool_skill.disable_model_invocation);
        assert!(my_tool_skill.content.contains("Instructions for using my-tool."));

        // Check second skill
        let other_tool_skill = skills.iter().find(|s| s.name == "other-tool_instructions").unwrap();
        assert_eq!(other_tool_skill.bound_tool, Some("other-tool".to_string()));
    }

    #[tokio::test]
    async fn test_load_v2_tool_prompts_missing_file() {
        use crate::extension::manifest::ToolSection;
        use crate::extension::types::PluginKind;

        let temp = TempDir::new().unwrap();
        let plugin_dir = temp.path();

        // No instruction file exists
        let manifest = crate::extension::manifest::PluginManifest {
            id: "tool-plugin".to_string(),
            name: "Tool Plugin".to_string(),
            version: None,
            description: None,
            kind: PluginKind::Wasm,
            entry: "plugin.wasm".into(),
            root_dir: plugin_dir.to_path_buf(),
            config_schema: None,
            config_ui_hints: std::collections::HashMap::new(),
            permissions: vec![],
            author: None,
            homepage: None,
            repository: None,
            license: None,
            keywords: vec![],
            extensions: vec![],
            tools_v2: Some(vec![
                ToolSection {
                    name: "my-tool".to_string(),
                    description: Some("My tool".to_string()),
                    handler: Some("handle_my_tool".to_string()),
                    instruction_file: Some("nonexistent.md".to_string()),
                    parameters: None,
                },
            ]),
            hooks_v2: None,
            commands_v2: None,
            services_v2: None,
            prompt_v2: None,
            capabilities_v2: None,
            // P2 fields
            channels_v2: None,
            providers_v2: None,
            http_routes_v2: None,
        };

        let loader = ComponentLoader::new();
        let skills = loader.load_v2_tool_prompts(&manifest, plugin_dir).await.unwrap();

        // Should gracefully skip missing files
        assert_eq!(skills.len(), 0);
    }
}
