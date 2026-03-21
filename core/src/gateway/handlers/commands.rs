//! Commands RPC Handlers
//!
//! Handlers for command listing, discovery, and execution.

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use super::parse_params;
use crate::command::{CommandContext, CommandNode, CommandParser, CommandType};
use crate::dispatcher::{ToolRegistry, ToolSourceType, UnifiedTool};
use crate::sync_primitives::Arc;

/// Command info for JSON serialization
#[derive(Debug, Clone, Serialize)]
pub struct CommandInfo {
    /// Command key (e.g., "search", "webfetch")
    pub key: String,
    /// Human-readable description
    pub description: String,
    /// SF Symbol icon name
    pub icon: String,
    /// Short hint text
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Command type: "action", "prompt", "namespace"
    pub command_type: String,
    /// Whether this command has children
    pub has_children: bool,
    /// Source identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    /// Source type: "builtin", "mcp", "skill", "custom"
    pub source_type: String,
}

impl From<CommandNode> for CommandInfo {
    fn from(node: CommandNode) -> Self {
        Self {
            key: node.key,
            description: node.description,
            icon: node.icon,
            hint: node.hint,
            command_type: node.node_type.as_str().to_string(),
            has_children: node.has_children,
            source_id: node.source_id,
            source_type: source_type_to_string(node.source_type),
        }
    }
}

fn source_type_to_string(st: ToolSourceType) -> String {
    match st {
        ToolSourceType::Builtin => "builtin".to_string(),
        ToolSourceType::Native => "native".to_string(),
        ToolSourceType::Mcp => "mcp".to_string(),
        ToolSourceType::Skill => "skill".to_string(),
        ToolSourceType::Custom => "custom".to_string(),
        ToolSourceType::Plugin => "plugin".to_string(),
    }
}

impl From<UnifiedTool> for CommandInfo {
    fn from(tool: UnifiedTool) -> Self {
        Self {
            key: tool.name,
            description: tool.description,
            icon: tool.icon.unwrap_or_else(|| "bolt".to_string()),
            hint: tool.usage,
            command_type: "action".to_string(),
            has_children: tool.has_subtools,
            source_id: Some(tool.id),
            source_type: tool.source.label().to_lowercase(),
        }
    }
}

/// List all registered commands from ToolRegistry
pub async fn handle_list_from_registry(
    request: JsonRpcRequest,
    tool_registry: &ToolRegistry,
) -> JsonRpcResponse {
    let tools: Vec<UnifiedTool> = tool_registry.list_root_commands().await;
    let command_infos: Vec<CommandInfo> = tools.into_iter().map(CommandInfo::from).collect();

    JsonRpcResponse::success(
        request.id,
        json!({
            "commands": command_infos
        }),
    )
}

/// List all registered commands
///
/// Returns the list of available commands for command completion.
/// In the full implementation, this should be called with access to
/// the GatewayServer state to include MCP servers and skills.
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"commands.list","id":1}
/// ```
///
/// # Example Response
///
/// ```json
/// {"jsonrpc":"2.0","result":{"commands":[{"key":"search","description":"Web search","icon":"magnifyingglass","command_type":"action","has_children":false,"source_type":"builtin"}]},"id":1}
/// ```
pub async fn handle_list(request: JsonRpcRequest) -> JsonRpcResponse {
    // Return builtin commands
    // TODO: In full implementation, access GatewayServer state to include
    // MCP servers, skills, and custom routing rules
    let commands = get_builtin_commands();

    let command_infos: Vec<CommandInfo> = commands.into_iter().map(CommandInfo::from).collect();

    JsonRpcResponse::success(
        request.id,
        json!({
            "commands": command_infos
        }),
    )
}

/// Get builtin commands (system commands)
fn get_builtin_commands() -> Vec<CommandNode> {
    vec![
        CommandNode::new("search", "Web search", CommandType::Action)
            .with_icon("magnifyingglass")
            .with_hint("Search the web")
            .with_source_type(ToolSourceType::Builtin),
        CommandNode::new("webfetch", "Fetch web page", CommandType::Action)
            .with_icon("globe")
            .with_hint("Fetch and parse a URL")
            .with_source_type(ToolSourceType::Builtin),
    ]
}

// ============================================================================
// command.execute — Parse and resolve a slash command via CommandParser
// ============================================================================

/// Parameters for command.execute request
#[derive(Debug, Clone, Deserialize)]
pub struct ExecuteParams {
    /// Slash command input (e.g. "/search weather", "/new")
    pub input: String,
    /// Optional session key for session-scoped commands
    #[serde(default)]
    pub session_id: Option<String>,
}

/// Resolved command info returned by command.execute
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedCommandInfo {
    /// Command name (without leading /)
    pub name: String,
    /// Arguments after the command name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    /// Source type: "builtin", "mcp", "skill", "custom", "plugin"
    pub source_type: String,
    /// Full original input
    pub input: String,
    /// Context-specific details
    pub context: ResolvedCommandContext,
}

/// Command context details for the client
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum ResolvedCommandContext {
    /// Builtin tool
    #[serde(rename = "builtin")]
    Builtin { tool_name: String },
    /// MCP server tool
    #[serde(rename = "mcp")]
    Mcp {
        server_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_name: Option<String>,
    },
    /// Skill
    #[serde(rename = "skill")]
    Skill {
        skill_id: String,
        display_name: String,
    },
    /// Custom routing rule
    #[serde(rename = "custom")]
    Custom { pattern: String },
    /// Unknown / no context
    #[serde(rename = "none")]
    None,
}

impl From<CommandContext> for ResolvedCommandContext {
    fn from(ctx: CommandContext) -> Self {
        match ctx {
            CommandContext::Builtin { tool_name } => ResolvedCommandContext::Builtin { tool_name },
            CommandContext::Mcp {
                server_name,
                tool_name,
            } => ResolvedCommandContext::Mcp {
                server_name,
                tool_name,
            },
            CommandContext::Skill {
                skill_id,
                display_name,
                ..
            } => ResolvedCommandContext::Skill {
                skill_id,
                display_name,
            },
            CommandContext::Custom { pattern, .. } => ResolvedCommandContext::Custom { pattern },
            CommandContext::None => ResolvedCommandContext::None,
        }
    }
}

/// Handle command.execute RPC request
///
/// Parses a slash command input and returns the resolved command info.
/// Clients can use this for command validation and then execute via
/// `chat.send` with the slash command as the message for full execution.
///
/// # Example Request
///
/// ```json
/// {"jsonrpc":"2.0","method":"command.execute","params":{"input":"/search weather"},"id":1}
/// ```
///
/// # Example Response
///
/// ```json
/// {"jsonrpc":"2.0","result":{"resolved":true,"command":{"name":"search","args":"weather","source_type":"builtin","input":"/search weather","context":{"type":"builtin","tool_name":"search"}}},"id":1}
/// ```
pub async fn handle_execute(
    request: JsonRpcRequest,
    command_parser: Arc<CommandParser>,
) -> JsonRpcResponse {
    let params: ExecuteParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let input = params.input.trim();
    if input.is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Input cannot be empty");
    }

    // Ensure input starts with /
    let slash_input = if input.starts_with('/') {
        input.to_string()
    } else {
        format!("/{}", input)
    };

    // Parse via CommandParser (async, queries ToolRegistry)
    match command_parser.parse_async(&slash_input).await {
        Some(parsed) => {
            let info = ResolvedCommandInfo {
                name: parsed.command_name,
                args: parsed.arguments,
                source_type: source_type_to_string(parsed.source_type),
                input: parsed.full_input,
                context: ResolvedCommandContext::from(parsed.context),
            };

            JsonRpcResponse::success(
                request.id,
                json!({
                    "resolved": true,
                    "command": info,
                }),
            )
        }
        None => JsonRpcResponse::success(
            request.id,
            json!({
                "resolved": false,
                "error": format!("Unknown command: {}", slash_input),
            }),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_list_commands() {
        let request = JsonRpcRequest::with_id("commands.list", None, json!(1));
        let response = handle_list(request).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert!(result["commands"].is_array());

        let commands = result["commands"].as_array().unwrap();
        assert!(!commands.is_empty());

        // Check first command structure
        let first = &commands[0];
        assert!(first["key"].is_string());
        assert!(first["description"].is_string());
        assert!(first["source_type"].is_string());
    }

    #[tokio::test]
    async fn test_list_from_registry() {
        use crate::config::RoutingRuleConfig;

        let registry = ToolRegistry::new();
        let rules = vec![RoutingRuleConfig {
            regex: "^/search".to_string(),
            provider: Some("openai".to_string()),
            system_prompt: Some("Search the web".to_string()),
            ..Default::default()
        }];
        registry.register_custom_commands(&rules).await;

        let request = JsonRpcRequest::with_id("commands.list", None, json!(1));
        let response = handle_list_from_registry(request, &registry).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        let commands = result["commands"].as_array().unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0]["key"], "search");
    }

    #[test]
    fn test_command_info_from_node() {
        let node = CommandNode::new("test", "Test command", CommandType::Action)
            .with_icon("star")
            .with_hint("Test hint")
            .with_source_id("builtin:test")
            .with_source_type(ToolSourceType::Builtin);

        let info = CommandInfo::from(node);

        assert_eq!(info.key, "test");
        assert_eq!(info.description, "Test command");
        assert_eq!(info.icon, "star");
        assert_eq!(info.hint, Some("Test hint".to_string()));
        assert_eq!(info.command_type, "action");
        assert_eq!(info.source_type, "builtin");
    }

    #[tokio::test]
    async fn test_execute_resolved() {
        use crate::config::RoutingRuleConfig;

        let registry = Arc::new(ToolRegistry::new());
        let rules = vec![RoutingRuleConfig {
            regex: "^/search".to_string(),
            provider: Some("openai".to_string()),
            system_prompt: Some("Search the web".to_string()),
            ..Default::default()
        }];
        registry.register_custom_commands(&rules).await;

        let parser = Arc::new(CommandParser::new(registry));
        let request = JsonRpcRequest::with_id(
            "command.execute",
            Some(json!({"input": "/search weather"})),
            json!(1),
        );
        let response = handle_execute(request, parser).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert_eq!(result["resolved"], true);
        assert_eq!(result["command"]["name"], "search");
        assert_eq!(result["command"]["args"], "weather");
        assert_eq!(result["command"]["source_type"], "custom");
    }

    #[tokio::test]
    async fn test_execute_unknown_command() {
        let registry = Arc::new(ToolRegistry::new());
        let parser = Arc::new(CommandParser::new(registry));
        let request = JsonRpcRequest::with_id(
            "command.execute",
            Some(json!({"input": "/nonexistent"})),
            json!(1),
        );
        let response = handle_execute(request, parser).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert_eq!(result["resolved"], false);
        assert!(result["error"].as_str().unwrap().contains("Unknown command"));
    }

    #[tokio::test]
    async fn test_execute_empty_input() {
        let registry = Arc::new(ToolRegistry::new());
        let parser = Arc::new(CommandParser::new(registry));
        let request = JsonRpcRequest::with_id(
            "command.execute",
            Some(json!({"input": ""})),
            json!(1),
        );
        let response = handle_execute(request, parser).await;

        assert!(response.is_error());
    }

    #[tokio::test]
    async fn test_execute_params_deserialization() {
        let json = json!({
            "input": "/search hello",
            "session_id": "agent:main:main:1"
        });
        let params: ExecuteParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.input, "/search hello");
        assert_eq!(params.session_id, Some("agent:main:main:1".to_string()));
    }

    #[test]
    fn test_resolved_command_context_serialization() {
        let ctx = ResolvedCommandContext::Builtin {
            tool_name: "search".to_string(),
        };
        let json = serde_json::to_value(&ctx).unwrap();
        assert_eq!(json["type"], "builtin");
        assert_eq!(json["tool_name"], "search");

        let ctx = ResolvedCommandContext::Mcp {
            server_name: "github".to_string(),
            tool_name: Some("list_repos".to_string()),
        };
        let json = serde_json::to_value(&ctx).unwrap();
        assert_eq!(json["type"], "mcp");
        assert_eq!(json["server_name"], "github");
    }
}
