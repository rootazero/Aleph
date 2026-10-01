//! Commands RPC Handlers
//!
//! Handlers for command listing and discovery (`commands.list`).
//! Returns hierarchical tree structure for namespaced commands.

use std::collections::BTreeMap;

use serde_json::json;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::tool_metadata::{ChannelType, ToolCatalog, UnifiedTool};

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

/// The two tree node types live in `aleph_protocol::commands`, not here.
///
/// They used to be private `Serialize`-only structs, which made their field
/// names a wire contract with no compiler behind it. The CLI declared its own
/// `struct Command { key, description }` — two required fields, neither of
/// which this handler has ever emitted (it sends `name` and `hint`) — so
/// `aleph tools list` died on a serde error and `aleph tools describe <name>`
/// matched `item["key"]`, a comparison that can never be true, and returned the
/// fabricated `tool 'X' not found in commands.list`: a wrong answer that reads
/// as a fact about the server. The TUI read `name` and was right all along.
///
/// Building the shared type makes a rename a compile error here and a loud
/// parse error there.
pub use aleph_protocol::commands::{ChildCommandNode, CommandTreeNode};

/// Known tool namespaces for hierarchical grouping.
const TOOL_NAMESPACES: &[&str] = &[
    "session", "agent", "cron", "skill", "vault", "memory", "image", "plugin", "team", "task",
];

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
///
/// This is the channel-side list face of the verb `commands.list` serves:
/// rows a plugin owns are listed only when `owner_visible(plugin_id, ctx)`
/// says so, with the same daemon-CWD ctx `handle_list_from_registry` derives
/// (its rationale comment applies here verbatim) — otherwise a channel user
/// is offered `/proj:cmd` and then refused by the slash fast path.
pub(crate) async fn render_command_help(
    catalog: &ToolCatalog,
    channel: Option<ChannelType>,
    owner_visible: &(dyn Fn(&str, &crate::extension::visibility::VisibilityCtx) -> bool + Sync),
) -> String {
    let tools = match channel {
        Some(ch) => catalog.list_for_channel(ch).await,
        None => catalog.list_all_for_ui().await,
    };
    let visibility = crate::extension::visibility::VisibilityCtx::from_project_root(None);
    let tools = crate::extension::visibility::retain_visible_owned_commands(
        tools,
        &visibility,
        owner_visible,
    );

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
/// are grouped under their namespace prefix. Rows a plugin owns are
/// listed only when `owner_visible(plugin_id, ctx)` says the daemon's own
/// project may see that plugin (`extension::visibility`, face ④).
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
    owner_visible: &(dyn Fn(&str, &crate::extension::visibility::VisibilityCtx) -> bool + Sync),
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

    // Face ④ of `extension::visibility`, the list half. No project parameter
    // (R2.3: no client sends one). The ctx is the one a project-less run gets
    // — `from_project_root(None)` = daemon CWD — so the list the TUI/CLI see
    // agrees with what a run started from them can use. `owner_visible` is
    // `ExtensionManager::plugin_visible` in production; a closure in tests.
    let visibility = crate::extension::visibility::VisibilityCtx::from_project_root(None);

    let tools: Vec<UnifiedTool> = match channel {
        Some(ch) => tool_registry.list_for_channel(ch).await,
        None => tool_registry.list_root_commands().await,
    };
    let tools = crate::extension::visibility::retain_visible_owned_commands(
        tools,
        &visibility,
        owner_visible,
    );
    let tree = build_command_tree(tools);

    JsonRpcResponse::success(
        request.id,
        json!({
            "commands": tree
        }),
    )
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
        let resp = handle_list_from_registry(req, &registry, &|_, _| true).await;
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
        let resp_all = handle_list_from_registry(req_all, &registry, &|_, _| true).await;
        let names_all: Vec<String> = resp_all.result.unwrap()["commands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names_all.contains(&"ping".to_string()));
        assert!(names_all.contains(&"danger".to_string()));
    }

    /// Face ④ of `extension::visibility`, the list half: a `commands/*.md`
    /// entry whose owning plugin the daemon's own project cannot see is not
    /// listed. The handler takes no project parameter (R2.3: no client sends
    /// one), so the tempdir plugin is the "other project" case by construction.
    #[tokio::test]
    async fn list_from_registry_hides_commands_of_plugins_the_daemon_cannot_see() {
        use crate::tool_metadata::ToolSource;
        let p = tempfile::tempdir().unwrap();
        let registry = ToolCatalog::new();
        for (id, name, source) in [
            (
                "skill:proj:cmd",
                "proj:cmd",
                ToolSource::Skill {
                    id: "proj:cmd".into(),
                    plugin_id: Some("proj".into()),
                },
            ),
            (
                "skill:glob:cmd",
                "glob:cmd",
                ToolSource::Skill {
                    id: "glob:cmd".into(),
                    plugin_id: Some("glob".into()),
                },
            ),
            ("builtin:help", "help", ToolSource::Builtin),
        ] {
            registry
                .register_with_conflict_resolution(UnifiedTool::new(id, name, "d", source))
                .await;
        }
        // `proj` is scoped to a tempdir — never the daemon CWD the handler
        // derives its ctx from — while `glob` is visible everywhere.
        let visible = |id: &str, ctx: &crate::extension::visibility::VisibilityCtx| match id {
            "glob" => true,
            "proj" => crate::extension::visibility::visible_to(
                &crate::extension::visibility::ScopeKey::project(p.path()),
                ctx,
            ),
            _ => false,
        };
        let request = JsonRpcRequest::with_id("commands.list", None, json!(1));
        let resp = handle_list_from_registry(request, &registry, &visible).await;
        let got: Vec<String> = resp.result.unwrap()["commands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap().to_string())
            .collect();
        assert!(!got.contains(&"proj:cmd".to_string()), "{got:?}");
        assert!(got.contains(&"glob:cmd".to_string()), "{got:?}");
        assert!(got.contains(&"help".to_string()), "{got:?}");
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
        let response = handle_list_from_registry(request, &registry, &|_, _| true).await;

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
        let response = handle_list_from_registry(request, &registry, &|_, _| true).await;

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

    /// The live `commands.list` response must be readable by the type every
    /// thin client parses, and a namespaced tool must be reachable by its
    /// canonical name.
    ///
    /// This is the reconciliation the CLI could not do for itself. Its own
    /// tests only ever asserted on `json!` literals it had just written —
    /// which tests `serde_json`, not the contract — so a `struct Command {
    /// key, description }` that named two fields this handler has never
    /// emitted stayed green while `aleph tools list` failed to deserialize on
    /// every server and `aleph tools describe` reported every existing tool as
    /// "not found".
    #[tokio::test]
    async fn the_live_response_parses_as_the_shared_contract() {
        use crate::tool_metadata::ToolSource;

        let registry = ToolCatalog::new();
        for (id, name, desc, source) in [
            (
                "builtin:session_new",
                "session_new",
                "New session",
                ToolSource::Builtin,
            ),
            (
                "builtin:search",
                "search",
                "Web search",
                ToolSource::Builtin,
            ),
        ] {
            registry
                .register_with_conflict_resolution(UnifiedTool::new(id, name, desc, source))
                .await;
        }

        let request = JsonRpcRequest::with_id("commands.list", None, json!(1));
        let response = handle_list_from_registry(request, &registry, &|_, _| true).await;
        let result = response.result.expect("commands.list must succeed");

        let parsed: aleph_protocol::commands::CommandListResponse = serde_json::from_value(result)
            .expect(
                "commands.list must parse as aleph_protocol::commands::CommandListResponse; \
                 a client that cannot read this response reports an empty catalogue",
            );

        // Derived, not restated: the lookup a client performs, run against the
        // response the server actually produced.
        let found = CommandTreeNode::find(&parsed.commands, "session_new")
            .expect("`session_new` is a child of the `session` namespace and must resolve");
        assert_eq!(found.internal_id(), Some("builtin:session_new"));
        assert_eq!(found.hint(), "New session");

        assert!(CommandTreeNode::find(&parsed.commands, "search").is_some());
        assert!(CommandTreeNode::find(&parsed.commands, "no_such_tool").is_none());
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

        let help = render_command_help(&registry, None, &|_, _| true).await;

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

    /// `/help` is the channel-side list face of the same verb `commands.list`
    /// serves: a plugin-owned row the daemon cannot see must not be offered.
    #[tokio::test]
    async fn render_command_help_hides_commands_of_plugins_the_daemon_cannot_see() {
        use crate::tool_metadata::ToolSource;

        let registry = ToolCatalog::new();
        registry.register_builtin_tools().await;
        // A plugin-owned skill row, carrying the usage hint `register_skills`
        // gives every skill row (that hint is what makes it a curated line).
        registry
            .register_with_conflict_resolution(
                UnifiedTool::new(
                    "skill:proj:cmd",
                    "proj:cmd",
                    "a plugin command",
                    ToolSource::Skill {
                        id: "proj:cmd".into(),
                        plugin_id: Some("proj".into()),
                    },
                )
                .with_usage("/proj:cmd [input]"),
            )
            .await;

        let hidden = render_command_help(&registry, None, &|_, _| false).await;
        assert!(!hidden.contains("proj:cmd"), "{hidden}");
        assert!(
            hidden.contains("/help"),
            "unowned rows still listed:\n{hidden}"
        );
        let shown = render_command_help(&registry, None, &|_, _| true).await;
        assert!(shown.contains("proj:cmd"), "{shown}");
    }
}
