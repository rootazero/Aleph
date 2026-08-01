//! Commands RPC Handlers
//!
//! Handlers for command listing, discovery, and execution.
//! Returns hierarchical tree structure for namespaced commands.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS};
use super::parse_params;
use crate::command::{CommandContext, CommandParser};
use crate::sync_primitives::Arc;
use crate::tool_metadata::{ChannelType, ToolCatalog, ToolSourceType, UnifiedTool};

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

/// Map a client-supplied `interface` hint to a [`ChannelType`] for visibility
/// filtering. Unknown or absent values yield `None`, meaning "no filter — list
/// every active command" (backward compatible with clients that send no hint).
fn channel_from_interface(interface: &str) -> Option<ChannelType> {
    match interface.trim().to_lowercase().as_str() {
        "panel" | "webchat" | "web" => Some(ChannelType::Panel),
        "tui" | "cli" | "terminal" => Some(ChannelType::Cli),
        "telegram" => Some(ChannelType::Telegram),
        "discord" => Some(ChannelType::Discord),
        "imessage" => Some(ChannelType::IMessage),
        _ => None,
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

/// Known tool namespaces for hierarchical grouping.
const TOOL_NAMESPACES: &[&str] = &[
    "session", "agent", "cron", "skill", "vault", "memory", "image", "plugin", "team", "task",
];

/// Decompose a canonical command name into an optional `(namespace, action)`
/// pair, mirroring [`build_command_tree`]'s grouping exactly.
///
/// Canonical names are underscore-separated (`session_new`), so only a name
/// whose first underscore-delimited segment is a known [`TOOL_NAMESPACES`]
/// entry is split — `web_fetch` / `search` stay standalone (`None`). The old
/// `command.execute` path split on `.`, which never matched an underscore name,
/// so `namespace` / `action` were *always* `None`, silently contradicting the
/// documented response shape.
fn split_namespace_action(command_name: &str) -> (Option<String>, Option<String>) {
    for ns in TOOL_NAMESPACES {
        if let Some(action) = command_name
            .strip_prefix(ns)
            .and_then(|rest| rest.strip_prefix('_'))
            .filter(|action| !action.is_empty())
        {
            return (Some((*ns).to_string()), Some(action.to_string()));
        }
    }
    (None, None)
}

/// Build a hierarchical tree from a flat list of tools.
///
/// Tools with a known namespace prefix (e.g., "`session_new`") are grouped under
/// their namespace. Other tools are standalone commands.
fn build_command_tree(tools: Vec<UnifiedTool>) -> Vec<CommandTreeNode> {
    // Group: namespace -> Vec<UnifiedTool>
    let mut namespaces: BTreeMap<String, Vec<UnifiedTool>> = BTreeMap::new();
    let mut standalone: Vec<UnifiedTool> = Vec::new();

    // Surface aliases as standalone command entries so shortcuts like `/new`
    // (an alias of `session_new`) remain discoverable in completion menus even
    // though they are no longer backed by a separate phantom tool. Collected up
    // front because the grouping loop below consumes `tools`.
    let alias_nodes: Vec<CommandTreeNode> = tools
        .iter()
        .flat_map(|tool| {
            tool.aliases.iter().map(move |alias| CommandTreeNode {
                name: alias.clone(),
                is_namespace: false,
                hint: tool.description.clone(),
                param_hint: tool.param_hint.clone(),
                source_type: Some(tool.source.label().to_lowercase()),
                internal_id: Some(tool.id.clone()),
                children: Vec::new(),
            })
        })
        .collect();

    for tool in tools {
        let ns = TOOL_NAMESPACES.iter().find(|&&ns| {
            tool.name.starts_with(ns) && tool.name.get(ns.len()..ns.len() + 1) == Some("_")
        });
        if let Some(&ns) = ns {
            namespaces.entry(ns.to_string()).or_default().push(tool);
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
                // Extract subcommand name: "session_new" -> "new"
                let sub_name = t
                    .name
                    .strip_prefix(&format!("{ns_name}_"))
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

    // Append alias shortcuts after their canonical commands.
    result.extend(alias_nodes);

    result
}

/// Render a human-readable `/help` listing of user-facing slash commands.
///
/// Powers the inbound router's `/help` handler on text channels (Telegram /
/// Slack / Discord), where there is no completion menu. Panel/CLI already
/// surface discovery via the `commands.list` RPC + completion UI.
///
/// Scannability over exhaustiveness: the ~130 bare executor tools (registered
/// nameless in the definitions loop, no `usage`, no alias) are folded into a
/// one-line namespace hint rather than dumped. A command is "user-facing" if it
/// carries a curated `usage` hint (builtins / skills / plugins / custom
/// commands) or a friendly alias seeded from `tool_metadata::aliases`
/// (`/model`→select_model, …). `channel` scopes visibility when known.
pub(crate) async fn render_command_help(
    catalog: &ToolCatalog,
    channel: Option<ChannelType>,
) -> String {
    let tools = match channel {
        Some(ch) => catalog.list_for_channel(ch).await,
        None => catalog.list_all_for_ui().await,
    };

    let mut curated: Vec<&UnifiedTool> = tools
        .iter()
        .filter(|t| t.usage.is_some() || !t.aliases.is_empty())
        .collect();
    curated.sort_by(|a, b| a.sort_order.cmp(&b.sort_order).then(a.name.cmp(&b.name)));

    let mut out = String::from("Available commands:");
    for t in &curated {
        let invocation = t.usage.clone().unwrap_or_else(|| format!("/{}", t.name));
        let aliases = if t.aliases.is_empty() {
            String::new()
        } else {
            format!(" (/{})", t.aliases.join(", /"))
        };
        out.push_str(&format!("\n{invocation}{aliases} — {}", t.description));
    }

    // Namespaced families (session/agent/memory/…) collapse to one hint line so
    // their many sub-commands don't flood the listing. A family is present when
    // at least one active tool carries its `<ns>_` prefix.
    let namespaces: Vec<String> = TOOL_NAMESPACES
        .iter()
        .filter(|ns| {
            tools
                .iter()
                .any(|t| t.name.starts_with(*ns) && t.name.get(ns.len()..=ns.len()) == Some("_"))
        })
        .map(|ns| format!("/{ns}"))
        .collect();
    if !namespaces.is_empty() {
        out.push_str("\n\nType a namespace for its sub-commands: ");
        out.push_str(&namespaces.join(", "));
    }

    out
}

/// Capitalize first letter of a string
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// List all registered commands from `ToolCatalog` (tree structure)
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
    tool_registry: &ToolCatalog,
) -> JsonRpcResponse {
    // Honor the optional `interface` hint that clients (TUI, Panel, …) already
    // send: when it maps to a known channel, list only the commands visible to
    // that channel; otherwise fall back to the full root listing. This wires the
    // existing `visible_channels` / `list_for_channel` infrastructure (e.g.
    // confirmation-requiring tools are hidden from channels lacking a
    // confirmation UI) into the live `commands.list` RPC.
    let channel = request
        .params
        .as_ref()
        .and_then(|p| p.get("interface"))
        .and_then(|v| v.as_str())
        .and_then(channel_from_interface);

    let tools: Vec<UnifiedTool> = match channel {
        Some(ch) => tool_registry.list_for_channel(ch).await,
        None => tool_registry.list_root_commands().await,
    };
    let tree = build_command_tree(tools);

    JsonRpcResponse::success(
        request.id,
        json!({
            "commands": tree
        }),
    )
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
    /// Namespace prefix (e.g., "session" for "`session_new`")
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
}

impl From<CommandContext> for ResolvedCommandContext {
    fn from(ctx: CommandContext) -> Self {
        match ctx {
            CommandContext::Builtin { tool_name } => Self::Builtin { tool_name },
            CommandContext::Mcp {
                server_name,
                tool_name,
            } => Self::Mcp {
                server_name,
                tool_name,
            },
            CommandContext::Skill {
                skill_id,
                display_name,
                ..
            } => Self::Skill {
                skill_id,
                display_name,
            },
            CommandContext::Custom { pattern, .. } => Self::Custom { pattern },
        }
    }
}

/// Build children list for namespace interaction responses
async fn build_namespace_children(
    tool_registry: &ToolCatalog,
    namespace: &str,
) -> Vec<ChildCommandNode> {
    let children_tools = tool_registry.list_namespace_children(namespace).await;
    children_tools
        .into_iter()
        .map(|t| {
            let sub_name = t
                .name
                .strip_prefix(&format!("{namespace}_"))
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
/// When the input is a namespace (e.g., "/session"), returns `needs_interaction`
/// with available children. When the subcommand is wrong, returns an error
/// with available children for correction.
///
/// # Example: Resolved command
/// ```json
/// {"resolved":true,"command":{"namespace":"session","action":"new","args":"my topic","internal_id":"builtin:session_new","source_type":"builtin"}}
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
    tool_registry: Arc<ToolCatalog>,
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
        format!("/{input}")
    };

    // Parse via CommandParser (async, queries ToolCatalog)
    match command_parser.parse_async(&slash_input).await {
        Some(parsed) => {
            // Successfully resolved — decompose the underscore-separated
            // canonical name into namespace + action using the same namespace
            // table that `build_command_tree` groups by.
            let (namespace, action) = split_namespace_action(&parsed.command_name);

            let info = ResolvedCommandInfo {
                namespace,
                action,
                args: parsed.arguments,
                // Canonical registry id, carried through the parser — correct
                // for every source (mcp:srv:tool, plugin:id:name,
                // custom:idx:name), unlike the old `{source}:{name}` rebuild.
                internal_id: parsed.tool_id,
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
            // Not resolved — check if the first word is a namespace. Strip
            // the Telegram `@botname` suffix first so `/session@MyBot` is
            // classified as the `session` namespace, not as an unknown
            // `session@mybot` token that misses every prefix check and the
            // "did you mean?" suggester (the same suffix is already
            // stripped by `resolve_command` on the happy path).
            let without_slash = slash_input.trim_start_matches('/');
            let words: Vec<&str> = without_slash.split_whitespace().collect();
            let first_word = words
                .first()
                .copied()
                .map(|w| w.split_once('@').map_or(w, |(n, _)| n).to_lowercase())
                .unwrap_or_default();

            if tool_registry.is_namespace(&first_word).await {
                // It's a known namespace
                let children = build_namespace_children(&tool_registry, &first_word).await;

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
                // Completely unknown command — offer near-matches ("did you
                // mean?") via the shared registry suggester so the panel RPC
                // path matches the channel router's behavior instead of dead-
                // ending. `suggestions` may be an empty array.
                let suggestions: Vec<String> = tool_registry
                    .suggest_commands(&first_word, 3)
                    .await
                    .into_iter()
                    .map(|n| format!("/{n}"))
                    .collect();
                JsonRpcResponse::success(
                    request.id,
                    json!({
                        "resolved": false,
                        "error": format!("Unknown command: {}", slash_input),
                        "suggestions": suggestions,
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

    #[test]
    fn test_channel_from_interface_mapping() {
        assert_eq!(channel_from_interface("panel"), Some(ChannelType::Panel));
        assert_eq!(channel_from_interface("WebChat"), Some(ChannelType::Panel));
        assert_eq!(channel_from_interface("tui"), Some(ChannelType::Cli));
        assert_eq!(channel_from_interface(" cli "), Some(ChannelType::Cli));
        assert_eq!(
            channel_from_interface("telegram"),
            Some(ChannelType::Telegram)
        );
        // Unknown / empty hints fall back to "no filter".
        assert_eq!(channel_from_interface("carrier-pigeon"), None);
        assert_eq!(channel_from_interface(""), None);
    }

    #[tokio::test]
    async fn test_list_from_registry_filters_by_channel() {
        use crate::tool_metadata::ToolSource;

        let registry = ToolCatalog::new();

        // A tool restricted to Panel + CLI only.
        let panel_only = UnifiedTool::new(
            "builtin:danger",
            "danger",
            "Dangerous op",
            ToolSource::Builtin,
        )
        .with_visible_channels(vec![ChannelType::Panel, ChannelType::Cli]);
        registry.register_with_conflict_resolution(panel_only).await;
        // A tool visible everywhere (empty visible_channels).
        registry
            .register_with_conflict_resolution(UnifiedTool::new(
                "builtin:ping",
                "ping",
                "Ping",
                ToolSource::Builtin,
            ))
            .await;

        // Telegram hint should hide the Panel/CLI-only command.
        let req = JsonRpcRequest::with_id(
            "commands.list",
            Some(json!({"interface": "telegram"})),
            json!(1),
        );
        let resp = handle_list_from_registry(req, &registry).await;
        let names: Vec<String> = resp.result.unwrap()["commands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"ping".to_string()));
        assert!(!names.contains(&"danger".to_string()));

        // No hint → no filter → both commands present.
        let req_all = JsonRpcRequest::with_id("commands.list", None, json!(1));
        let resp_all = handle_list_from_registry(req_all, &registry).await;
        let names_all: Vec<String> = resp_all.result.unwrap()["commands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names_all.contains(&"ping".to_string()));
        assert!(names_all.contains(&"danger".to_string()));
    }

    #[tokio::test]
    async fn test_list_from_registry_flat_commands() {
        use crate::config::RoutingRuleConfig;

        let registry = ToolCatalog::new();
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
        use crate::tool_metadata::ToolSource;

        let registry = ToolCatalog::new();

        // Register namespaced tools directly
        for (id, name, desc) in [
            ("builtin:session_new", "session_new", "Start new session"),
            ("builtin:session_list", "session_list", "List sessions"),
            ("custom:search", "search", "Web search"),
        ] {
            let source = if id.starts_with("builtin:") {
                ToolSource::Builtin
            } else {
                ToolSource::Custom { rule_index: 0 }
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

    #[tokio::test]
    async fn test_execute_resolved() {
        use crate::config::RoutingRuleConfig;

        let registry = Arc::new(ToolCatalog::new());
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
        assert!(result["command"]["args"]
            .as_str()
            .unwrap()
            .contains("weather"));
    }

    #[tokio::test]
    async fn test_execute_namespace_only() {
        use crate::tool_metadata::ToolSource;

        let registry = Arc::new(ToolCatalog::new());

        // Register namespaced tools
        for (id, name, desc) in [
            ("builtin:session_new", "session_new", "Start new session"),
            ("builtin:session_list", "session_list", "List sessions"),
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

    /// Telegram-style `@botname` suffix on a bare namespace input must still
    /// resolve to the namespace listing, not silently fall through to the
    /// "unknown command" + did-you-mean path.
    #[tokio::test]
    async fn test_execute_namespace_only_with_at_bot_suffix() {
        use crate::tool_metadata::ToolSource;

        let registry = Arc::new(ToolCatalog::new());

        for (id, name, desc) in [
            ("builtin:session_new", "session_new", "Start new session"),
            ("builtin:session_list", "session_list", "List sessions"),
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
            Some(json!({"input": "/session@MyBot"})),
            json!(1),
        );
        let response = handle_execute(request, parser, registry).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert_eq!(result["resolved"], false);
        assert_eq!(result["needs_interaction"], true);
        assert_eq!(result["namespace"], "session");
        let children = result["children"].as_array().unwrap();
        assert_eq!(children.len(), 2);
    }

    /// `@botname` on a subcommand typo must still surface the unknown-
    /// subcommand error keyed under the right namespace, with the bot
    /// suffix dropped before the namespace check.
    #[tokio::test]
    async fn test_execute_bad_subcommand_with_at_bot_suffix() {
        use crate::tool_metadata::ToolSource;

        let registry = Arc::new(ToolCatalog::new());

        for (id, name, desc) in [
            ("builtin:session_new", "session_new", "Start new session"),
            ("builtin:session_list", "session_list", "List sessions"),
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
            Some(json!({"input": "/session@MyBot nw"})),
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
        let children = result["children"].as_array().unwrap();
        assert_eq!(children.len(), 2);
    }

    #[tokio::test]
    async fn test_execute_bad_subcommand() {
        use crate::tool_metadata::ToolSource;

        let registry = Arc::new(ToolCatalog::new());

        for (id, name, desc) in [
            ("builtin:session_new", "session_new", "Start new session"),
            ("builtin:session_list", "session_list", "List sessions"),
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
        let registry = Arc::new(ToolCatalog::new());
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
    async fn test_execute_unknown_command_offers_suggestions() {
        use crate::config::RoutingRuleConfig;

        let registry = Arc::new(ToolCatalog::new());
        let rules = vec![RoutingRuleConfig {
            regex: "^/search".to_string(),
            provider: Some("openai".to_string()),
            system_prompt: Some("Web search".to_string()),
            ..Default::default()
        }];
        registry.register_custom_commands(&rules).await;

        let parser = Arc::new(CommandParser::new(registry.clone()));
        let request = JsonRpcRequest::with_id(
            "command.execute",
            Some(json!({"input": "/serch"})),
            json!(1),
        );
        let response = handle_execute(request, parser, registry).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert_eq!(result["resolved"], false);
        let suggestions = result["suggestions"].as_array().unwrap();
        assert!(suggestions.iter().any(|s| s == "/search"));
    }

    #[tokio::test]
    async fn test_execute_empty_input() {
        let registry = Arc::new(ToolCatalog::new());
        let parser = Arc::new(CommandParser::new(registry.clone()));
        let request =
            JsonRpcRequest::with_id("command.execute", Some(json!({"input": ""})), json!(1));
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
        use crate::tool_metadata::ToolSource;

        let tools = vec![
            UnifiedTool::new(
                "builtin:session_new",
                "session_new",
                "New session",
                ToolSource::Builtin,
            ),
            UnifiedTool::new(
                "builtin:session_list",
                "session_list",
                "List sessions",
                ToolSource::Builtin,
            ),
            UnifiedTool::new(
                "custom:search",
                "search",
                "Web search",
                ToolSource::Custom { rule_index: 0 },
            ),
            UnifiedTool::new(
                "builtin:plugin_install",
                "plugin_install",
                "Install plugin",
                ToolSource::Builtin,
            ),
            UnifiedTool::new(
                "builtin:plugin_list",
                "plugin_list",
                "List plugins",
                ToolSource::Builtin,
            ),
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

    #[test]
    fn test_split_namespace_action() {
        // Known namespace + action → split.
        assert_eq!(
            split_namespace_action("session_new"),
            (Some("session".to_string()), Some("new".to_string()))
        );
        assert_eq!(
            split_namespace_action("plugin_marketplace_install"),
            (
                Some("plugin".to_string()),
                Some("marketplace_install".to_string())
            )
        );
        // Non-namespace underscore name stays standalone (not "web"/"fetch").
        assert_eq!(split_namespace_action("web_fetch"), (None, None));
        // Bare command with no underscore.
        assert_eq!(split_namespace_action("search"), (None, None));
        // Namespace word alone (no action) is not split.
        assert_eq!(split_namespace_action("session"), (None, None));
    }

    /// A resolved namespaced builtin must populate `namespace`/`action` (the
    /// old `.`-split left them `None`) and return the canonical registry id as
    /// `internal_id` (not a `{source}:{name}` rebuild).
    #[tokio::test]
    async fn test_execute_resolved_namespace_and_internal_id() {
        use crate::tool_metadata::ToolSource;

        let registry = Arc::new(ToolCatalog::new());
        registry
            .register_with_conflict_resolution(UnifiedTool::new(
                "builtin:session_new",
                "session_new",
                "Start a new session",
                ToolSource::Builtin,
            ))
            .await;

        let parser = Arc::new(CommandParser::new(registry.clone()));
        let request = JsonRpcRequest::with_id(
            "command.execute",
            Some(json!({"input": "/session new my topic"})),
            json!(1),
        );
        let response = handle_execute(request, parser, registry).await;

        assert!(response.is_success());
        let result = response.result.unwrap();
        assert_eq!(result["resolved"], true);
        let cmd = &result["command"];
        assert_eq!(cmd["namespace"], "session");
        assert_eq!(cmd["action"], "new");
        assert_eq!(cmd["args"], "my topic");
        assert_eq!(cmd["internal_id"], "builtin:session_new");
        assert_eq!(cmd["source_type"], "builtin");
    }

    /// `/help` rendering must (1) list curated commands with their usage, (2)
    /// surface a friendly alias seeded from `tool_metadata::aliases`, (3) fold
    /// namespaced families into one hint line, and (4) omit the bare executor
    /// tools that carry neither a usage hint nor an alias — otherwise the
    /// listing drowns in ~130 raw tool names.
    #[tokio::test]
    async fn render_command_help_curates_and_folds() {
        use crate::tool_metadata::ToolSource;

        let registry = ToolCatalog::new();
        registry.register_builtin_tools().await;

        // A bare executor tool with a seeded friendly alias (as the catalog
        // builder's definitions loop would produce for select_model).
        registry
            .register_with_conflict_resolution(
                UnifiedTool::new(
                    "builtin:select_model",
                    "select_model",
                    "Switch the active model",
                    ToolSource::Builtin,
                )
                .with_aliases(["model"]),
            )
            .await;
        // A namespaced tool — must fold into the /session hint, not list raw.
        registry
            .register_with_conflict_resolution(UnifiedTool::new(
                "builtin:session_list",
                "session_list",
                "List sessions",
                ToolSource::Builtin,
            ))
            .await;
        // A bare tool with no usage and no alias — must be omitted.
        registry
            .register_with_conflict_resolution(UnifiedTool::new(
                "builtin:web_fetch",
                "web_fetch",
                "Fetch a URL",
                ToolSource::Builtin,
            ))
            .await;

        let help = render_command_help(&registry, None).await;

        assert!(help.contains("/help"), "curated /help must appear:\n{help}");
        assert!(
            help.contains("/model"),
            "seeded alias /model must be surfaced:\n{help}"
        );
        assert!(
            help.contains("/session"),
            "namespaced family must fold into a /session hint:\n{help}"
        );
        assert!(
            !help.contains("web_fetch"),
            "bare no-usage no-alias tool must be omitted:\n{help}"
        );
        // The namespace fold line, not a raw per-tool dump.
        assert!(
            help.contains("namespace"),
            "fold hint line missing:\n{help}"
        );
    }
}
