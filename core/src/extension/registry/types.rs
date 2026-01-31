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

/// Events that can trigger plugin hooks (for WASM/Node.js plugins).
///
/// This enum is used by the plugin registration API to specify which events
/// a plugin hook should respond to. Uses **snake_case** serialization for
/// JSON-RPC IPC with plugins.
///
/// # Difference from HookEvent
///
/// **`PluginHookEvent`** (this enum):
/// - For WASM/Node.js plugin hooks registered via Plugin API
/// - Uses snake_case serialization (`"before_tool_call"`, `"session_start"`)
/// - Oriented toward plugin lifecycle and gateway events
/// - Registered programmatically by plugins during load
///
/// **[`HookEvent`](crate::extension::types::HookEvent)**:
/// - For shell command hooks configured in CLAUDE.md or config files
/// - Uses PascalCase serialization (`"PreToolUse"`, `"SessionStart"`)
/// - Oriented toward CLI/shell integration
/// - Configured declaratively in config files
///
/// # Example (plugin registration via JSON-RPC)
/// ```json
/// {
///   "hooks": [
///     { "event": "before_tool_call", "handler": "onBeforeToolCall", "priority": 0 },
///     { "event": "message_received", "handler": "onMessage", "priority": -10 }
///   ]
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginHookEvent {
    /// Before agent starts processing
    BeforeAgentStart,
    /// After agent completes processing
    AgentEnd,
    /// Before a tool is called
    BeforeToolCall,
    /// After a tool call completes
    AfterToolCall,
    /// When tool result is being persisted
    ToolResultPersist,
    /// When a message is received from a channel
    MessageReceived,
    /// Before a message is sent to a channel
    MessageSending,
    /// After a message has been sent
    MessageSent,
    /// When a session starts
    SessionStart,
    /// When a session ends
    SessionEnd,
    /// Before session compaction
    BeforeCompaction,
    /// After session compaction
    AfterCompaction,
    /// When gateway starts
    GatewayStart,
    /// When gateway stops
    GatewayStop,
}

/// Hook registration for plugins to intercept system events
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookRegistration {
    /// The event that triggers this hook
    pub event: PluginHookEvent,
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
    fn test_plugin_hook_event_serialization() {
        let event = PluginHookEvent::BeforeToolCall;
        let json = serde_json::to_string(&event).unwrap();
        assert_eq!(json, "\"before_tool_call\"");

        let deserialized: PluginHookEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, PluginHookEvent::BeforeToolCall);
    }

    #[test]
    fn test_all_plugin_hook_events_serialize() {
        let events = [
            PluginHookEvent::BeforeAgentStart,
            PluginHookEvent::AgentEnd,
            PluginHookEvent::BeforeToolCall,
            PluginHookEvent::AfterToolCall,
            PluginHookEvent::ToolResultPersist,
            PluginHookEvent::MessageReceived,
            PluginHookEvent::MessageSending,
            PluginHookEvent::MessageSent,
            PluginHookEvent::SessionStart,
            PluginHookEvent::SessionEnd,
            PluginHookEvent::BeforeCompaction,
            PluginHookEvent::AfterCompaction,
            PluginHookEvent::GatewayStart,
            PluginHookEvent::GatewayStop,
        ];

        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            let roundtrip: PluginHookEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(roundtrip, event);
        }
    }

    #[test]
    fn test_hook_registration() {
        let hook = HookRegistration {
            event: PluginHookEvent::MessageReceived,
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
}
