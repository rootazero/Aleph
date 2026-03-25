//! Plugin registration type definitions
//!
//! This module defines the 9 registration types used by the plugin API:
//! - P0 Core: Tool, Hook
//! - P1 Important: Channel, Provider, GatewayMethod
//! - P2 Useful: HttpRoute, HttpHandler, Cli, Service
//! - P3 Optional: Command
//!
//! Plus diagnostics support for plugin health reporting.

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::PathBuf;

/// JSON Schema type alias for tool parameter definitions
pub type JsonSchema = JsonValue;

// ============================================================================
// P0 Core Registration Types
// ============================================================================

/// Tool registration for plugins to expose callable tools to the agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRegistration {
    /// Unique tool name (must be unique across all plugins)
    pub name: String,
    /// Human-readable description of what the tool does
    pub description: String,
    /// JSON Schema defining the tool's input parameters
    pub parameters: JsonSchema,
    /// Handler function name within the plugin
    pub handler: String,
    /// ID of the plugin that registered this tool
    pub plugin_id: String,
}

/// Hook registration for plugins to intercept system events
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookRegistration {
    /// The event that triggers this hook
    pub event: crate::extension::types::HookEvent,
    /// Execution priority (lower = earlier, default 0)
    pub priority: i32,
    /// Handler function name within the plugin
    pub handler: String,
    /// Optional human-readable name for the hook
    pub name: Option<String>,
    /// Optional description of what the hook does
    pub description: Option<String>,
    /// ID of the plugin that registered this hook
    pub plugin_id: String,
}

// ============================================================================
// P1 Important Registration Types
// ============================================================================

/// Channel registration for messaging platform integrations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRegistration {
    /// Unique channel identifier (e.g., "telegram", "discord")
    pub id: String,
    /// Display label for the channel
    pub label: String,
    /// Path to documentation
    pub docs_path: Option<String>,
    /// Short description blurb
    pub blurb: Option<String>,
    /// System image/icon path
    pub system_image: Option<String>,
    /// Alternative names for the channel
    pub aliases: Vec<String>,
    /// Display order (lower = first)
    pub order: i32,
    /// ID of the plugin that registered this channel
    pub plugin_id: String,
}

/// Provider registration for AI model providers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderRegistration {
    /// Unique provider identifier (e.g., "anthropic", "openai")
    pub id: String,
    /// Display name for the provider
    pub name: String,
    /// List of model IDs supported by this provider
    pub models: Vec<String>,
    /// ID of the plugin that registered this provider
    pub plugin_id: String,
}

/// Gateway RPC method registration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayMethodRegistration {
    /// Method name (e.g., "myplugin.do_something")
    pub method: String,
    /// Optional description of the method
    pub description: Option<String>,
    /// Handler function name within the plugin
    pub handler: String,
    /// ID of the plugin that registered this method
    pub plugin_id: String,
}

// ============================================================================
// P2 Useful Registration Types
// ============================================================================

/// HTTP route registration for REST API endpoints
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpRouteRegistration {
    /// URL path pattern (e.g., "/api/v1/myendpoint")
    pub path: String,
    /// HTTP methods allowed (e.g., ["GET", "POST"])
    pub methods: Vec<String>,
    /// Handler function name within the plugin
    pub handler: String,
    /// ID of the plugin that registered this route
    pub plugin_id: String,
}

/// HTTP handler registration for middleware/interceptors
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpHandlerRegistration {
    /// Handler function name within the plugin
    pub handler: String,
    /// Execution priority (lower = earlier)
    pub priority: i32,
    /// ID of the plugin that registered this handler
    pub plugin_id: String,
}

/// CLI command registration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliRegistration {
    /// Command name (e.g., "mycommand")
    pub name: String,
    /// Description shown in help text
    pub description: String,
    /// Handler function name within the plugin
    pub handler: String,
    /// ID of the plugin that registered this command
    pub plugin_id: String,
}

/// Background service registration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceRegistration {
    /// Unique service identifier
    pub id: String,
    /// Display name for the service
    pub name: String,
    /// Handler function to start the service
    pub start_handler: String,
    /// Handler function to stop the service
    pub stop_handler: String,
    /// ID of the plugin that registered this service
    pub plugin_id: String,
}

// ============================================================================
// P3 Optional Registration Types
// ============================================================================

/// In-chat command registration (e.g., /mycommand)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandRegistration {
    /// Command name without leading slash
    pub name: String,
    /// Description shown in command help
    pub description: String,
    /// Handler function name within the plugin
    pub handler: String,
    /// ID of the plugin that registered this command
    pub plugin_id: String,
}

// ============================================================================
// Diagnostics
// ============================================================================

/// Severity level for plugin diagnostics
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticLevel {
    /// Warning - plugin may have issues but can continue
    Warn,
    /// Error - plugin has critical issues
    Error,
}

/// Diagnostic message from a plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDiagnostic {
    /// Severity level
    pub level: DiagnosticLevel,
    /// Human-readable diagnostic message
    pub message: String,
    /// ID of the plugin that generated this diagnostic
    pub plugin_id: Option<String>,
    /// Source location or component that generated the diagnostic
    pub source: Option<String>,
}

// ============================================================================
// Skill & Agent Registration Types (added for capability-driven architecture)
// ============================================================================

/// Skill registration for plugins to expose prompt-based skills.
///
/// This is the unified type for both plugin-registered skills (via `CapabilityApi`)
/// and filesystem-discovered skills (via legacy loader). `ExtensionSkill` is a type
/// alias for this struct.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillRegistration {
    /// Unique skill name
    pub name: String,

    /// Plugin name (if from a plugin, used for qualified_name)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_name: Option<String>,

    /// Skill type (command vs skill)
    #[serde(default)]
    pub skill_type: crate::extension::types::SkillType,

    /// Human-readable description of the skill
    pub description: String,

    /// Skill prompt content (Markdown)
    pub content: String,

    /// Whether to disable automatic model invocation
    #[serde(default)]
    pub disable_model_invocation: bool,

    /// Prompt injection scope
    #[serde(default)]
    pub scope: crate::extension::types::PromptScope,

    /// Bound tool name (for Tool scope)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_tool: Option<String>,

    /// Trigger phrases that activate this skill
    #[serde(default)]
    pub triggers: Vec<String>,

    /// Tools this skill is allowed to use
    #[serde(default)]
    pub allowed_tools: Vec<String>,

    /// Optional category for grouping
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,

    /// Source path on disk (empty for runtime-registered skills)
    #[serde(default)]
    pub source_path: PathBuf,

    /// Discovery source
    #[serde(default)]
    pub source: crate::discovery::DiscoverySource,

    /// ID of the plugin that registered this skill (plugin API path)
    #[serde(default)]
    pub plugin_id: String,
}

impl SkillRegistration {
    /// Get the fully qualified name (plugin:skill or just skill)
    pub fn qualified_name(&self) -> String {
        match &self.plugin_name {
            Some(plugin) => format!("{}:{}", plugin, self.name),
            None => self.name.clone(),
        }
    }

    /// Check if this skill can be auto-invoked by the model
    pub fn is_auto_invocable(&self) -> bool {
        !self.disable_model_invocation
            && self.skill_type == crate::extension::types::SkillType::Skill
    }

    /// Substitute $ARGUMENTS placeholder
    pub fn with_arguments(&self, arguments: &str) -> String {
        self.content.replace("$ARGUMENTS", arguments)
    }

    /// Convert to SkillInfo for compatibility with ToolRegistry
    pub fn to_skill_info(&self) -> crate::skills::SkillInfo {
        crate::skills::SkillInfo {
            id: self.qualified_name(),
            name: self.name.clone(),
            description: self.description.clone(),
            triggers: self.triggers.clone(),
            allowed_tools: self.allowed_tools.clone(),
            ecosystem: "aleph".to_string(),
        }
    }

    /// Get the base directory for this skill (for file references)
    pub fn base_dir(&self) -> PathBuf {
        self.source_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

/// Agent registration for plugins to expose agent definitions.
///
/// This is the unified type for both plugin-registered agents (via `CapabilityApi`)
/// and filesystem-discovered agents (via legacy loader). `ExtensionAgent` is a type
/// alias for this struct.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentRegistration {
    /// Unique agent name
    pub name: String,

    /// Plugin name (if from a plugin)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_name: Option<String>,

    /// Agent mode
    #[serde(default)]
    pub mode: crate::extension::types::AgentMode,

    /// Human-readable description of the agent
    #[serde(default)]
    pub description: Option<String>,

    /// Whether to hide from UI
    #[serde(default)]
    pub hidden: bool,

    /// UI color (hex format)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,

    /// Optional model override
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Temperature
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    /// Top P
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,

    /// Maximum iteration steps
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<u32>,

    /// Tool permissions
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<HashMap<String, bool>>,

    /// Permission rules
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<HashMap<String, crate::extension::types::PermissionRule>>,

    /// Provider-specific options
    #[serde(default)]
    pub options: HashMap<String, serde_json::Value>,

    /// Agent system prompt content (markdown body)
    pub content: String,

    /// Source path on disk (empty for runtime-registered agents)
    #[serde(default)]
    pub source_path: PathBuf,

    /// Discovery source
    #[serde(default)]
    pub source: crate::discovery::DiscoverySource,

    /// ID of the plugin that registered this agent (plugin API path)
    #[serde(default)]
    pub plugin_id: String,
}

impl AgentRegistration {
    /// Get the fully qualified name
    pub fn qualified_name(&self) -> String {
        match &self.plugin_name {
            Some(plugin) => format!("{}:{}", plugin, self.name),
            None => self.name.clone(),
        }
    }

    /// Check if agent is a primary agent
    pub fn is_primary(&self) -> bool {
        matches!(
            self.mode,
            crate::extension::types::AgentMode::Primary
                | crate::extension::types::AgentMode::All
        )
    }

    /// Check if agent can be used as a sub-agent
    pub fn is_subagent(&self) -> bool {
        matches!(
            self.mode,
            crate::extension::types::AgentMode::Subagent
                | crate::extension::types::AgentMode::All
        )
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_registration() {
        let tool = ToolRegistration {
            name: "my_tool".to_string(),
            description: "A test tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            handler: "handle_my_tool".to_string(),
            plugin_id: "test-plugin".to_string(),
        };
        assert_eq!(tool.name, "my_tool");
        assert_eq!(tool.plugin_id, "test-plugin");
    }

    #[test]
    fn test_hook_event_serialization() {
        use crate::extension::types::HookEvent;

        let event = HookEvent::BeforeToolCall;
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, "\"before_tool_call\"");

        let deserialized: HookEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, HookEvent::BeforeToolCall);
    }

    #[test]
    fn test_all_hook_events_serialize() {
        use crate::extension::types::HookEvent;

        let events = [
            HookEvent::BeforeAgentStart,
            HookEvent::AgentEnd,
            HookEvent::BeforeToolCall,
            HookEvent::AfterToolCall,
            HookEvent::ToolResultPersist,
            HookEvent::MessageReceived,
            HookEvent::MessageSending,
            HookEvent::MessageSent,
            HookEvent::SessionStart,
            HookEvent::SessionEnd,
            HookEvent::BeforeCompaction,
            HookEvent::AfterCompaction,
            HookEvent::GatewayStart,
            HookEvent::GatewayStop,
            HookEvent::Notification,
            HookEvent::PermissionRequest,
        ];

        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            let roundtrip: HookEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(roundtrip, event);
        }
    }

    #[test]
    fn test_hook_event_pascal_case_aliases() {
        use crate::extension::types::HookEvent;

        // Verify PascalCase aliases work for backward compat
        let event: HookEvent = serde_json::from_str("\"PreToolUse\"").unwrap();
        assert_eq!(event, HookEvent::BeforeToolCall);

        let event: HookEvent = serde_json::from_str("\"PostToolUse\"").unwrap();
        assert_eq!(event, HookEvent::AfterToolCall);

        let event: HookEvent = serde_json::from_str("\"PreCompact\"").unwrap();
        assert_eq!(event, HookEvent::BeforeCompaction);

        let event: HookEvent = serde_json::from_str("\"SessionStart\"").unwrap();
        assert_eq!(event, HookEvent::SessionStart);
    }

    #[test]
    fn test_hook_registration() {
        use crate::extension::types::HookEvent;

        let hook = HookRegistration {
            event: HookEvent::MessageReceived,
            priority: 10,
            handler: "on_message".to_string(),
            name: Some("Message Logger".to_string()),
            description: Some("Logs all incoming messages".to_string()),
            plugin_id: "logger-plugin".to_string(),
        };
        assert_eq!(hook.priority, 10);
        assert_eq!(hook.name, Some("Message Logger".to_string()));
    }

    #[test]
    fn test_channel_registration() {
        let channel = ChannelRegistration {
            id: "telegram".to_string(),
            label: "Telegram".to_string(),
            docs_path: Some("/docs/telegram.md".to_string()),
            blurb: Some("Telegram Bot integration".to_string()),
            system_image: None,
            aliases: vec!["tg".to_string()],
            order: 1,
            plugin_id: "telegram-plugin".to_string(),
        };
        assert_eq!(channel.aliases, vec!["tg"]);
    }

    #[test]
    fn test_provider_registration() {
        let provider = ProviderRegistration {
            id: "anthropic".to_string(),
            name: "Anthropic".to_string(),
            models: vec![
                "claude-opus-4-5".to_string(),
                "claude-sonnet-4".to_string(),
            ],
            plugin_id: "anthropic-plugin".to_string(),
        };
        assert_eq!(provider.models.len(), 2);
    }

    #[test]
    fn test_gateway_method_registration() {
        let method = GatewayMethodRegistration {
            method: "myplugin.execute".to_string(),
            description: Some("Execute a custom action".to_string()),
            handler: "execute_action".to_string(),
            plugin_id: "my-plugin".to_string(),
        };
        assert!(method.method.starts_with("myplugin."));
    }

    #[test]
    fn test_http_route_registration() {
        let route = HttpRouteRegistration {
            path: "/api/v1/webhook".to_string(),
            methods: vec!["GET".to_string(), "POST".to_string()],
            handler: "handle_webhook".to_string(),
            plugin_id: "webhook-plugin".to_string(),
        };
        assert_eq!(route.methods.len(), 2);
    }

    #[test]
    fn test_http_handler_registration() {
        let handler = HttpHandlerRegistration {
            handler: "auth_middleware".to_string(),
            priority: -100,
            plugin_id: "auth-plugin".to_string(),
        };
        assert_eq!(handler.priority, -100);
    }

    #[test]
    fn test_cli_registration() {
        let cli = CliRegistration {
            name: "sync".to_string(),
            description: "Sync data with remote".to_string(),
            handler: "handle_sync".to_string(),
            plugin_id: "sync-plugin".to_string(),
        };
        assert_eq!(cli.name, "sync");
    }

    #[test]
    fn test_service_registration() {
        let service = ServiceRegistration {
            id: "background-worker".to_string(),
            name: "Background Worker".to_string(),
            start_handler: "start_worker".to_string(),
            stop_handler: "stop_worker".to_string(),
            plugin_id: "worker-plugin".to_string(),
        };
        assert_ne!(service.start_handler, service.stop_handler);
    }

    #[test]
    fn test_command_registration() {
        let command = CommandRegistration {
            name: "remind".to_string(),
            description: "Set a reminder".to_string(),
            handler: "handle_remind".to_string(),
            plugin_id: "reminder-plugin".to_string(),
        };
        assert_eq!(command.name, "remind");
    }

    #[test]
    fn test_diagnostic_level_serialization() {
        let warn = DiagnosticLevel::Warn;
        let error = DiagnosticLevel::Error;

        assert_eq!(serde_json::to_string(&warn).unwrap(), "\"warn\"");
        assert_eq!(serde_json::to_string(&error).unwrap(), "\"error\"");
    }

    #[test]
    fn test_plugin_diagnostic() {
        let diagnostic = PluginDiagnostic {
            level: DiagnosticLevel::Error,
            message: "Failed to connect to database".to_string(),
            plugin_id: Some("db-plugin".to_string()),
            source: Some("connection_pool".to_string()),
        };
        assert_eq!(diagnostic.level, DiagnosticLevel::Error);
        assert!(diagnostic.message.contains("database"));
    }

    #[test]
    fn test_skill_registration() {
        let skill = SkillRegistration {
            name: "web-search".to_string(),
            description: "Search the web".to_string(),
            content: "You are a web search assistant.".to_string(),
            triggers: vec!["search".to_string(), "find".to_string()],
            allowed_tools: vec!["web_search".to_string()],
            category: Some("research".to_string()),
            plugin_id: "search-plugin".to_string(),
            ..Default::default()
        };
        assert_eq!(skill.name, "web-search");
        assert_eq!(skill.triggers.len(), 2);
        assert_eq!(skill.category, Some("research".to_string()));
    }

    #[test]
    fn test_agent_registration() {
        let agent = AgentRegistration {
            name: "coder".to_string(),
            description: Some("A coding assistant agent".to_string()),
            content: "You are a coding expert.".to_string(),
            model: Some("claude-sonnet-4".to_string()),
            plugin_id: "coder-plugin".to_string(),
            ..Default::default()
        };
        assert_eq!(agent.name, "coder");
        assert_eq!(agent.model, Some("claude-sonnet-4".to_string()));
    }

    #[test]
    fn test_skill_registration_serialization() {
        let skill = SkillRegistration {
            name: "test-skill".to_string(),
            description: "A test skill".to_string(),
            content: "prompt content".to_string(),
            triggers: vec!["trigger1".to_string()],
            category: None,
            plugin_id: "test-plugin".to_string(),
            ..Default::default()
        };
        let json = serde_json::to_string(&skill).unwrap();
        let roundtrip: SkillRegistration = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip.name, "test-skill");
        assert!(roundtrip.category.is_none());
    }

    #[test]
    fn test_agent_registration_serialization() {
        let agent = AgentRegistration {
            name: "test-agent".to_string(),
            description: Some("A test agent".to_string()),
            content: "system prompt".to_string(),
            model: None,
            plugin_id: "test-plugin".to_string(),
            ..Default::default()
        };
        let json = serde_json::to_string(&agent).unwrap();
        let roundtrip: AgentRegistration = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip.name, "test-agent");
        assert!(roundtrip.model.is_none());
    }
}
