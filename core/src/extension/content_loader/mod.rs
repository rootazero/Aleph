//! Content loader - loads skills, commands, agents, and plugins

mod bulk;
mod plugin;
mod skill;
mod v2_prompt;

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
use tracing::{debug, trace};

/// Result of loading all components from discovery
#[derive(Debug, Default)]
pub struct LoadResult {
    /// All loaded skills (standalone + from plugins)
    pub skills: Vec<ExtensionSkill>,
    /// All loaded commands (standalone + from plugins)
    pub commands: Vec<ExtensionCommand>,
    /// All loaded agents (standalone + from plugins)
    pub agents: Vec<ExtensionAgent>,
    /// All loaded hook configurations
    pub hooks: Vec<HookConfig>,
    /// Load summary with counts and errors
    pub summary: LoadSummary,
}

/// Content loader
#[derive(Debug, Default)]
pub struct ContentLoader {
    // Future: could hold caches, runtime references, etc.
}

impl ContentLoader {
    /// Create a new content loader
    pub fn new() -> Self {
        Self::default()
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
pub(super) struct HooksFileConfig {
    #[serde(default)]
    pub(super) hooks: HashMap<HookEvent, Vec<HookMatcher>>,
}

#[derive(Debug, serde::Deserialize)]
pub(super) struct HookMatcher {
    #[serde(default)]
    pub(super) matcher: Option<String>,
    pub(super) hooks: Vec<HookAction>,
}

/// MCP file configuration
#[derive(Debug, serde::Deserialize)]
pub(super) struct McpFileConfig {
    #[serde(rename = "mcpServers", default)]
    pub(super) mcp_servers: HashMap<String, McpServerConfig>,
}

#[cfg(test)]
mod tests;
