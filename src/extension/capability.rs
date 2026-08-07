//! Capability-driven plugin architecture types
//!
//! This module defines the `CapabilityDeclaration` enum — the unified model for
//! everything a plugin can contribute. Each variant maps 1:1 to a registration
//! type in the `PluginRegistry`.

use serde::{Deserialize, Serialize};

use crate::extension::manifest::PluginPermission;
use crate::extension::registry::{
    AgentRegistration, HookRegistration, ServiceRegistration, SkillRegistration, ToolRegistration,
};
use crate::extension::types::{McpServerConfig, PluginOrigin};

// ============================================================================
// Type Aliases (Declaration → Registration mapping)
// ============================================================================

/// A tool declaration is a `ToolRegistration`
pub type ToolDeclaration = ToolRegistration;
/// A hook declaration is a `HookRegistration`
pub type HookDeclaration = HookRegistration;
/// A service declaration is a `ServiceRegistration`
pub type ServiceDeclaration = ServiceRegistration;
/// A skill declaration is a `SkillRegistration`
pub type SkillDeclaration = SkillRegistration;
/// An agent declaration is an `AgentRegistration`
pub type AgentDeclaration = AgentRegistration;
/// An MCP server declaration is a `McpServerConfig`
pub type McpServerDeclaration = McpServerConfig;

// ============================================================================
// CapabilityDeclaration
// ============================================================================

/// Unified enum of everything a plugin can contribute.
///
/// Each variant maps 1:1 to a registration type in the `PluginRegistry`.
/// The `#[serde(tag = "type")]` attribute allows serialization as:
/// `{ "type": "tool", "name": "my_tool", ... }`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CapabilityDeclaration {
    /// A callable tool exposed to the agent
    Tool(ToolDeclaration),
    /// A hook that intercepts system events
    Hook(HookDeclaration),
    /// A background service
    Service(ServiceDeclaration),
    /// A prompt-based skill. Plugin `commands/` markdown is also modelled here
    /// as a [`SkillRegistration`] with `skill_type = SkillType::Command` (a
    /// user-triggered `/command`), so the whole "skill vs command" distinction
    /// lives in one store rather than a parallel `CommandRegistration` type.
    Skill(SkillDeclaration),
    /// An agent definition
    Agent(AgentDeclaration),
    /// An MCP server configuration
    McpServer(McpServerDeclaration),
}

impl CapabilityDeclaration {
    /// Returns the tier this capability belongs to.
    #[must_use]
    pub const fn tier(&self) -> Tier {
        match self {
            // P0: Always allowed, no permission check
            Self::Tool(_) | Self::Hook(_) | Self::Skill(_) => Tier::Core,
            // P1: Always allowed, no permission check
            Self::Agent(_) => Tier::Important,
            // P2: Some are permission-gated (Service needs Background)
            Self::Service(_) | Self::McpServer(_) => Tier::Pluggable,
        }
    }

    /// Returns a human-readable kind name for this capability.
    #[must_use]
    pub const fn kind_name(&self) -> &'static str {
        match self {
            Self::Tool(_) => "tool",
            Self::Hook(_) => "hook",
            Self::Service(_) => "service",
            Self::Skill(_) => "skill",
            Self::Agent(_) => "agent",
            Self::McpServer(_) => "mcp_server",
        }
    }

    /// Returns the required permission for this capability, if any.
    #[must_use]
    pub const fn required_permission(&self) -> Option<PluginPermission> {
        match self {
            // P0 + P1: no permission needed
            Self::Tool(_) | Self::Hook(_) | Self::Skill(_) | Self::Agent(_) => None,
            // P2: some need permission
            Self::Service(_) => Some(PluginPermission::Background),
            Self::McpServer(_) => None, // MCP is the standard extension mechanism
        }
    }
}

// ============================================================================
// Tier
// ============================================================================

/// Priority tier for capabilities.
///
/// Determines registration order and conflict resolution behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Core capabilities (Tool, Hook, Skill) — always registered first
    Core,
    /// Important capabilities (Agent)
    Important,
    /// Pluggable capabilities (Service, `McpServer`)
    Pluggable,
}

// ============================================================================
// CapabilitySource
// ============================================================================

/// Metadata about where a capability was declared.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilitySource {
    /// The plugin that declared this capability
    pub plugin_id: String,
    /// Where the plugin was discovered
    pub origin: PluginOrigin,
    /// Format of the source manifest
    pub format: SourceFormat,
}

/// The manifest format that declared a capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceFormat {
    /// aleph.plugin.toml (native format)
    AlephToml,
    /// .claude/plugin.json (Claude Code compat)
    ClaudeCode,
    /// .codex-plugin/plugin.json (Codex CLI compat)
    Codex,
    /// .cursor-plugin/plugin.json or .cursorrules (Cursor IDE compat)
    Cursor,
    /// Auto-discovered from well-known directory structure
    AutoDiscovered,
    /// Dynamic registration via MCP/WASM runtime
    Runtime,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool() -> ToolDeclaration {
        ToolRegistration {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            handler: "handle_test".to_string(),
            plugin_id: "test-plugin".to_string(),
        }
    }

    fn make_skill() -> SkillDeclaration {
        SkillRegistration {
            name: "test-skill".to_string(),
            description: "A test skill".to_string(),
            content: "You are a test skill.".to_string(),
            triggers: vec!["test".to_string()],
            plugin_id: "test-plugin".to_string(),
            ..Default::default()
        }
    }

    fn make_agent() -> AgentDeclaration {
        AgentRegistration {
            name: "test-agent".to_string(),
            description: Some("A test agent".to_string()),
            content: "You are a test agent.".to_string(),
            plugin_id: "test-plugin".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_tier_classification() {
        let tool = CapabilityDeclaration::Tool(make_tool());
        assert_eq!(tool.tier(), Tier::Core);

        let skill = CapabilityDeclaration::Skill(make_skill());
        assert_eq!(skill.tier(), Tier::Core); // Skills are P0 Core

        let agent = CapabilityDeclaration::Agent(make_agent());
        assert_eq!(agent.tier(), Tier::Important); // Agents are P1 Important
    }

    #[test]
    fn test_kind_name() {
        let tool = CapabilityDeclaration::Tool(make_tool());
        assert_eq!(tool.kind_name(), "tool");

        let skill = CapabilityDeclaration::Skill(make_skill());
        assert_eq!(skill.kind_name(), "skill");

        let agent = CapabilityDeclaration::Agent(make_agent());
        assert_eq!(agent.kind_name(), "agent");
    }

    #[test]
    fn test_required_permission() {
        let tool = CapabilityDeclaration::Tool(make_tool());
        assert_eq!(tool.required_permission(), None);

        let skill = CapabilityDeclaration::Skill(make_skill());
        assert_eq!(skill.required_permission(), None);
    }

    #[test]
    fn test_source_format_serialization() {
        let format = SourceFormat::AlephToml;
        let json = serde_json::to_string(&format).unwrap();
        assert_eq!(json, "\"aleph_toml\"");

        let roundtrip: SourceFormat = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip, SourceFormat::AlephToml);
    }

    #[test]
    fn test_capability_source() {
        let source = CapabilitySource {
            plugin_id: "my-plugin".to_string(),
            origin: PluginOrigin::Global,
            format: SourceFormat::ClaudeCode,
        };
        let json = serde_json::to_string(&source).unwrap();
        let roundtrip: CapabilitySource = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip.plugin_id, "my-plugin");
        assert_eq!(roundtrip.format, SourceFormat::ClaudeCode);
    }

    #[test]
    fn test_tier_serialization() {
        let tier = Tier::Core;
        let json = serde_json::to_string(&tier).unwrap();
        assert_eq!(json, "\"core\"");

        let roundtrip: Tier = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip, Tier::Core);
    }

    #[test]
    fn test_all_7_variants_have_kind_names() {
        use crate::extension::types::HookEvent;

        let capabilities: Vec<CapabilityDeclaration> = vec![
            CapabilityDeclaration::Tool(make_tool()),
            CapabilityDeclaration::Hook(HookRegistration {
                event: HookEvent::BeforeToolCall,
                priority: 0,
                handler: "h".to_string(),
                name: None,
                description: None,
                plugin_id: "p".to_string(),
                kind: None,
                matcher: None,
                actions: Vec::new(),
                plugin_root: None,
                timeout_secs: None,
            }),
            CapabilityDeclaration::Service(ServiceRegistration {
                id: "s".to_string(),
                name: "S".to_string(),
                start_handler: "start".to_string(),
                stop_handler: "stop".to_string(),
                plugin_id: "p".to_string(),
                auto_start: true,
            }),
            CapabilityDeclaration::Skill(make_skill()),
            CapabilityDeclaration::Agent(make_agent()),
            CapabilityDeclaration::McpServer(McpServerConfig::Stdio {
                command: "npx".to_string(),
                args: vec![],
                env: std::collections::HashMap::new(),
            }),
        ];

        assert_eq!(capabilities.len(), 6);
        let kind_names: Vec<&str> = capabilities.iter().map(|c| c.kind_name()).collect();
        assert_eq!(
            kind_names,
            vec!["tool", "hook", "service", "skill", "agent", "mcp_server",]
        );
    }
}
