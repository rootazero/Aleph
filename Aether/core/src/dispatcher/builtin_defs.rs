//! Builtin Command Definitions (Flat Namespace Mode)
//!
//! This module is the SINGLE SOURCE OF TRUTH for builtin command definitions.
//! Both ToolRegistry and Config derive their builtin data from here.
//!
//! This ensures consistency between:
//! - Tool metadata (for UI, command completion, L3 router)
//! - Routing rules (for request processing)
//!
//! ## Flat Namespace Architecture
//!
//! In flat namespace mode, MCP tools and Skills are registered as root-level
//! commands directly (e.g., `/git`, `/refine-text`), not under `/mcp` or `/skill`
//! namespaces. This eliminates "implementation detail leakage" where users
//! had to remember which namespace contains which tool.
//!
//! Only 3 system builtin commands remain:
//! - `/search` - Web search capability
//! - `/youtube` - YouTube video transcript analysis
//! - `/chat` - Multi-turn conversation
//!
//! MCP tools and Skills are now registered dynamically via ToolRegistry with
//! automatic conflict resolution based on priority.

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

/// All builtin command definitions (Flat Namespace Mode)
///
/// These are the 3 system builtin commands that are always available.
/// In flat namespace mode:
/// - `/mcp` namespace removed - MCP tools registered directly as root commands
/// - `/skill` namespace removed - Skills registered directly as root commands
///
/// Users now invoke tools by name without namespace prefixes:
/// - Before: `/mcp git status` → After: `/git status`
/// - Before: `/skill refine-text` → After: `/refine-text`
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
    // /youtube - YouTube video transcript capability
    BuiltinCommandDef {
        name: "youtube",
        display_name: "YouTube",
        description: "Analyze YouTube video content via transcript extraction",
        icon: "play.rectangle.fill",
        usage: "/youtube <YouTube URL>",
        localization_key: "tool.youtube",
        sort_order: 2,
        has_subtools: false,
        routing_regex: r"^/youtube\s+",
        routing_system_prompt: "You are a YouTube video content analyst. A video transcript will be provided in the context section below if available. Analyze the transcript and provide insights, summaries, or answer questions about the video content. If no transcript is provided, explain that the video may not have captions enabled or transcript extraction failed.",
        routing_capabilities: &["video", "memory"],
        routing_intent_type: "youtube_analysis",
    },
    // /chat - Multi-turn conversation
    BuiltinCommandDef {
        name: "chat",
        display_name: "Chat",
        description: "Start a multi-turn conversation session",
        icon: "bubble.left.and.bubble.right",
        usage: "/chat <message>",
        localization_key: "tool.chat",
        sort_order: 3,
        has_subtools: false,
        routing_regex: r"^/chat\s+",
        routing_system_prompt: "You are a helpful AI assistant. Engage in natural conversation and provide helpful responses.",
        routing_capabilities: &["memory"],
        routing_intent_type: "general_chat",
    },
    // /fetch - Web page content fetching
    BuiltinCommandDef {
        name: "fetch",
        display_name: "Fetch URL",
        description: "Fetch and read web page content from a URL",
        icon: "globe",
        usage: "/fetch <URL>",
        localization_key: "tool.fetch",
        sort_order: 4,
        has_subtools: false,
        routing_regex: r"^/fetch\s+",
        routing_system_prompt: "You are a helpful assistant. A web page content has been fetched and will be provided below. Analyze, summarize, or answer questions about the content.",
        routing_capabilities: &["web_fetch"],
        routing_intent_type: "web_fetch",
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
        // Flat namespace mode: only 4 builtin commands
        // /mcp and /skill namespaces removed
        assert_eq!(BUILTIN_COMMANDS.len(), 4);
    }

    #[test]
    fn test_builtin_routing_rules() {
        let rules = get_builtin_routing_rules();
        assert_eq!(rules.len(), 4);

        // Verify search rule
        let search = rules.iter().find(|r| r.regex.contains("/search")).unwrap();
        assert!(search.is_builtin);
        assert_eq!(search.capabilities, Some(vec!["search".to_string()]));

        // Verify youtube rule
        let youtube = rules.iter().find(|r| r.regex.contains("/youtube")).unwrap();
        assert!(youtube.is_builtin);
        assert_eq!(
            youtube.capabilities,
            Some(vec!["video".to_string(), "memory".to_string()])
        );

        // Verify chat rule
        let chat = rules.iter().find(|r| r.regex.contains("/chat")).unwrap();
        assert!(chat.is_builtin);
        assert_eq!(chat.capabilities, Some(vec!["memory".to_string()]));
    }

    #[test]
    fn test_builtin_command_names() {
        let names: Vec<_> = BUILTIN_COMMANDS.iter().map(|c| c.name).collect();
        // Flat namespace: no /mcp or /skill
        assert_eq!(names, vec!["search", "youtube", "chat", "fetch"]);
    }

    #[test]
    fn test_no_namespace_commands() {
        // Verify /mcp and /skill are NOT in builtin commands
        let names: Vec<_> = BUILTIN_COMMANDS.iter().map(|c| c.name).collect();
        assert!(!names.contains(&"mcp"), "/mcp should not be a builtin in flat namespace mode");
        assert!(!names.contains(&"skill"), "/skill should not be a builtin in flat namespace mode");
    }

    #[test]
    fn test_no_subtools_in_flat_namespace() {
        // In flat namespace mode, no builtin should have subtools
        for cmd in BUILTIN_COMMANDS {
            assert!(
                !cmd.has_subtools,
                "Builtin '{}' should not have subtools in flat namespace mode",
                cmd.name
            );
        }
    }
}
