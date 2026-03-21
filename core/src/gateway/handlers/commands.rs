//! Commands RPC Handlers
//!
//! Handlers for command listing, discovery, and execution.
//! Returns hierarchical tree structure for namespaced commands.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};
use super::parse_params;
use crate::command::{CommandContext, CommandNode, CommandParser, CommandType};
use crate::dispatcher::{ToolRegistry, ToolSourceType, UnifiedTool};
use crate::sync_primitives::Arc;

/// Command info for JSON serialization (backward compat for flat lists)
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

// ============================================================================
// Tree node types for hierarchical command listing
// ============================================================================

/// A child command within a namespace
#[derive(Debug, Clone, Serialize)]
struct ChildCommandNode {
    /// Subcommand name (e.g., "new", "list")
    name: String,
    /// Human-readable description
    hint: String,
    /// Parameter hint (e.g., "[topic]", "<query>")
    #[serde(skip_serializing_if = "Option::is_none")]
    param_hint: Option<String>,
    /// Source type
    source_type: String,
    /// Tool internal ID
    internal_id: String,
}

/// A top-level entry in the command tree
#[derive(Debug, Clone, Serialize)]
struct CommandTreeNode {
    /// Command or namespace name
    name: String,
    /// Whether this is a namespace (has children)
    is_namespace: bool,
    /// Human-readable hint/description
    hint: String,
    /// Parameter hint (for standalone commands)
    #[serde(skip_serializing_if = "Option::is_none")]
    param_hint: Option<String>,
    /// Source type (for standalone commands)
    #[serde(skip_serializing_if = "Option::is_none")]
    source_type: Option<String>,
    /// Tool internal ID (for standalone commands)
    #[serde(skip_serializing_if = "Option::is_none")]
    internal_id: Option<String>,
    /// Children (for namespaces)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    children: Vec<ChildCommandNode>,
}

/// Build a hierarchical tree from a flat list of tools.
///
/// Tools with dots in their name (e.g., "session.new") are grouped under
/// their namespace prefix. Tools without dots are standalone commands.
fn build_command_tree(tools: Vec<UnifiedTool>) -> Vec<CommandTreeNode> {
    // Group: namespace -> Vec<UnifiedTool>
    let mut namespaces: BTreeMap<String, Vec<UnifiedTool>> = BTreeMap::new();
    let mut standalone: Vec<UnifiedTool> = Vec::new();

    for tool in tools {
        if let Some((ns, _)) = tool.name.split_once('.') {
            namespaces
                .entry(ns.to_string())
                .or_default()
                .push(tool);
        } else {
            standalone.push(tool);
        }
    }

    let mut result: Vec<CommandTreeNode> = Vec::new();

    // Add namespace entries
    for (ns_name, children_tools) in namespaces {
        // Build a combined hint from the namespace name
        let hint = format!("{} commands", capitalize(&ns_name));

        let children: Vec<ChildCommandNode> = children_tools
            .into_iter()
            .map(|t| {
                // Extract subcommand name: "session.new" -> "new"
                let sub_name = t
                    .name
                    .strip_prefix(&format!("{}.", ns_name))
                    .unwrap_or(&t.name)
                    .to_string();
                ChildCommandNode {
                    name: sub_name,
                    hint: t.description,
                    param_hint: t.param_hint,
                    source_type: t.source.label().to_lowercase(),
                    internal_id: t.id,
                }
            })
            .collect();

        result.push(CommandTreeNode {
            name: ns_name,
            is_namespace: true,
            hint,
            param_hint: None,
            source_type: None,
            internal_id: None,
            children,
        });
    }

    // Add standalone commands
    for tool in standalone {
        result.push(CommandTreeNode {
            name: tool.name.clone(),
            is_namespace: false,
            hint: tool.description,
            param_hint: tool.param_hint,
            source_type: Some(tool.source.label().to_lowercase()),
            internal_id: Some(tool.id),
            children: Vec::new(),
        });
    }

    result
}

/// Capitalize first letter of a string
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// List all registered commands from ToolRegistry (tree structure)
///
/// Returns a hierarchical command tree where dotted tool names
/// are grouped under their namespace prefix.
///
/// # Example Response
///
/// ```json
/// {
///   "commands": [
///     {
///       "name": "session",
///       "is_namespace": true,
///       "hint": "Session commands",
///       "children": [
///         { "name": "new", "hint": "Start a new session", "param_hint": "[topic]" }
///       ]
///     },
///     {
///       "name": "search",
///       "is_namespace": false,
///       "hint": "Web search",
///       "param_hint": "<query>"
///     }
///   ]
/// }
/// ```
pub async fn handle_list_from_registry(
    request: JsonRpcRequest,
    tool_registry: &ToolRegistry,
) -> JsonRpcResponse {
    let tools: Vec<UnifiedTool> = tool_registry.list_root_commands().await;
    let tree = build_command_tree(tools);

    JsonRpcResponse::success(
        request.id,
        json!({
            "commands": tree
        }),
    )
}

/// List all registered commands (legacy builtin-only handler)
pub async fn handle_list(request: JsonRpcRequest) -> JsonRpcResponse {
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
    /// Namespace prefix (e.g., "session" for "session.new")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Action/subcommand (e.g., "new")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// Arguments after the command name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    /// Internal tool ID
    pub internal_id: String,
    /// Source type: "builtin", "mcp", "skill", "custom", "plugin"
    pub source_type: String,
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

/// Build children list for namespace interaction responses
async fn build_namespace_children(
    tool_registry: &ToolRegistry,
    namespace: &str,
) -> Vec<ChildCommandNode> {
    let children_tools = tool_registry.list_namespace_children(namespace).await;
    children_tools
        .into_iter()
        .map(|t| {
            let sub_name = t
                .name
                .strip_prefix(&format!("{}.", namespace))
                .unwrap_or(&t.name)
                .to_string();
            ChildCommandNode {
                name: sub_name,
                hint: t.description,
                param_hint: t.param_hint,
                source_type: t.source.label().to_lowercase(),
                internal_id: t.id,
            }
        })
        .collect()
}

/// Handle command.execute RPC request
///
/// Parses a slash command input and returns the resolved command info.
///
/// When the input is a namespace (e.g., "/session"), returns needs_interaction
/// with available children. When the subcommand is wrong, returns an error
/// with available children for correction.
///
/// # Example: Resolved command
/// ```json
/// {"resolved":true,"command":{"namespace":"session","action":"new","args":"my topic","internal_id":"builtin:session.new","source_type":"builtin"}}
/// ```
///
/// # Example: Namespace only
/// ```json
/// {"resolved":false,"needs_interaction":true,"namespace":"session","children":[...]}
/// ```
///
/// # Example: Unknown subcommand
/// ```json
/// {"resolved":false,"error":"Unknown subcommand: nw","needs_interaction":true,"namespace":"session","children":[...]}
/// ```
pub async fn handle_execute(
    request: JsonRpcRequest,
    command_parser: Arc<CommandParser>,
    tool_registry: Arc<ToolRegistry>,
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
            // Successfully resolved — decompose the name into namespace + action
            let (namespace, action) = if let Some((ns, act)) = parsed.command_name.split_once('.') {
                (Some(ns.to_string()), Some(act.to_string()))
            } else {
                (None, None)
            };

            let info = ResolvedCommandInfo {
                namespace,
                action,
                args: parsed.arguments,
                internal_id: format!(
                    "{}:{}",
                    source_type_to_string(parsed.source_type),
                    parsed.command_name
                ),
                source_type: source_type_to_string(parsed.source_type),
            };

            JsonRpcResponse::success(
                request.id,
                json!({
                    "resolved": true,
                    "command": info,
                }),
            )
        }
        None => {
            // Not resolved — check if the first word is a namespace
            let without_slash = slash_input.trim_start_matches('/');
            let words: Vec<&str> = without_slash.split_whitespace().collect();
            let first_word = words.first().map(|w| w.to_lowercase()).unwrap_or_default();

            if tool_registry.is_namespace(&first_word).await {
                // It's a known namespace
                let children =
                    build_namespace_children(&tool_registry, &first_word).await;

                if words.len() > 1 {
                    // Had a subcommand that didn't resolve — typo
                    let bad_sub = words[1..].join(" ");
                    JsonRpcResponse::success(
                        request.id,
                        json!({
                            "resolved": false,
                            "error": format!("Unknown subcommand: {}", bad_sub),
                            "needs_interaction": true,
                            "namespace": first_word,
                            "children": children,
                        }),
                    )
                } else {
                    // Just the namespace, no subcommand
                    JsonRpcResponse::success(
                        request.id,
                        json!({
                            "resolved": false,
                            "needs_interaction": true,
                            "namespace": first_word,
                            "children": children,
                        }),
                    )
                }
            } else {
                // Completely unknown command
                JsonRpcResponse::success(
                    request.id,
                    json!({
                        "resolved": false,
                        "error": format!("Unknown command: {}", slash_input),
                    }),
                )
            }
        }
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
    async fn test_list_from_registry_flat_commands() {
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
        // Standalone command (no dot)
        assert_eq!(commands[0]["name"], "search");
        assert_eq!(commands[0]["is_namespace"], false);
    }

    #[tokio::test]
    async fn test_list_from_registry_tree_structure() {
        use crate::dispatcher::ToolSource;

        let registry = ToolRegistry::new();

        // Register namespaced tools directly
        for (id, name, desc) in [
            ("builtin:session.new", "session.new", "Start new session"),
            ("builtin:session.list", "session.list", "List sessions"),
            ("custom:search", "search", "Web search"),
        ] {
            let source = if id.starts_with("builtin:") {
                ToolSource::Builtin
            } else {
                ToolSource::Custom {
                    rule_index: 0,
                }
            };
            registry
                .register_with_conflict_resolution(UnifiedTool::new(id, name, desc, source))
                .await;
        }

        let request = JsonRpcRequest::with_id("commands.list", None, json!(1));
        let response = handle_list_from_registry(request, &registry).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        let commands = result["commands"].as_array().unwrap();

        // Should have 2 entries: "session" namespace + "search" standalone
        assert_eq!(commands.len(), 2);

        // Find the namespace entry
        let session = commands.iter().find(|c| c["name"] == "session").unwrap();
        assert_eq!(session["is_namespace"], true);
        let children = session["children"].as_array().unwrap();
        assert_eq!(children.len(), 2);

        let child_names: Vec<&str> = children
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        assert!(child_names.contains(&"new"));
        assert!(child_names.contains(&"list"));

        // Find the standalone entry
        let search = commands.iter().find(|c| c["name"] == "search").unwrap();
        assert_eq!(search["is_namespace"], false);
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

        let parser = Arc::new(CommandParser::new(registry.clone()));
        let request = JsonRpcRequest::with_id(
            "command.execute",
            Some(json!({"input": "/search weather"})),
            json!(1),
        );
        let response = handle_execute(request, parser, registry).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert_eq!(result["resolved"], true);
        assert_eq!(result["command"]["source_type"], "custom");
        assert!(result["command"]["args"].as_str().unwrap().contains("weather"));
    }

    #[tokio::test]
    async fn test_execute_namespace_only() {
        use crate::dispatcher::ToolSource;

        let registry = Arc::new(ToolRegistry::new());

        // Register namespaced tools
        for (id, name, desc) in [
            ("builtin:session.new", "session.new", "Start new session"),
            ("builtin:session.list", "session.list", "List sessions"),
        ] {
            registry
                .register_with_conflict_resolution(UnifiedTool::new(
                    id,
                    name,
                    desc,
                    ToolSource::Builtin,
                ))
                .await;
        }

        let parser = Arc::new(CommandParser::new(registry.clone()));
        let request = JsonRpcRequest::with_id(
            "command.execute",
            Some(json!({"input": "/session"})),
            json!(1),
        );
        let response = handle_execute(request, parser, registry).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert_eq!(result["resolved"], false);
        assert_eq!(result["needs_interaction"], true);
        assert_eq!(result["namespace"], "session");
        assert!(result["children"].is_array());
        let children = result["children"].as_array().unwrap();
        assert_eq!(children.len(), 2);
    }

    #[tokio::test]
    async fn test_execute_bad_subcommand() {
        use crate::dispatcher::ToolSource;

        let registry = Arc::new(ToolRegistry::new());

        for (id, name, desc) in [
            ("builtin:session.new", "session.new", "Start new session"),
            ("builtin:session.list", "session.list", "List sessions"),
        ] {
            registry
                .register_with_conflict_resolution(UnifiedTool::new(
                    id,
                    name,
                    desc,
                    ToolSource::Builtin,
                ))
                .await;
        }

        let parser = Arc::new(CommandParser::new(registry.clone()));
        let request = JsonRpcRequest::with_id(
            "command.execute",
            Some(json!({"input": "/session nw"})),
            json!(1),
        );
        let response = handle_execute(request, parser, registry).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert_eq!(result["resolved"], false);
        assert_eq!(result["needs_interaction"], true);
        assert_eq!(result["namespace"], "session");
        assert!(result["error"]
            .as_str()
            .unwrap()
            .contains("Unknown subcommand"));
        assert!(result["children"].is_array());
    }

    #[tokio::test]
    async fn test_execute_unknown_command() {
        let registry = Arc::new(ToolRegistry::new());
        let parser = Arc::new(CommandParser::new(registry.clone()));
        let request = JsonRpcRequest::with_id(
            "command.execute",
            Some(json!({"input": "/nonexistent"})),
            json!(1),
        );
        let response = handle_execute(request, parser, registry).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert_eq!(result["resolved"], false);
        assert!(result["error"]
            .as_str()
            .unwrap()
            .contains("Unknown command"));
        // No needs_interaction for completely unknown
        assert!(result.get("needs_interaction").is_none());
    }

    #[tokio::test]
    async fn test_execute_empty_input() {
        let registry = Arc::new(ToolRegistry::new());
        let parser = Arc::new(CommandParser::new(registry.clone()));
        let request = JsonRpcRequest::with_id(
            "command.execute",
            Some(json!({"input": ""})),
            json!(1),
        );
        let response = handle_execute(request, parser, registry).await;

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

    #[test]
    fn test_build_command_tree_mixed() {
        use crate::dispatcher::ToolSource;

        let tools = vec![
            UnifiedTool::new("builtin:session.new", "session.new", "New session", ToolSource::Builtin),
            UnifiedTool::new("builtin:session.list", "session.list", "List sessions", ToolSource::Builtin),
            UnifiedTool::new("custom:search", "search", "Web search", ToolSource::Custom { rule_index: 0 }),
            UnifiedTool::new("builtin:plugin.marketplace.install", "plugin.marketplace.install", "Install plugin", ToolSource::Builtin),
            UnifiedTool::new("builtin:plugin.list", "plugin.list", "List plugins", ToolSource::Builtin),
        ];

        let tree = build_command_tree(tools);

        // Should have 3 entries: namespaces first (BTreeMap order), then standalone
        assert_eq!(tree.len(), 3);

        // Namespaces come first (BTreeMap alphabetical order)
        assert_eq!(tree[0].name, "plugin");
        assert!(tree[0].is_namespace);
        assert_eq!(tree[0].children.len(), 2);

        assert_eq!(tree[1].name, "session");
        assert!(tree[1].is_namespace);
        assert_eq!(tree[1].children.len(), 2);

        // Standalone commands come after namespaces
        assert_eq!(tree[2].name, "search");
        assert!(!tree[2].is_namespace);
    }

    #[test]
    fn test_capitalize() {
        assert_eq!(capitalize("session"), "Session");
        assert_eq!(capitalize(""), "");
        assert_eq!(capitalize("a"), "A");
    }
}
