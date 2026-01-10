//! Builtin Command Definitions
//!
//! This module is the SINGLE SOURCE OF TRUTH for builtin command definitions.
//! Both ToolRegistry and Config derive their builtin data from here.
//!
//! This ensures consistency between:
//! - Tool metadata (for UI, command completion, L3 router)
//! - Routing rules (for request processing)

use crate::config::RoutingRuleConfig;

/// Builtin command definition with both tool metadata and routing config
#[derive(Debug, Clone)]
pub struct BuiltinCommandDef {
    // Tool metadata
    pub name: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    pub icon: &'static str,
    pub usage: &'static str,
    pub localization_key: &'static str,
    pub sort_order: i32,
    pub has_subtools: bool,

    // Routing config
    pub routing_regex: &'static str,
    pub routing_system_prompt: &'static str,
    pub routing_capabilities: &'static [&'static str],
    pub routing_intent_type: &'static str,
}

/// All builtin command definitions
///
/// These are the 5 system builtin commands that are always available.
pub const BUILTIN_COMMANDS: &[BuiltinCommandDef] = &[
    // /search - Web search capability
    BuiltinCommandDef {
        name: "search",
        display_name: "Web Search",
        description: "Search the web for real-time information, news, and facts",
        icon: "magnifyingglass",
        usage: "/search <query>",
        localization_key: "tool.search",
        sort_order: 1,
        has_subtools: false,
        routing_regex: r"^/search\s+",
        routing_system_prompt: "You are a helpful search assistant. Answer questions based on the web search results provided below. Be concise and cite sources when possible.",
        routing_capabilities: &["search"],
        routing_intent_type: "builtin_search",
    },
    // /mcp - MCP tools namespace
    BuiltinCommandDef {
        name: "mcp",
        display_name: "MCP Tools",
        description: "Invoke Model Context Protocol tools for extended capabilities",
        icon: "puzzlepiece.extension",
        usage: "/mcp <tool> [params]",
        localization_key: "tool.mcp",
        sort_order: 2,
        has_subtools: true,
        routing_regex: r"^/mcp\s+",
        routing_system_prompt: "You are an MCP integration assistant. Use the available MCP tools to help the user.",
        routing_capabilities: &[],
        routing_intent_type: "builtin_mcp",
    },
    // /skill - Skills namespace
    BuiltinCommandDef {
        name: "skill",
        display_name: "Skills",
        description: "Execute predefined skill workflows",
        icon: "wand.and.stars",
        usage: "/skill <name>",
        localization_key: "tool.skill",
        sort_order: 3,
        has_subtools: true,
        routing_regex: r"^/skill\s+",
        routing_system_prompt: "You are a helpful AI assistant. Follow the skill instructions provided in the context to complete the task.",
        routing_capabilities: &["skills", "memory"],
        routing_intent_type: "skills",
    },
    // /video - Video transcript capability
    BuiltinCommandDef {
        name: "video",
        display_name: "Video Transcript",
        description: "Analyze YouTube video content via transcript extraction",
        icon: "play.rectangle",
        usage: "/video <YouTube URL>",
        localization_key: "tool.video",
        sort_order: 4,
        has_subtools: false,
        routing_regex: r"^/video\s+",
        routing_system_prompt: "You are a video content analyst. A video transcript will be provided in the context section below if available. Analyze the transcript and provide insights, summaries, or answer questions about the video content. If no transcript is provided, explain that the video may not have captions enabled or transcript extraction failed.",
        routing_capabilities: &["video", "memory"],
        routing_intent_type: "video_analysis",
    },
    // /chat - Multi-turn conversation
    BuiltinCommandDef {
        name: "chat",
        display_name: "Chat",
        description: "Start a multi-turn conversation session",
        icon: "bubble.left.and.bubble.right",
        usage: "/chat <message>",
        localization_key: "tool.chat",
        sort_order: 5,
        has_subtools: false,
        routing_regex: r"^/chat\s+",
        routing_system_prompt: "You are a helpful AI assistant. Engage in natural conversation and provide helpful responses.",
        routing_capabilities: &["memory"],
        routing_intent_type: "general_chat",
    },
];

/// Get builtin routing rules (for Config module)
///
/// This function generates RoutingRuleConfig from the builtin definitions.
/// Call this instead of maintaining separate hardcoded rules.
pub fn get_builtin_routing_rules() -> Vec<RoutingRuleConfig> {
    BUILTIN_COMMANDS
        .iter()
        .map(|def| RoutingRuleConfig {
            rule_type: Some("command".to_string()),
            is_builtin: true,
            regex: def.routing_regex.to_string(),
            provider: Some("openai".to_string()), // Will be overridden by default_provider
            system_prompt: Some(def.routing_system_prompt.to_string()),
            strip_prefix: Some(true),
            capabilities: if def.routing_capabilities.is_empty() {
                None
            } else {
                Some(def.routing_capabilities.iter().map(|s| s.to_string()).collect())
            },
            intent_type: Some(def.routing_intent_type.to_string()),
            context_format: Some("markdown".to_string()),
            skill_id: None,
            skill_version: None,
            workflow: None,
            tools: None,
            knowledge_base: None,
            icon: Some(def.icon.to_string()),
            hint: Some(def.usage.to_string()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_commands_count() {
        assert_eq!(BUILTIN_COMMANDS.len(), 5);
    }

    #[test]
    fn test_builtin_routing_rules() {
        let rules = get_builtin_routing_rules();
        assert_eq!(rules.len(), 5);

        // Verify search rule
        let search = rules.iter().find(|r| r.regex.contains("/search")).unwrap();
        assert!(search.is_builtin);
        assert_eq!(search.capabilities, Some(vec!["search".to_string()]));
    }

    #[test]
    fn test_builtin_command_names() {
        let names: Vec<_> = BUILTIN_COMMANDS.iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["search", "mcp", "skill", "video", "chat"]);
    }
}
