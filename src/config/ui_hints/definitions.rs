//! Built-in UI hints definitions for Aleph configuration.
//!
//! This module contains the default UI hints for all standard Aleph configuration
//! fields. It uses the `define_groups!` and `define_hints!` macros for a declarative
//! definition style.

use super::{ConfigUiHints, FieldHint, GroupMeta};
use crate::{define_groups, define_hints};

/// Build the complete UI hints for Aleph configuration.
///
/// Returns a `ConfigUiHints` instance containing all default groups and field hints.
///
/// # Example
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
        "memory.dreaming.enabled" => {
            label: "DreamDaemon Enabled",
            help: "Enable idle-time memory consolidation",
            group: "memory",
            order: 5,
            advanced: true,
        },
        "memory.dreaming.window_start_local" => {
            label: "Dreaming Window Start",
            help: "Local time (HH:MM) when dreaming can start",
            group: "memory",
            order: 7,
            advanced: true,
        },
        "memory.dreaming.window_end_local" => {
            label: "Dreaming Window End",
            help: "Local time (HH:MM) when dreaming ends",
            group: "memory",
            order: 8,
            advanced: true,
        },
        "memory.dreaming.max_duration_seconds" => {
            label: "Dreaming Max Duration",
            help: "Maximum seconds per DreamDaemon run",
            group: "memory",
            order: 9,
            advanced: true,
        },
        "memory.memory_decay.half_life_days" => {
            label: "Fact Half-Life (Days)",
            help: "Half-life for memory fact decay",
            group: "memory",
            advanced: true,
        },
        "memory.memory_decay.min_strength" => {
            label: "Fact Min Strength",
            help: "Minimum strength before pruning facts",
            group: "memory",
            advanced: true,
        },
        "memory.memory_decay.protected_types" => {
            label: "Fact Protected Types",
            help: "Fact types protected from decay",
            group: "memory",
            advanced: true,
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

        // === Gateway ===
        "gateway.port" => {
            label: "Gateway Port",
            help: "Port for the WebSocket gateway",
            group: "advanced",
            placeholder: "18790",
        },
        "gateway.bind" => {
            label: "Bind Address",
            help: "Address to bind the gateway (loopback, all, or specific IP)",
            group: "advanced",
        },

        // === Session ===
        "session.dm_scope" => {
            label: "DM Scope",
            help: "Session isolation strategy for direct messages",
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
    fn test_all_groups_have_order() {
        let hints = build_ui_hints();
        for (id, meta) in &hints.groups {
            assert!(meta.order > 0, "Group {} should have a positive order", id);
        }
    }
}
