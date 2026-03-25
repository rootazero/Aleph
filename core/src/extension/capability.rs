//! Capability-driven plugin architecture types
//!
//! This module defines the `CapabilityDeclaration` enum — the unified model for
//! everything a plugin can contribute. Each variant maps 1:1 to a registration
//! type in the PluginRegistry.

use serde::{Deserialize, Serialize};

use crate::extension::manifest::PluginPermission;
use crate::extension::registry::{
    AgentRegistration, ChannelRegistration, CliRegistration, CommandRegistration,
    GatewayMethodRegistration, HookRegistration, HttpRouteRegistration, ProviderRegistration,
    ServiceRegistration, SkillRegistration, ToolRegistration,
};
use crate::extension::types::{McpServerConfig, PluginOrigin};

// ============================================================================
// Type Aliases (Declaration → Registration mapping)
// ============================================================================

/// A tool declaration is a ToolRegistration
pub type ToolDeclaration = ToolRegistration;
/// A hook declaration is a HookRegistration
pub type HookDeclaration = HookRegistration;
/// A channel declaration is a ChannelRegistration
pub type ChannelDeclaration = ChannelRegistration;
/// A provider declaration is a ProviderRegistration
pub type ProviderDeclaration = ProviderRegistration;
/// A gateway method declaration is a GatewayMethodRegistration
pub type GatewayMethodDeclaration = GatewayMethodRegistration;
/// An HTTP route declaration is a HttpRouteRegistration
pub type HttpRouteDeclaration = HttpRouteRegistration;
/// A CLI command declaration is a CliRegistration
pub type CliDeclaration = CliRegistration;
/// A service declaration is a ServiceRegistration
pub type ServiceDeclaration = ServiceRegistration;
/// An in-chat command declaration is a CommandRegistration
pub type CommandDeclaration = CommandRegistration;
/// A skill declaration is a SkillRegistration
pub type SkillDeclaration = SkillRegistration;
/// An agent declaration is an AgentRegistration
pub type AgentDeclaration = AgentRegistration;
/// An MCP server declaration is a McpServerConfig
pub type McpServerDeclaration = McpServerConfig;

// ============================================================================
// CapabilityDeclaration
// ============================================================================

/// Unified enum of everything a plugin can contribute.
///
/// Each variant maps 1:1 to a registration type in the PluginRegistry.
/// The `#[serde(tag = "type")]` attribute allows serialization as:
/// `{ "type": "tool", "name": "my_tool", ... }`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CapabilityDeclaration {
    /// A callable tool exposed to the agent
    Tool(ToolDeclaration),
    /// A hook that intercepts system events
    Hook(HookDeclaration),
    /// A messaging channel integration
    Channel(ChannelDeclaration),
    /// An AI model provider backend
    Provider(ProviderDeclaration),
    /// A gateway RPC method
    GatewayMethod(GatewayMethodDeclaration),
    /// An HTTP REST endpoint
    HttpRoute(HttpRouteDeclaration),
    /// A CLI command
    Cli(CliDeclaration),
    /// A background service
    Service(ServiceDeclaration),
    /// An in-chat command (e.g., /mycommand)
    Command(CommandDeclaration),
    /// A prompt-based skill
    Skill(SkillDeclaration),
    /// An agent definition
    Agent(AgentDeclaration),
    /// An MCP server configuration
    McpServer(McpServerDeclaration),
}

impl CapabilityDeclaration {
    /// Returns the tier this capability belongs to.
    pub fn tier(&self) -> Tier {
        match self {
            Self::Tool(_) | Self::Hook(_) => Tier::Core,
            Self::Channel(_) | Self::Provider(_) | Self::GatewayMethod(_) => Tier::Important,
            Self::HttpRoute(_)
            | Self::Cli(_)
            | Self::Service(_)
            | Self::Command(_)
            | Self::Skill(_)
            | Self::Agent(_) => Tier::Pluggable,
            Self::McpServer(_) => Tier::GatewayExtension,
        }
    }

    /// Returns a human-readable kind name for this capability.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Tool(_) => "tool",
            Self::Hook(_) => "hook",
            Self::Channel(_) => "channel",
            Self::Provider(_) => "provider",
            Self::GatewayMethod(_) => "gateway_method",
            Self::HttpRoute(_) => "http_route",
            Self::Cli(_) => "cli",
            Self::Service(_) => "service",
            Self::Command(_) => "command",
            Self::Skill(_) => "skill",
            Self::Agent(_) => "agent",
            Self::McpServer(_) => "mcp_server",
        }
    }

    /// Returns the required permission for this capability, if any.
    pub fn required_permission(&self) -> Option<PluginPermission> {
        match self {
            Self::Tool(_) | Self::Hook(_) | Self::Command(_) | Self::Skill(_) | Self::Agent(_) => {
                None
            }
            Self::Channel(_) => Some(PluginPermission::Network),
            Self::Provider(_) => Some(PluginPermission::Network),
            Self::GatewayMethod(_) => Some(PluginPermission::GatewayRpc),
            Self::HttpRoute(_) => Some(PluginPermission::HttpRoutes),
            Self::Cli(_) => Some(PluginPermission::Shell),
            Self::Service(_) => Some(PluginPermission::Background),
            Self::McpServer(_) => Some(PluginPermission::Shell),
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
    /// Core capabilities (Tool, Hook) — always registered first
    Core,
    /// Important capabilities (Channel, Provider, GatewayMethod)
    Important,
    /// Pluggable capabilities (HttpRoute, Cli, Service, Command, Skill, Agent)
    Pluggable,
    /// Gateway extensions (McpServer) — registered last
    GatewayExtension,
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
        assert_eq!(skill.tier(), Tier::Pluggable);

        let agent = CapabilityDeclaration::Agent(make_agent());
        assert_eq!(agent.tier(), Tier::Pluggable);
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
    fn test_all_12_variants_have_kind_names() {
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
            }),
            CapabilityDeclaration::Channel(ChannelRegistration {
                id: "ch".to_string(),
                label: "Ch".to_string(),
                docs_path: None,
                blurb: None,
                system_image: None,
                aliases: vec![],
                order: 0,
                plugin_id: "p".to_string(),
            }),
            CapabilityDeclaration::Provider(ProviderRegistration {
                id: "pr".to_string(),
                name: "Pr".to_string(),
                models: vec![],
                plugin_id: "p".to_string(),
            }),
            CapabilityDeclaration::GatewayMethod(GatewayMethodRegistration {
                method: "m".to_string(),
                description: None,
                handler: "h".to_string(),
                plugin_id: "p".to_string(),
            }),
            CapabilityDeclaration::HttpRoute(HttpRouteRegistration {
                path: "/".to_string(),
                methods: vec![],
                handler: "h".to_string(),
                plugin_id: "p".to_string(),
            }),
            CapabilityDeclaration::Cli(CliRegistration {
                name: "c".to_string(),
                description: "d".to_string(),
                handler: "h".to_string(),
                plugin_id: "p".to_string(),
            }),
            CapabilityDeclaration::Service(ServiceRegistration {
                id: "s".to_string(),
                name: "S".to_string(),
                start_handler: "start".to_string(),
                stop_handler: "stop".to_string(),
                plugin_id: "p".to_string(),
            }),
            CapabilityDeclaration::Command(CommandRegistration {
                name: "cmd".to_string(),
                description: "d".to_string(),
                handler: "h".to_string(),
                plugin_id: "p".to_string(),
            }),
            CapabilityDeclaration::Skill(make_skill()),
            CapabilityDeclaration::Agent(make_agent()),
            CapabilityDeclaration::McpServer(McpServerConfig {
                command: "npx".to_string(),
                args: vec![],
                env: std::collections::HashMap::new(),
            }),
        ];

        assert_eq!(capabilities.len(), 12);
        let kind_names: Vec<&str> = capabilities.iter().map(|c| c.kind_name()).collect();
        assert_eq!(
            kind_names,
            vec![
                "tool",
                "hook",
                "channel",
                "provider",
                "gateway_method",
                "http_route",
                "cli",
                "service",
                "command",
                "skill",
                "agent",
                "mcp_server",
            ]
        );
    }
}
