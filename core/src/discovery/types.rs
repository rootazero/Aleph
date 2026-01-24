//! Type definitions for the discovery system

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Source of a discovered component
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DiscoverySource {
    /// Aether native global (~/.aether/)
    AetherGlobal,
    /// Claude Code global (~/.claude/)
    ClaudeGlobal,
    /// Project-level (./.claude/ in project directory)
    Project,
    /// Plugin-provided (from a loaded plugin)
    Plugin,
}

impl DiscoverySource {
    /// Whether this source is read-only (Claude Code directories)
    pub fn is_read_only(&self) -> bool {
        matches!(self, Self::ClaudeGlobal | Self::Project)
    }

    /// Whether this source is from Claude Code
    pub fn is_claude_source(&self) -> bool {
        matches!(self, Self::ClaudeGlobal | Self::Project)
    }
}

/// A directory to scan for components
#[derive(Debug, Clone)]
pub struct ScanDirectory {
    /// Path to the directory
    pub path: PathBuf,
    /// Source type
    pub source: DiscoverySource,
    /// Priority (higher = later in merge order, takes precedence)
    pub priority: u32,
}

impl ScanDirectory {
    /// Create a new scan directory
    pub fn new(path: PathBuf, source: DiscoverySource, priority: u32) -> Self {
        Self {
            path,
            source,
            priority,
        }
    }

    /// Check if the directory exists
    pub fn exists(&self) -> bool {
        self.path.exists() && self.path.is_dir()
    }
}

/// A discovered path with metadata
#[derive(Debug, Clone)]
pub struct DiscoveredPath {
    /// Full path to the discovered item
    pub path: PathBuf,
    /// Source of the discovery
    pub source: DiscoverySource,
    /// Name derived from the path (e.g., skill name from directory)
    pub name: String,
    /// Priority for conflict resolution
    pub priority: u32,
}

impl DiscoveredPath {
    /// Create a new discovered path
    pub fn new(path: PathBuf, source: DiscoverySource, priority: u32) -> Self {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        Self {
            path,
            source,
            name,
            priority,
        }
    }

    /// Create with explicit name
    pub fn with_name(path: PathBuf, source: DiscoverySource, priority: u32, name: String) -> Self {
        Self {
            path,
            source,
            name,
            priority,
        }
    }
}

/// Component type being discovered
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComponentType {
    /// Skill (AI-invocable)
    Skill,
    /// Command (user-triggered)
    Command,
    /// Agent definition
    Agent,
    /// Plugin package
    Plugin,
    /// Hook configuration
    Hook,
    /// MCP server configuration
    McpServer,
}

impl ComponentType {
    /// Get the directory name for this component type
    pub fn dir_name(&self) -> &'static str {
        match self {
            Self::Skill => "skills",
            Self::Command => "commands",
            Self::Agent => "agents",
            Self::Plugin => "plugins",
            Self::Hook => "hooks",
            Self::McpServer => "mcp",
        }
    }
}

/// Discovered component with full metadata
#[derive(Debug, Clone)]
pub struct DiscoveredComponent {
    /// Component type
    pub component_type: ComponentType,
    /// Discovered path info
    pub path_info: DiscoveredPath,
    /// Plugin name if from a plugin
    pub plugin_name: Option<String>,
}

impl DiscoveredComponent {
    /// Create a new discovered component
    pub fn new(component_type: ComponentType, path_info: DiscoveredPath) -> Self {
        Self {
            component_type,
            path_info,
            plugin_name: None,
        }
    }

    /// Create from a plugin
    pub fn from_plugin(
        component_type: ComponentType,
        path_info: DiscoveredPath,
        plugin_name: String,
    ) -> Self {
        Self {
            component_type,
            path_info,
            plugin_name: Some(plugin_name),
        }
    }

    /// Get the qualified name (plugin:name or just name)
    pub fn qualified_name(&self) -> String {
        match &self.plugin_name {
            Some(plugin) => format!("{}:{}", plugin, self.path_info.name),
            None => self.path_info.name.clone(),
        }
    }
}

/// Configuration file discovery result
#[derive(Debug, Clone)]
pub struct DiscoveredConfig {
    /// Path to the config file
    pub path: PathBuf,
    /// Source of the config
    pub source: DiscoverySource,
    /// Priority (higher = takes precedence)
    pub priority: u32,
}

impl DiscoveredConfig {
    /// Create a new discovered config
    pub fn new(path: PathBuf, source: DiscoverySource, priority: u32) -> Self {
        Self {
            path,
            source,
            priority,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discovery_source_read_only() {
        assert!(!DiscoverySource::AetherGlobal.is_read_only());
        assert!(DiscoverySource::ClaudeGlobal.is_read_only());
        assert!(DiscoverySource::Project.is_read_only());
        assert!(!DiscoverySource::Plugin.is_read_only());
    }

    #[test]
    fn test_component_type_dir_name() {
        assert_eq!(ComponentType::Skill.dir_name(), "skills");
        assert_eq!(ComponentType::Command.dir_name(), "commands");
        assert_eq!(ComponentType::Agent.dir_name(), "agents");
        assert_eq!(ComponentType::Plugin.dir_name(), "plugins");
    }

    #[test]
    fn test_discovered_component_qualified_name() {
        let path_info = DiscoveredPath::new(
            PathBuf::from("/path/to/my-skill"),
            DiscoverySource::AetherGlobal,
            0,
        );

        let component = DiscoveredComponent::new(ComponentType::Skill, path_info.clone());
        assert_eq!(component.qualified_name(), "my-skill");

        let plugin_component = DiscoveredComponent::from_plugin(
            ComponentType::Skill,
            path_info,
            "my-plugin".to_string(),
        );
        assert_eq!(plugin_component.qualified_name(), "my-plugin:my-skill");
    }
}
