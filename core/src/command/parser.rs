//! Unified Slash Command Parser
//!
//! Delegates all command resolution to ToolRegistry.

use crate::dispatcher::{ToolRegistry, ToolSource, ToolSourceType, UnifiedTool};
use crate::sync_primitives::Arc;

/// Parsed command result
#[derive(Debug, Clone)]
pub struct ParsedCommand {
    /// Command source type
    pub source_type: ToolSourceType,
    /// Command name (without leading /)
    pub command_name: String,
    /// Arguments after the command name
    pub arguments: Option<String>,
    /// Full original input
    pub full_input: String,
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

/// Unified command parser — delegates to ToolRegistry
pub struct CommandParser {
    /// Tool registry for command resolution
    tool_registry: Arc<ToolRegistry>,
}

impl CommandParser {
    /// Create a new command parser backed by ToolRegistry
    pub fn new(tool_registry: Arc<ToolRegistry>) -> Self {
        Self { tool_registry }
    }

    /// Parse user input as a slash command (async)
    ///
    /// Returns `Some(ParsedCommand)` if the input matches a registered command.
    pub async fn parse_async(&self, input: &str) -> Option<ParsedCommand> {
        let trimmed = input.trim();
        if !trimmed.starts_with('/') {
            return None;
        }

        let resolved = self.tool_registry.resolve_command(trimmed).await?;

        let source_type = tool_source_to_source_type(&resolved.tool.source);
        let context = tool_to_command_context(&resolved.tool);

        Some(ParsedCommand {
            source_type,
            command_name: resolved.tool.name.clone(),
            arguments: resolved.arguments,
            full_input: resolved.raw_input,
            context,
        })
    }

    /// Synchronous parse (for backward compatibility with ExecutionDecider)
    ///
    /// Uses `tokio::task::block_in_place` — only safe when called from
    /// within an async context on a multi-threaded runtime.
    pub fn parse(&self, input: &str) -> Option<ParsedCommand> {
        let trimmed = input.trim();
        if !trimmed.starts_with('/') {
            return None;
        }

        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.parse_async(trimmed))
        })
    }

    /// Get a reference to the underlying ToolRegistry
    pub fn tool_registry(&self) -> &Arc<ToolRegistry> {
        &self.tool_registry
    }
}

/// Convert ToolSource to ToolSourceType
fn tool_source_to_source_type(source: &ToolSource) -> ToolSourceType {
    match source {
        ToolSource::Builtin => ToolSourceType::Builtin,
        ToolSource::Native => ToolSourceType::Native,
        ToolSource::Mcp { .. } => ToolSourceType::Mcp,
        ToolSource::Skill { .. } => ToolSourceType::Skill,
        ToolSource::Custom { .. } => ToolSourceType::Custom,
    }
}

/// Derive CommandContext from UnifiedTool fields
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
            pattern: tool.routing_regex.clone().unwrap_or_default(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RoutingRuleConfig;

    fn create_test_registry() -> Arc<ToolRegistry> {
        Arc::new(ToolRegistry::new())
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

    #[tokio::test]
    async fn test_parse_sync_compatibility() {
        let registry = create_test_registry();
        let rules = vec![RoutingRuleConfig {
            regex: "^/help".to_string(),
            provider: None,
            system_prompt: Some("Help".to_string()),
            ..Default::default()
        }];
        registry.register_custom_commands(&rules).await;

        let parser = CommandParser::new(registry);
        let result = parser.parse("/help");
        assert!(result.is_some());
    }
}
