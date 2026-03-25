//! Legacy content loading — free functions for loading skills, commands, agents,
//! and legacy plugins from the discovery layer.
//!
//! These functions were extracted from the deprecated `content_loader/` module.
//! They are stateless helpers called by `ExtensionManager::load_all()`.

use super::error::*;
use super::manifest::{
    parse_frontmatter, parse_plugin_manifest, validate_plugin_name, LegacyPluginManifest,
};
use super::types::*;
use crate::discovery::{
    DiscoveryManager, DiscoverySource, AGENTS_DIR, AGENT_FILE, CLAUDE_HOME_DIR, COMMANDS_DIR,
    HOOKS_DIR, HOOKS_FILE, MCP_CONFIG_FILE, PLUGIN_MANIFEST_DIR, PLUGIN_MANIFEST_FILE,
    SKILLS_DIR, SKILL_FILE,
};
use std::collections::HashMap;
use std::path::Path;
use tracing::{debug, trace};

// =============================================================================
// Bulk loading
// =============================================================================

/// Result of loading all components from discovery
#[derive(Debug, Default)]
pub(crate) struct LoadResult {
    pub skills: Vec<ExtensionSkill>,
    pub commands: Vec<ExtensionCommand>,
    pub agents: Vec<ExtensionAgent>,
    pub hooks: Vec<HookConfig>,
    pub summary: LoadSummary,
}

/// Load all discovered skills, commands, agents, plugins, and hooks.
pub(crate) async fn load_all(discovery: &DiscoveryManager) -> ExtensionResult<LoadResult> {
    let mut result = LoadResult::default();

    // 1. Load skills
    let skill_dirs = discovery.discover_skill_dirs()?;
    for dir in skill_dirs {
        match load_skill(&dir.path).await {
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
        match load_command(&dir.path).await {
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
        match load_agent(&dir.path).await {
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
        match load_plugin(&dir.path).await {
            Ok(plugin) => {
                for hook in &plugin.hooks {
                    result.hooks.push(hook.clone());
                    result.summary.hooks_loaded += 1;
                }
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

// =============================================================================
// Skill / Command / Agent loading
// =============================================================================

/// Load a skill from a directory or file
async fn load_skill(path: &Path) -> ExtensionResult<ExtensionSkill> {
    load_skill_internal(path, None, SkillType::Skill).await
}

/// Load a command from a directory or file
async fn load_command(path: &Path) -> ExtensionResult<ExtensionCommand> {
    load_skill_internal(path, None, SkillType::Command).await
}

/// Load a skill/command with optional plugin name
async fn load_skill_internal(
    path: &Path,
    plugin_name: Option<String>,
    skill_type: SkillType,
) -> ExtensionResult<ExtensionSkill> {
    let (md_path, name) = if path.is_dir() {
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

    let content = tokio::fs::read_to_string(&md_path).await?;
    let (frontmatter, body) = parse_frontmatter::<SkillFrontmatter>(&content, &md_path)?;

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
        ..Default::default()
    };

    trace!("Loaded skill: {:?}", skill.qualified_name());
    Ok(skill)
}

/// Load an agent from a directory or file
async fn load_agent(path: &Path) -> ExtensionResult<ExtensionAgent> {
    load_agent_internal(path, None).await
}

/// Load an agent with optional plugin name
async fn load_agent_internal(
    path: &Path,
    plugin_name: Option<String>,
) -> ExtensionResult<ExtensionAgent> {
    let (md_path, name) = if path.is_dir() {
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

    let content = tokio::fs::read_to_string(&md_path).await?;
    let (frontmatter, body) = parse_frontmatter::<AgentFrontmatter>(&content, &md_path)?;

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
        content: body,
        source_path: path.to_path_buf(),
        source: determine_source(path),
        ..Default::default()
    };

    trace!("Loaded agent: {:?}", agent.qualified_name());
    Ok(agent)
}

// =============================================================================
// Legacy plugin loading
// =============================================================================

/// Load a plugin from a directory (legacy .claude-plugin format)
async fn load_plugin(path: &Path) -> ExtensionResult<ExtensionPlugin> {
    debug!("Loading plugin from: {:?}", path);

    let manifest_path = path.join(PLUGIN_MANIFEST_DIR).join(PLUGIN_MANIFEST_FILE);
    if !manifest_path.exists() {
        return Err(ExtensionError::invalid_manifest(
            path,
            "Missing .claude-plugin/plugin.json",
        ));
    }

    let manifest: LegacyPluginManifest = parse_plugin_manifest(&manifest_path).await?;
    validate_plugin_name(&manifest.name)?;

    // Load skills
    let mut skills = Vec::new();
    let skills_dir = manifest
        .skills
        .as_ref()
        .map(|p| path.join(p))
        .unwrap_or_else(|| path.join(SKILLS_DIR));
    if skills_dir.exists() {
        skills.extend(load_skills_from_dir(&skills_dir, Some(manifest.name.clone())).await?);
    }

    // Load commands
    let mut commands = Vec::new();
    let commands_dir = manifest
        .commands
        .as_ref()
        .map(|p| path.join(p))
        .unwrap_or_else(|| path.join(COMMANDS_DIR));
    if commands_dir.exists() {
        commands.extend(load_commands_from_dir(&commands_dir, Some(manifest.name.clone())).await?);
    }

    // Load agents
    let mut agents = Vec::new();
    let agents_dir = manifest
        .agents
        .as_ref()
        .map(|p| path.join(p))
        .unwrap_or_else(|| path.join(AGENTS_DIR));
    if agents_dir.exists() {
        agents.extend(load_agents_from_dir(&agents_dir, Some(manifest.name.clone())).await?);
    }

    // Load hooks
    let hooks = load_hooks(path, &manifest, &manifest.name).await?;

    // Load MCP servers
    let mcp_servers = load_mcp_servers(path, &manifest).await?;

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

// =============================================================================
// Directory scanning helpers
// =============================================================================

async fn load_skills_from_dir(
    dir: &Path,
    plugin_name: Option<String>,
) -> ExtensionResult<Vec<ExtensionSkill>> {
    let mut skills = Vec::new();
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with('.') {
                continue;
            }
        }
        match load_skill_internal(&path, plugin_name.clone(), SkillType::Skill).await {
            Ok(skill) => skills.push(skill),
            Err(e) => tracing::warn!("Failed to load skill from {:?}: {}", path, e),
        }
    }
    Ok(skills)
}

async fn load_commands_from_dir(
    dir: &Path,
    plugin_name: Option<String>,
) -> ExtensionResult<Vec<ExtensionCommand>> {
    let mut commands = Vec::new();
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with('.') {
                continue;
            }
        }
        match load_skill_internal(&path, plugin_name.clone(), SkillType::Command).await {
            Ok(cmd) => commands.push(cmd),
            Err(e) => tracing::warn!("Failed to load command from {:?}: {}", path, e),
        }
    }
    Ok(commands)
}

async fn load_agents_from_dir(
    dir: &Path,
    plugin_name: Option<String>,
) -> ExtensionResult<Vec<ExtensionAgent>> {
    let mut agents = Vec::new();
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with('.') {
                continue;
            }
        }
        match load_agent_internal(&path, plugin_name.clone()).await {
            Ok(agent) => agents.push(agent),
            Err(e) => tracing::warn!("Failed to load agent from {:?}: {}", path, e),
        }
    }
    Ok(agents)
}

// =============================================================================
// Hook / MCP loading
// =============================================================================

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

async fn load_hooks(
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

async fn load_mcp_servers(
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

// =============================================================================
// Helpers
// =============================================================================

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
