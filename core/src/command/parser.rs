//! Unified Slash Command Parser
//!
//! This module provides a unified parser for all slash commands in Aether:
//! - Builtin commands (search, youtube, webfetch, agent)
//! - MCP tool commands
//! - Skill commands
//! - Custom commands (from routing rules)
//!
//! The parser extracts the command name and arguments from user input,
//! then looks up the command in all registries to determine its type
//! and associated context.

use crate::command::CommandRegistry;
use crate::config::RoutingRuleConfig;
use crate::dispatcher::ToolSourceType;
use crate::skills::{Skill, SkillsRegistry};
use std::sync::Arc;
use tracing::debug;

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
        /// Tool name for agent mode (e.g., "search", "youtube")
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
    },

    /// Custom command context
    Custom {
        /// System prompt to inject
        system_prompt: Option<String>,
        /// Provider override (if specified in rule)
        provider: Option<String>,
        /// Rule regex pattern
        pattern: String,
    },

    /// No specific context (fallback)
    None,
}

/// Unified command parser
pub struct CommandParser {
    /// Command registry (from config rules)
    command_registry: Option<Arc<CommandRegistry>>,

    /// Skills registry
    skills_registry: Option<Arc<SkillsRegistry>>,

    /// Routing rules (for custom commands)
    routing_rules: Vec<RoutingRuleConfig>,

    /// MCP server names (for MCP command lookup)
    mcp_server_names: Vec<String>,

    /// Builtin command names
    builtin_commands: Vec<&'static str>,
}

impl CommandParser {
    /// Create a new command parser
    pub fn new() -> Self {
        Self {
            command_registry: None,
            skills_registry: None,
            routing_rules: Vec::new(),
            mcp_server_names: Vec::new(),
            builtin_commands: vec!["agent", "search", "youtube", "webfetch"],
        }
    }

    /// Set the command registry
    pub fn with_command_registry(mut self, registry: Arc<CommandRegistry>) -> Self {
        self.command_registry = Some(registry);
        self
    }

    /// Set the skills registry
    pub fn with_skills_registry(mut self, registry: Arc<SkillsRegistry>) -> Self {
        self.skills_registry = Some(registry);
        self
    }

    /// Set routing rules
    pub fn with_routing_rules(mut self, rules: Vec<RoutingRuleConfig>) -> Self {
        self.routing_rules = rules;
        self
    }

    /// Set MCP server names
    pub fn with_mcp_servers(mut self, names: Vec<String>) -> Self {
        self.mcp_server_names = names;
        self
    }

    /// Parse user input as a command
    ///
    /// Returns `Some(ParsedCommand)` if the input is a valid slash command
    /// (starting with /).
    pub fn parse(&self, input: &str) -> Option<ParsedCommand> {
        let trimmed = input.trim();

        // Only handle slash commands
        if trimmed.starts_with('/') {
            return self.parse_slash_command(trimmed);
        }

        None
    }

    /// Parse slash command (input starting with /)
    fn parse_slash_command(&self, input: &str) -> Option<ParsedCommand> {
        // Extract command name and arguments
        let without_slash = &input[1..];
        let (command_name, arguments) = self.extract_parts(without_slash);

        if command_name.is_empty() {
            return None;
        }

        debug!(
            command = %command_name,
            args = ?arguments,
            "Parsing slash command"
        );

        // Try to find the command in order of priority:
        // 1. Builtin commands (highest priority)
        // 2. Skills
        // 3. MCP tools
        // 4. Custom commands (from routing rules)

        // 1. Check builtin commands
        if self.builtin_commands.contains(&command_name.as_str()) {
            return Some(ParsedCommand {
                source_type: ToolSourceType::Builtin,
                command_name: command_name.clone(),
                arguments,
                full_input: input.to_string(),
                context: CommandContext::Builtin {
                    tool_name: command_name,
                },
            });
        }

        // 2. Check skills registry
        if let Some(ref skills_registry) = self.skills_registry {
            if let Some(skill) = skills_registry.get_skill(&command_name) {
                return Some(self.create_skill_command(&command_name, arguments, input, &skill));
            }
        }

        // 3. Check MCP servers
        if self.mcp_server_names.contains(&command_name) {
            return Some(ParsedCommand {
                source_type: ToolSourceType::Mcp,
                command_name: command_name.clone(),
                arguments,
                full_input: input.to_string(),
                context: CommandContext::Mcp {
                    server_name: command_name,
                    tool_name: None,
                },
            });
        }

        // 4. Check custom commands (routing rules with ^/ prefix)
        if let Some(rule) = self.find_matching_rule(&command_name) {
            return Some(ParsedCommand {
                source_type: ToolSourceType::Custom,
                command_name: command_name.clone(),
                arguments,
                full_input: input.to_string(),
                context: CommandContext::Custom {
                    system_prompt: rule.system_prompt.clone(),
                    provider: rule.provider.clone(),
                    pattern: rule.regex.clone(),
                },
            });
        }

        // Command not found in any registry
        debug!(command = %command_name, "Command not found in any registry");
        None
    }

    /// Extract command name and arguments from input (without leading /)
    fn extract_parts(&self, input: &str) -> (String, Option<String>) {
        let parts: Vec<&str> = input.splitn(2, char::is_whitespace).collect();

        let command_name = parts.first().map(|s| s.to_lowercase()).unwrap_or_default();

        let arguments = parts
            .get(1)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        (command_name, arguments)
    }

    /// Create a skill command with context
    fn create_skill_command(
        &self,
        command_name: &str,
        arguments: Option<String>,
        full_input: &str,
        skill: &Skill,
    ) -> ParsedCommand {
        ParsedCommand {
            source_type: ToolSourceType::Skill,
            command_name: command_name.to_string(),
            arguments,
            full_input: full_input.to_string(),
            context: CommandContext::Skill {
                skill_id: skill.id.clone(),
                instructions: skill.instructions.clone(),
                display_name: skill.frontmatter.name.clone(),
            },
        }
    }

    /// Find a matching routing rule for the command
    fn find_matching_rule(&self, command_name: &str) -> Option<&RoutingRuleConfig> {
        for rule in &self.routing_rules {
            // Check if this is a command rule (starts with ^/)
            if !rule.regex.starts_with("^/") {
                continue;
            }

            // Extract the command key from the regex
            let rule_command = rule
                .regex
                .trim_start_matches("^/")
                .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
                .next()
                .unwrap_or("");

            if rule_command.eq_ignore_ascii_case(command_name) {
                return Some(rule);
            }
        }
        None
    }
}

impl Default for CommandParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_skills_registry() -> Arc<SkillsRegistry> {
        let temp_dir = TempDir::new().unwrap();
        let skills_dir = temp_dir.path().to_path_buf();

        // Create a test skill
        let skill_dir = skills_dir.join("knowledge-graph");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            r#"---
name: Knowledge Graph
description: Create knowledge graphs from documents
---

# Knowledge Graph Skill

When creating a knowledge graph, follow these steps:
1. Analyze the document structure
2. Extract key concepts
3. Generate relationships
"#,
        )
        .unwrap();

        let registry = SkillsRegistry::new(skills_dir);
        registry.load_all().unwrap();
        Arc::new(registry)
    }

    #[test]
    fn test_parse_builtin_command() {
        let parser = CommandParser::new();

        // Test /agent command
        let result = parser.parse("/agent organize my files");
        assert!(result.is_some());
        let cmd = result.unwrap();
        assert_eq!(cmd.source_type, ToolSourceType::Builtin);
        assert_eq!(cmd.command_name, "agent");
        assert_eq!(cmd.arguments, Some("organize my files".to_string()));

        // Test /search command
        let result = parser.parse("/search weather today");
        assert!(result.is_some());
        let cmd = result.unwrap();
        assert_eq!(cmd.command_name, "search");
        assert_eq!(cmd.arguments, Some("weather today".to_string()));
    }

    #[test]
    fn test_parse_skill_command() {
        let skills_registry = create_test_skills_registry();
        let parser = CommandParser::new().with_skills_registry(skills_registry);

        let result = parser.parse("/knowledge-graph analyze this document");
        assert!(result.is_some());
        let cmd = result.unwrap();
        assert_eq!(cmd.source_type, ToolSourceType::Skill);
        assert_eq!(cmd.command_name, "knowledge-graph");
        assert_eq!(cmd.arguments, Some("analyze this document".to_string()));

        if let CommandContext::Skill {
            skill_id,
            instructions,
            display_name,
        } = cmd.context
        {
            assert_eq!(skill_id, "knowledge-graph");
            assert_eq!(display_name, "Knowledge Graph");
            assert!(instructions.contains("knowledge graph"));
        } else {
            panic!("Expected Skill context");
        }
    }

    #[test]
    fn test_parse_custom_command() {
        let rules = vec![RoutingRuleConfig {
            regex: "^/translate".to_string(),
            provider: Some("openai".to_string()),
            system_prompt: Some("You are a translator.".to_string()),
            ..Default::default()
        }];

        let parser = CommandParser::new().with_routing_rules(rules);

        let result = parser.parse("/translate hello to French");
        assert!(result.is_some());
        let cmd = result.unwrap();
        assert_eq!(cmd.source_type, ToolSourceType::Custom);
        assert_eq!(cmd.command_name, "translate");

        if let CommandContext::Custom {
            system_prompt,
            provider,
            ..
        } = cmd.context
        {
            assert_eq!(system_prompt, Some("You are a translator.".to_string()));
            assert_eq!(provider, Some("openai".to_string()));
        } else {
            panic!("Expected Custom context");
        }
    }

    #[test]
    fn test_parse_mcp_command() {
        let parser =
            CommandParser::new().with_mcp_servers(vec!["git".to_string(), "docker".to_string()]);

        let result = parser.parse("/git status");
        assert!(result.is_some());
        let cmd = result.unwrap();
        assert_eq!(cmd.source_type, ToolSourceType::Mcp);
        assert_eq!(cmd.command_name, "git");
        assert_eq!(cmd.arguments, Some("status".to_string()));
    }

    #[test]
    fn test_parse_non_command() {
        let parser = CommandParser::new();

        // Not starting with /
        assert!(parser.parse("hello world").is_none());
        assert!(parser.parse("  regular text").is_none());

        // Just /
        assert!(parser.parse("/").is_none());
        assert!(parser.parse("/ ").is_none());
    }

    #[test]
    fn test_parse_unknown_command() {
        let parser = CommandParser::new();

        // Unknown command (not in any registry)
        let result = parser.parse("/unknown-command args");
        assert!(result.is_none());
    }

    #[test]
    fn test_command_priority() {
        // Builtin should take priority over skill with same name
        let skills_registry = create_test_skills_registry();

        // Create a skill named "search" (same as builtin)
        // But builtin should still win

        let parser = CommandParser::new().with_skills_registry(skills_registry);

        let result = parser.parse("/search query");
        assert!(result.is_some());
        let cmd = result.unwrap();
        // Builtin should win
        assert_eq!(cmd.source_type, ToolSourceType::Builtin);
    }

    #[test]
    fn test_case_insensitive_command() {
        let parser = CommandParser::new();

        let result = parser.parse("/AGENT do something");
        assert!(result.is_some());
        let cmd = result.unwrap();
        assert_eq!(cmd.command_name, "agent");
    }

    #[test]
    fn test_whitespace_handling() {
        let parser = CommandParser::new();

        // Leading/trailing whitespace
        let result = parser.parse("  /agent task  ");
        assert!(result.is_some());
        let cmd = result.unwrap();
        assert_eq!(cmd.command_name, "agent");
        assert_eq!(cmd.arguments, Some("task".to_string()));

        // No arguments
        let result = parser.parse("/search");
        assert!(result.is_some());
        let cmd = result.unwrap();
        assert_eq!(cmd.command_name, "search");
        assert!(cmd.arguments.is_none());
    }
}
