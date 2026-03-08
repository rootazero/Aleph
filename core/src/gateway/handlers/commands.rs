//! Commands RPC Handlers
//!
//! Handlers for command listing and discovery.

use serde::Serialize;
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::command::{CommandNode, CommandType};
use crate::dispatcher::{ToolRegistry, ToolSourceType, UnifiedTool};

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
}
