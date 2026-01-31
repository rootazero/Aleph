//! Built-in UI hints definitions for Aether configuration.
//!
//! This module contains the default UI hints for all standard Aether configuration
//! fields. It uses the `define_groups!` and `define_hints!` macros for a declarative
//! definition style.

use super::{ConfigUiHints, FieldHint, GroupMeta};
use crate::{define_groups, define_hints};

/// Build the complete UI hints for Aether configuration.
///
/// Returns a `ConfigUiHints` instance containing all default groups and field hints.
///
/// # Example
///
/// ```ignore
/// use aethecore::config::ui_hints::build_ui_hints;
///
/// let hints = build_ui_hints();
///
/// // Get hint for a specific field
/// if let Some(hint) = hints.get_hint("general.language") {
///     println!("Label: {:?}", hint.label);
/// }
///
/// // Get hint using wildcard matching
/// if let Some(hint) = hints.get_hint("providers.openai.api_key") {
///     assert!(hint.sensitive);
/// }
/// ```
pub fn build_ui_hints() -> ConfigUiHints {
    ConfigUiHints {
        groups: build_groups(),
        fields: build_field_hints(),
    }
}

fn build_groups() -> std::collections::HashMap<String, GroupMeta> {
    define_groups! {
        "general" => { label: "General", order: 10, icon: "gear" },
        "providers" => { label: "AI Providers", order: 20, icon: "cloud" },
        "agents" => { label: "Agents", order: 30, icon: "robot" },
        "channels" => { label: "Channels", order: 40, icon: "chat" },
        "tools" => { label: "Tools", order: 50, icon: "wrench" },
        "memory" => { label: "Memory", order: 60, icon: "brain" },
        "search" => { label: "Search", order: 70, icon: "search" },
        "shortcuts" => { label: "Shortcuts", order: 80, icon: "keyboard" },
        "behavior" => { label: "Behavior", order: 90, icon: "sliders" },
        "advanced" => { label: "Advanced", order: 100, icon: "cog" },
    }
}

fn build_field_hints() -> std::collections::HashMap<String, FieldHint> {
    define_hints! {
        // === General ===
        "general.default_provider" => {
            label: "Default Provider",
            help: "AI provider used when no routing rule matches",
            group: "general",
            order: 1,
        },
        "general.language" => {
            label: "Language",
            help: "UI display language (en, zh-Hans)",
            group: "general",
            order: 2,
        },
        "general.output_dir" => {
            label: "Output Directory",
            help: "Directory for generated files",
            group: "general",
            order: 3,
            placeholder: "~/.aether/output",
        },

        // === Providers (wildcard) ===
        "providers.*.api_key" => {
            label: "API Key",
            help: "API key for authentication",
            group: "providers",
            sensitive: true,
        },
        "providers.*.model" => {
            label: "Model",
            help: "Model identifier (e.g., gpt-4o, claude-opus-4-5)",
            group: "providers",
        },
        "providers.*.base_url" => {
            label: "Base URL",
            help: "Custom API endpoint URL",
            group: "providers",
            advanced: true,
        },
        "providers.*.timeout_seconds" => {
            label: "Timeout",
            help: "Request timeout in seconds (1-300)",
            group: "providers",
        },
        "providers.*.enabled" => {
            label: "Enabled",
            help: "Whether this provider is active",
            group: "providers",
        },
        "providers.*.temperature" => {
            label: "Temperature",
            help: "Sampling temperature (0.0-2.0)",
            group: "providers",
            advanced: true,
        },
        "providers.*.max_tokens" => {
            label: "Max Tokens",
            help: "Maximum tokens in response",
            group: "providers",
            advanced: true,
        },

        // === Provider-specific overrides ===
        "providers.openai.model" => {
            label: "OpenAI Model",
            help: "OpenAI model identifier (e.g., gpt-4o, gpt-4-turbo)",
            group: "providers",
            placeholder: "gpt-4o",
        },
        "providers.anthropic.model" => {
            label: "Anthropic Model",
            help: "Anthropic model identifier (e.g., claude-opus-4-5, claude-sonnet-4)",
            group: "providers",
            placeholder: "claude-opus-4-5",
        },
        "providers.gemini.model" => {
            label: "Gemini Model",
            help: "Google Gemini model identifier",
            group: "providers",
            placeholder: "gemini-2.0-flash",
        },

        // === Memory ===
        "memory.enabled" => {
            label: "Enable Memory",
            help: "Enable semantic memory for context retrieval",
            group: "memory",
            order: 1,
        },
        "memory.max_context_items" => {
            label: "Max Context Items",
            help: "Maximum number of memory items to include",
            group: "memory",
            order: 2,
        },
        "memory.similarity_threshold" => {
            label: "Similarity Threshold",
            help: "Minimum similarity score for memory retrieval (0.0-1.0)",
            group: "memory",
            order: 3,
        },
        "memory.embedding_model" => {
            label: "Embedding Model",
            help: "Model used for generating embeddings",
            group: "memory",
            order: 4,
            advanced: true,
        },
        "memory.chunk_size" => {
            label: "Chunk Size",
            help: "Text chunk size for embedding",
            group: "memory",
            advanced: true,
        },

        // === Shortcuts ===
        "shortcuts.summon" => {
            label: "Summon Shortcut",
            help: "Keyboard shortcut to summon Aether",
            group: "shortcuts",
            placeholder: "Command+Grave",
        },
        "shortcuts.cancel" => {
            label: "Cancel Shortcut",
            help: "Keyboard shortcut to cancel current operation",
            group: "shortcuts",
            placeholder: "Escape",
        },
        "shortcuts.new_conversation" => {
            label: "New Conversation",
            help: "Keyboard shortcut to start a new conversation",
            group: "shortcuts",
            placeholder: "Command+N",
        },

        // === Behavior ===
        "behavior.output_mode" => {
            label: "Output Mode",
            help: "How to display AI responses (typewriter, instant)",
            group: "behavior",
        },
        "behavior.typing_speed" => {
            label: "Typing Speed",
            help: "Characters per second for typewriter mode (50-400)",
            group: "behavior",
        },
        "behavior.auto_scroll" => {
            label: "Auto Scroll",
            help: "Automatically scroll to new content",
            group: "behavior",
        },
        "behavior.confirm_dangerous_actions" => {
            label: "Confirm Dangerous Actions",
            help: "Require confirmation for file modifications and shell commands",
            group: "behavior",
        },

        // === Search ===
        "search.enabled" => {
            label: "Enable Search",
            help: "Enable web search capabilities",
            group: "search",
            order: 1,
        },
        "search.default_provider" => {
            label: "Search Provider",
            help: "Default search provider",
            group: "search",
            order: 2,
        },
        "search.max_results" => {
            label: "Max Results",
            help: "Maximum number of search results to return",
            group: "search",
            order: 3,
        },

        // === Tools ===
        "tools.fs_enabled" => {
            label: "File System Access",
            help: "Enable file system tools",
            group: "tools",
        },
        "tools.git_enabled" => {
            label: "Git Access",
            help: "Enable Git tools",
            group: "tools",
        },
        "tools.shell_enabled" => {
            label: "Shell Access",
            help: "Enable shell command execution",
            group: "tools",
        },
        "tools.browser_enabled" => {
            label: "Browser Control",
            help: "Enable browser automation via CDP",
            group: "tools",
        },
        "tools.allowed_paths" => {
            label: "Allowed Paths",
            help: "Paths the agent is allowed to access",
            group: "tools",
            advanced: true,
        },
        "tools.blocked_paths" => {
            label: "Blocked Paths",
            help: "Paths the agent is not allowed to access",
            group: "tools",
            advanced: true,
        },

        // === MCP ===
        "mcp.enabled" => {
            label: "Enable MCP",
            help: "Enable Model Context Protocol servers",
            group: "tools",
            advanced: true,
        },
        "mcp.servers.*.command" => {
            label: "Server Command",
            help: "Command to start the MCP server",
            group: "tools",
            advanced: true,
        },
        "mcp.servers.*.args" => {
            label: "Server Arguments",
            help: "Arguments passed to the server command",
            group: "tools",
            advanced: true,
        },
        "mcp.servers.*.env" => {
            label: "Server Environment",
            help: "Environment variables for the server",
            group: "tools",
            advanced: true,
        },

        // === Agent ===
        "agent.require_confirmation" => {
            label: "Require Confirmation",
            help: "Require user confirmation for actions",
            group: "agents",
        },
        "agent.default_thinking" => {
            label: "Default Thinking Level",
            help: "Default thinking level for agent responses (off, minimal, low, medium, high, xhigh)",
            group: "agents",
        },
        "agent.max_iterations" => {
            label: "Max Iterations",
            help: "Maximum number of tool-use iterations per request",
            group: "agents",
            advanced: true,
        },
        "agent.identity" => {
            label: "Agent Identity",
            help: "System prompt defining the agent's persona",
            group: "agents",
            advanced: true,
        },

        // === Channels ===
        "channels.*.enabled" => {
            label: "Enabled",
            help: "Whether this channel is active",
            group: "channels",
        },
        "channels.telegram.token" => {
            label: "Telegram Bot Token",
            help: "Bot token from @BotFather",
            group: "channels",
            sensitive: true,
        },
        "channels.telegram.allowed_users" => {
            label: "Allowed Users",
            help: "List of allowed Telegram user IDs",
            group: "channels",
        },
        "channels.discord.token" => {
            label: "Discord Bot Token",
            help: "Bot token from Discord Developer Portal",
            group: "channels",
            sensitive: true,
        },
        "channels.discord.allowed_guilds" => {
            label: "Allowed Guilds",
            help: "List of allowed Discord server IDs",
            group: "channels",
        },
        "channels.webchat.enabled" => {
            label: "Enable WebChat",
            help: "Enable the built-in web chat interface",
            group: "channels",
        },
        "channels.webchat.port" => {
            label: "WebChat Port",
            help: "Port for the web chat server",
            group: "channels",
        },

        // === Rules ===
        "rules.*.regex" => {
            label: "Pattern",
            help: "Regex pattern to match",
            group: "advanced",
        },
        "rules.*.provider" => {
            label: "Provider",
            help: "Provider to use when pattern matches",
            group: "advanced",
        },
        "rules.*.priority" => {
            label: "Priority",
            help: "Rule priority (lower = higher priority)",
            group: "advanced",
        },

        // === Gateway ===
        "gateway.port" => {
            label: "Gateway Port",
            help: "Port for the WebSocket gateway",
            group: "advanced",
            placeholder: "18789",
        },
        "gateway.bind" => {
            label: "Bind Address",
            help: "Address to bind the gateway (loopback, all, or specific IP)",
            group: "advanced",
        },
        "gateway.require_auth" => {
            label: "Require Authentication",
            help: "Require authentication for gateway connections",
            group: "advanced",
        },

        // === Session ===
        "session.dm_scope" => {
            label: "DM Scope",
            help: "Session isolation strategy for direct messages",
            group: "advanced",
        },
        "session.auto_reset_hour" => {
            label: "Auto Reset Hour",
            help: "Hour of day (0-23) to auto-reset sessions",
            group: "advanced",
        },
        "session.expiry_days" => {
            label: "Session Expiry",
            help: "Days until sessions expire",
            group: "advanced",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_ui_hints() {
        let hints = build_ui_hints();

        // Check groups
        assert!(hints.groups.contains_key("general"));
        assert!(hints.groups.contains_key("providers"));
        assert!(hints.groups.contains_key("advanced"));

        // Check field hints
        assert!(hints.fields.contains_key("general.language"));
        assert!(hints.fields.contains_key("providers.*.api_key"));

        // Check sensitive field
        let api_key_hint = hints.fields.get("providers.*.api_key").unwrap();
        assert!(api_key_hint.sensitive);
    }

    #[test]
    fn test_wildcard_provider_hints() {
        let hints = build_ui_hints();

        // Test wildcard matching for providers
        let hint = hints.get_hint("providers.openai.api_key");
        assert!(hint.is_some());
        assert!(hint.unwrap().sensitive);

        let hint2 = hints.get_hint("providers.claude.model");
        assert!(hint2.is_some());
        assert_eq!(hint2.unwrap().group, Some("providers".to_string()));
    }

    #[test]
    fn test_provider_specific_override() {
        let hints = build_ui_hints();

        // OpenAI model should have specific hint
        let openai_hint = hints.get_hint("providers.openai.model");
        assert!(openai_hint.is_some());
        assert_eq!(
            openai_hint.unwrap().label,
            Some("OpenAI Model".to_string())
        );

        // Unknown provider should fall back to wildcard
        let unknown_hint = hints.get_hint("providers.unknown.model");
        assert!(unknown_hint.is_some());
        assert_eq!(unknown_hint.unwrap().label, Some("Model".to_string()));
    }

    #[test]
    fn test_group_ordering() {
        let hints = build_ui_hints();
        let sorted = hints.sorted_groups();

        // Verify ordering
        let orders: Vec<i32> = sorted.iter().map(|(_, m)| m.order).collect();
        let mut sorted_orders = orders.clone();
        sorted_orders.sort();
        assert_eq!(orders, sorted_orders);

        // General should come first
        assert_eq!(sorted[0].0, "general");
    }

    #[test]
    fn test_sensitive_fields() {
        let hints = build_ui_hints();

        // API keys should be sensitive
        assert!(hints
            .get_hint("providers.openai.api_key")
            .unwrap()
            .sensitive);
        assert!(hints
            .get_hint("channels.telegram.token")
            .unwrap()
            .sensitive);
        assert!(hints
            .get_hint("channels.discord.token")
            .unwrap()
            .sensitive);

        // Regular fields should not be sensitive
        assert!(!hints.get_hint("general.language").unwrap().sensitive);
    }

    #[test]
    fn test_advanced_fields() {
        let hints = build_ui_hints();

        // Base URL should be advanced
        assert!(hints
            .get_hint("providers.openai.base_url")
            .unwrap()
            .advanced);

        // MCP should be advanced
        assert!(hints.get_hint("mcp.enabled").unwrap().advanced);

        // Language should not be advanced
        assert!(!hints.get_hint("general.language").unwrap().advanced);
    }

    #[test]
    fn test_all_groups_have_order() {
        let hints = build_ui_hints();
        for (id, meta) in &hints.groups {
            assert!(
                meta.order > 0,
                "Group {} should have a positive order",
                id
            );
        }
    }

    #[test]
    fn test_channel_hints() {
        let hints = build_ui_hints();

        // Telegram token
        let telegram = hints.get_hint("channels.telegram.token");
        assert!(telegram.is_some());
        assert!(telegram.unwrap().sensitive);

        // Discord token
        let discord = hints.get_hint("channels.discord.token");
        assert!(discord.is_some());
        assert!(discord.unwrap().sensitive);

        // Wildcard enabled
        let enabled = hints.get_hint("channels.webchat.enabled");
        assert!(enabled.is_some());
    }
}
