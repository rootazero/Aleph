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
    /// Command name (without leading /)
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
}

/// Command-specific context based on source type
#[derive(Debug, Clone)]
pub enum CommandContext {
    /// Builtin command context
    Builtin {
        /// Tool name for agent mode
        tool_name: String,
    },
    /// MCP tool context
    Mcp {
        /// Server name
        server_name: String,
        /// Tool name within the server
        tool_name: Option<String>,
    },
    /// Skill context
    Skill {
        /// Skill ID
        skill_id: String,
        /// Skill instructions to inject
        instructions: String,
        /// Skill name for display
        display_name: String,
        /// Allowed tools for this skill
        allowed_tools: Vec<String>,
    },
    /// Custom command context
    Custom {
        /// System prompt to inject
        system_prompt: Option<String>,
        /// Provider override
        provider: Option<String>,
        /// Rule regex pattern
        pattern: String,
    },
    /// No specific context (fallback)
    None,
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

        let source_type = ToolSourceType::from(&resolved.tool.source);
        let context = tool_to_command_context(&resolved.tool);

        Some(ParsedCommand {
            source_type,
            command_name: resolved.tool.name.clone(),
            tool_id: resolved.tool.id.clone(),
            arguments: resolved.arguments,
            context,
        })
    }

    /// Get a reference to the underlying `ToolCatalog`
    #[must_use]
    pub const fn tool_registry(&self) -> &Arc<ToolCatalog> {
        &self.tool_registry
    }
}

/// Derive `CommandContext` from `UnifiedTool` fields
fn tool_to_command_context(tool: &UnifiedTool) -> CommandContext {
    match &tool.source {
        ToolSource::Builtin | ToolSource::Native => CommandContext::Builtin {
            tool_name: tool.name.clone(),
        },
        ToolSource::Mcp { server } => CommandContext::Mcp {
            server_name: server.clone(),
            tool_name: Some(tool.name.clone()),
        },
        ToolSource::Skill { id } => CommandContext::Skill {
            skill_id: id.clone(),
            instructions: tool.routing_system_prompt.clone().unwrap_or_default(),
            display_name: tool.display_name.clone(),
            allowed_tools: tool.routing_capabilities.clone(),
        },
        ToolSource::Custom { .. } => CommandContext::Custom {
            system_prompt: tool.routing_system_prompt.clone(),
            provider: None, // Provider is resolved at routing time
            pattern: tool
                .routing_regex
                .as_ref()
                .unwrap_or(&tool.name)
                .clone(),
        },
        ToolSource::Plugin { .. } => CommandContext::Builtin {
            // Plugin tools live in the tool registry under their namespaced id
            // (`plugin:<plugin_id>:<name>`) and are invoked through the
            // direct-tool fast path. Routing them as `Mcp` mangled the id into
            // `mcp__plugin:<id>_<name>`, which never matched a registered tool,
            // so every plugin slash command failed with a hard execution error.
            tool_name: tool.id.clone(),
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
            other => panic!("expected Builtin (direct-tool) context, got {:?}", other),
        }
    }
}
