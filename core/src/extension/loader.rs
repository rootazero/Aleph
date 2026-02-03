//! Component loader - loads skills, commands, agents, and plugins

use super::error::*;
use super::manifest::{
    parse_frontmatter, parse_plugin_manifest, validate_plugin_name, LegacyPluginManifest,
};
use super::types::*;
use crate::discovery::{
    DiscoverySource, AGENTS_DIR, CLAUDE_HOME_DIR, COMMANDS_DIR, HOOKS_DIR, HOOKS_FILE,
    MCP_CONFIG_FILE, PLUGIN_MANIFEST_DIR, PLUGIN_MANIFEST_FILE, SKILLS_DIR, SKILL_FILE, AGENT_FILE,
};
use std::collections::HashMap;
use std::path::Path;
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
    } else if path_str.contains("/.aether/") {
        DiscoverySource::AetherGlobal
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
}
