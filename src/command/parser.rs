//! Unified Slash Command Parser
//!
//! Delegates all command resolution to `ToolCatalog`.

use crate::sync_primitives::Arc;
use crate::tool_metadata::{ToolCatalog, ToolSource, ToolSourceType, UnifiedTool};

/// Parsed command result
#[derive(Debug, Clone)]
pub struct ParsedCommand {
    /// Command source type
    pub source_type: ToolSourceType,
    /// Canonical [`UnifiedTool::name`] of the resolved tool — **not** the
    /// literal word the user typed. Aliases (`/new` → `session_new`) and the
    /// Telegram `@botname` suffix are resolved away by `ToolCatalog` before
    /// this struct is built, so the value here is always the registry's
    /// canonical name. Use a substring of the original input if you need to
    /// echo back what the user typed.
    pub command_name: String,
    /// Canonical registry id of the resolved tool, verbatim from
    /// [`UnifiedTool::id`] (e.g. `builtin:session_new`, `mcp:fs:read_file`,
    /// `plugin:diag:ping`, `custom:3:translate`).
    ///
    /// `resolve_command` already knows the full id; carrying it here means
    /// downstream consumers (the `command.execute` RPC, the channel fast-path
    /// serializer) no longer reconstruct it lossily from `source_type` +
    /// `command_name` — a reconstruction that silently dropped the MCP server,
    /// plugin id, and custom rule-index segments.
    pub tool_id: String,
    /// Arguments after the command name
    pub arguments: Option<String>,
    /// Command-specific context
    pub context: CommandContext,
    /// The plugin that owns the resolved entry, as
    /// `extension::visibility::catalog_owner` derives it (`ToolSource::Plugin`,
    /// a plugin-registered `ToolSource::Skill`, or a plugin-declared MCP
    /// server), for the visibility gate at the mode consumer. `None` = not
    /// plugin-owned.
    pub owning_plugin: Option<String>,
}

/// Command-specific context based on source type.
///
/// `Builtin` is the **dispatch shape**, not the source: `Builtin` /
/// `Native` / `Plugin` all land here. For `Builtin` and `Native` the
/// `tool_name` field holds `UnifiedTool::name` (bare, e.g. `session_new`);
/// for `Plugin` it holds the full `UnifiedTool::id` (namespaced, e.g.
/// `plugin:diagnostics:ping`) — both are valid keys for the direct-tool
/// fast path. Read `source_type` on the enclosing `ParsedCommand` for the
/// actual source.
#[derive(Debug, Clone)]
pub enum CommandContext {
    /// Builtin / native / plugin command — direct-tool fast path.
    Builtin {
        /// Direct-tool dispatch target. See the variant doc for what each
        /// producer writes here.
        tool_name: String,
    },
    /// MCP tool context
    Mcp {
        /// Server name
        server_name: String,
    },
    /// Skill context
    Skill {
        /// Skill ID
        skill_id: String,
        /// Skill name for display
        display_name: String,
        /// The skill's declared tool scope, validated at registration.
        /// `None` = the skill declared nothing (allow-all, the behaviour every
        /// skill shipped with); `Some(vec![])` = explicit deny-all. Do not
        /// flatten: an empty allow-set means allow-all downstream.
        allowed_tools: Option<Vec<String>>,
    },
    /// Custom command context
    Custom {
        /// System prompt to inject
        system_prompt: Option<String>,
        /// Rule regex pattern
        pattern: String,
    },
}

/// Unified command parser — delegates to `ToolCatalog`
pub struct CommandParser {
    /// Tool registry for command resolution
    tool_registry: Arc<ToolCatalog>,
}

impl CommandParser {
    /// Create a new command parser backed by `ToolCatalog`
    #[must_use]
    pub const fn new(tool_registry: Arc<ToolCatalog>) -> Self {
        Self { tool_registry }
    }

    /// Parse user input as a slash command (async)
    ///
    /// Returns `Some(ParsedCommand)` if the input matches a registered command.
    /// Only processes inputs starting with '/'.
    pub async fn parse_async(&self, input: &str) -> Option<ParsedCommand> {
        let trimmed = input.trim();
        if !trimmed.starts_with('/') {
            return None;
        }

        let resolved = self.tool_registry.resolve_command(trimmed).await?;

        // `ParsedCommand` needs `name`/`id` *and* context derivation consumes
        // the tool, so those two fields are cloned once; every other field
        // moves into the `CommandContext` without cloning.
        let source_type = ToolSourceType::from(&resolved.tool.source);
        // The list faces filter on the same derivation, so what `/help`
        // offers and what the fast path admits cannot disagree.
        let owning_plugin =
            crate::extension::visibility::catalog_owner(&resolved.tool).map(str::to_owned);
        let command_name = resolved.tool.name.clone();
        let tool_id = resolved.tool.id.clone();
        let context = tool_to_command_context(resolved.tool);

        Some(ParsedCommand {
            source_type,
            command_name,
            tool_id,
            arguments: resolved.arguments,
            context,
            owning_plugin,
        })
    }

    /// Get a reference to the underlying `ToolCatalog`
    #[must_use]
    pub const fn tool_registry(&self) -> &Arc<ToolCatalog> {
        &self.tool_registry
    }
}

/// Derive `CommandContext` from `UnifiedTool` fields.
///
/// Takes the tool by value so every context field moves instead of cloning;
/// the caller clones `name`/`id` up front for `ParsedCommand`.
fn tool_to_command_context(tool: UnifiedTool) -> CommandContext {
    match tool.source {
        ToolSource::Builtin | ToolSource::Native => CommandContext::Builtin {
            tool_name: tool.name,
        },
        ToolSource::Mcp { server } => CommandContext::Mcp {
            server_name: server,
        },
        // No `instructions` field: skill registration deliberately leaves
        // `routing_system_prompt` unset. The skill's description reaches the
        // model through the `<available_skills>` block and its body only
        // through `skill_read`; carrying a third copy here had no reader.
        ToolSource::Skill { id, .. } => CommandContext::Skill {
            skill_id: id,
            display_name: tool.display_name,
            allowed_tools: tool.routing_capabilities,
        },
        ToolSource::Custom { .. } => CommandContext::Custom {
            system_prompt: tool.routing_system_prompt,
            // No `provider`: `RoutingRuleConfig::provider` is required by
            // config validation and read by nothing. There is no agent-loop
            // routing pass that resolves it — an earlier version of this
            // comment claimed there was, sourced from a struct doc that
            // contradicted its own module doc. `register_custom_commands`
            // settles it: it copies `regex` and `system_prompt` onto the tool
            // and never touches `provider`.
            pattern: tool.routing_regex.unwrap_or(tool.name),
        },
        ToolSource::Plugin { .. } => CommandContext::Builtin {
            // Plugin tools live in the tool registry under their namespaced id
            // (`plugin:<plugin_id>:<name>`) and are invoked through the
            // direct-tool fast path. Routing them as `Mcp` mangled the id into
            // `mcp__plugin:<id>_<name>`, which never matched a registered tool,
            // so every plugin slash command failed with a hard execution error.
            tool_name: tool.id,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RoutingRuleConfig;

    fn create_test_registry() -> Arc<ToolCatalog> {
        Arc::new(ToolCatalog::new())
    }

    #[tokio::test]
    async fn test_parse_async_found() {
        let registry = create_test_registry();
        let rules = vec![RoutingRuleConfig {
            regex: "^/search".to_string(),
            provider: Some("openai".to_string()),
            system_prompt: Some("Search the web".to_string()),
            ..Default::default()
        }];
        registry.register_custom_commands(&rules).await;

        let parser = CommandParser::new(registry);
        let result = parser.parse_async("/search weather").await;
        assert!(result.is_some());
        let cmd = result.unwrap();
        assert_eq!(cmd.command_name, "search");
        assert_eq!(cmd.arguments, Some("weather".to_string()));
        assert!(matches!(cmd.source_type, ToolSourceType::Custom));
    }

    #[tokio::test]
    async fn test_parse_async_not_found() {
        let registry = create_test_registry();
        let parser = CommandParser::new(registry);
        assert!(parser.parse_async("/unknown").await.is_none());
    }

    #[tokio::test]
    async fn test_parse_async_not_slash() {
        let registry = create_test_registry();
        let parser = CommandParser::new(registry);
        assert!(parser.parse_async("hello").await.is_none());
    }

    /// A plugin slash command must resolve to a direct-tool context carrying the
    /// registry's canonical id (`plugin:<id>:<name>`). Routing it as `Mcp`
    /// previously mangled the id and broke every plugin slash command.
    #[tokio::test]
    async fn test_parse_async_plugin_routes_to_direct_tool() {
        let registry = create_test_registry();
        registry
            .register_plugin_tools(&[(
                "diagnostics".to_string(),
                "ping".to_string(),
                "Ping the host".to_string(),
            )])
            .await;

        let parser = CommandParser::new(registry);
        let cmd = parser
            .parse_async("/ping localhost")
            .await
            .expect("plugin slash command should resolve");

        assert_eq!(cmd.command_name, "ping");
        assert_eq!(cmd.arguments, Some("localhost".to_string()));
        assert!(matches!(cmd.source_type, ToolSourceType::Plugin));
        // The canonical registry id must survive resolution intact — not be
        // reconstructed lossily downstream as `plugin:ping`.
        assert_eq!(cmd.tool_id, "plugin:diagnostics:ping");
        // Execution must target the canonical registry id, not a mangled MCP id.
        match cmd.context {
            CommandContext::Builtin { tool_name } => {
                assert_eq!(tool_name, "plugin:diagnostics:ping");
            }
            other => panic!("expected Builtin (direct-tool) context, got {other:?}"),
        }
    }

    /// Face ④ of `extension::visibility`: the catalog row is the one object
    /// every slash surface shares, so the owner recorded on it (`Plugin`, a
    /// plugin-registered `Skill`, or a plugin-declared MCP server) must
    /// survive resolution onto the
    /// `ParsedCommand` the mode serializer reads. A skill nobody owns says so
    /// with `None`, never with an empty string.
    #[tokio::test]
    async fn parse_async_records_the_owning_plugin_for_plugin_owned_entries() {
        use crate::tool_metadata::{ToolSource, UnifiedTool};
        let catalog = Arc::new(ToolCatalog::new());
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
                "plugin:diag:ping",
                "ping",
                ToolSource::Plugin {
                    plugin_id: "diag".into(),
                },
            ),
            (
                "skill:plain",
                "plain",
                ToolSource::Skill {
                    id: "plain".into(),
                    plugin_id: None,
                },
            ),
            (
                "mcp:plugin:proj/srv:srv__t",
                "srv__t",
                ToolSource::Mcp {
                    server: "plugin:proj/srv".into(),
                },
            ),
            (
                "mcp:github:gh__t",
                "gh__t",
                ToolSource::Mcp {
                    server: "github".into(),
                },
            ),
        ] {
            catalog
                .register_with_conflict_resolution(UnifiedTool::new(id, name, "d", source))
                .await;
        }
        let parser = CommandParser::new(Arc::clone(&catalog));
        assert_eq!(
            parser
                .parse_async("/proj:cmd x")
                .await
                .unwrap()
                .owning_plugin
                .as_deref(),
            Some("proj")
        );
        assert_eq!(
            parser
                .parse_async("/ping")
                .await
                .unwrap()
                .owning_plugin
                .as_deref(),
            Some("diag")
        );
        assert_eq!(
            parser.parse_async("/plain").await.unwrap().owning_plugin,
            None
        );
        // A plugin-declared MCP server's tool carries its plugin onto the
        // mode JSON, so the fast path's `slash_owner_admits` judges it.
        assert_eq!(
            parser
                .parse_async("/srv__t")
                .await
                .unwrap()
                .owning_plugin
                .as_deref(),
            Some("proj")
        );
        assert_eq!(
            parser.parse_async("/gh__t").await.unwrap().owning_plugin,
            None
        );
    }
}
