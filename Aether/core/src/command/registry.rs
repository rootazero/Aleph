// Command Registry
//
// Manages the command tree and provides APIs for:
// - Getting root commands
// - Getting children of a namespace
// - Filtering commands by prefix
// - Executing commands

use std::collections::HashMap;

use crate::config::{Config, RoutingRuleConfig};
use crate::skills::SkillInfo;

use super::types::{CommandExecutionResult, CommandNode, CommandType};

/// Builtin command hints with localization support
///
/// Returns the hint for a builtin command in the specified language.
/// Falls back to English if the language is not supported.
pub fn get_builtin_hint(command_key: &str, language: &str) -> Option<String> {
    // Builtin hints table (Flat Namespace Mode)
    // Format: (command_key, english_hint, chinese_hint)
    // Note: /mcp and /skill removed - tools are registered directly
    static BUILTIN_HINTS: &[(&str, &str, &str)] = &[
        ("search", "Web search", "网页搜索"),
        ("video", "Video info", "视频信息"),
        ("chat", "Chat", "对话"),
    ];

    let is_chinese = language.starts_with("zh");

    for (key, en_hint, zh_hint) in BUILTIN_HINTS {
        if *key == command_key {
            return Some(if is_chinese { zh_hint } else { en_hint }.to_string());
        }
    }

    None
}

/// Command Registry - manages the command tree
pub struct CommandRegistry {
    /// Static commands from config.toml rules
    builtin_commands: Vec<CommandNode>,

    /// Children map: parent_key -> children
    children_map: HashMap<String, Vec<CommandNode>>,

    /// Current language for hint localization
    language: String,

    /// Whether to show command hints
    show_hints: bool,
}

impl CommandRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            builtin_commands: Vec::new(),
            children_map: HashMap::new(),
            language: "en".to_string(),
            show_hints: true,
        }
    }

    /// Create registry from config
    pub fn from_config(config: &Config, language: &str) -> Self {
        let mut registry = Self::new();
        registry.language = language.to_string();
        registry.show_hints = config.general.show_command_hints.unwrap_or(true);

        // Parse routing rules into command nodes
        for rule in &config.rules {
            if let Some(node) = Self::rule_to_command_node(rule, language, registry.show_hints) {
                registry.builtin_commands.push(node);
            }
        }

        // Sort commands alphabetically by key for consistent display
        registry
            .builtin_commands
            .sort_by(|a, b| a.key.cmp(&b.key));

        // NOTE: In flat namespace mode, MCP namespace is NOT added.
        // MCP tools are registered directly as root commands via ToolRegistry.

        registry
    }

    /// Convert a routing rule to a command node (if it's a command rule)
    fn rule_to_command_node(
        rule: &RoutingRuleConfig,
        language: &str,
        show_hints: bool,
    ) -> Option<CommandNode> {
        // Only process command rules (regex starts with ^/)
        if !rule.regex.starts_with("^/") {
            return None;
        }

        // Extract command key from regex
        // e.g., "^/search" -> "search", "^/en\\s*" -> "en"
        let key = rule
            .regex
            .trim_start_matches("^/")
            .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
            .next()
            .unwrap_or("")
            .to_string();

        if key.is_empty() {
            return None;
        }

        // Determine command type based on capabilities
        let has_capabilities = rule
            .capabilities
            .as_ref()
            .map(|c| !c.is_empty())
            .unwrap_or(false);

        let node_type = if has_capabilities {
            CommandType::Action
        } else {
            CommandType::Prompt
        };

        // Get description from system_prompt (first line) or generate from key
        let description = rule
            .system_prompt
            .as_ref()
            .and_then(|p| p.lines().next())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| format!("/{} command", key));

        // Truncate description if too long (using char boundaries for UTF-8 safety)
        let description = if description.chars().count() > 50 {
            let truncated: String = description.chars().take(47).collect();
            format!("{}...", truncated)
        } else {
            description
        };

        // Get icon from rule or use default
        let icon = rule
            .icon
            .clone()
            .unwrap_or_else(|| node_type.default_icon().to_string());

        // Get hint: first try rule's hint field, then try builtin hints
        let hint = if show_hints {
            rule.hint
                .clone()
                .or_else(|| get_builtin_hint(&key, language))
        } else {
            None
        };

        let source_id = format!("rule:{}", key);

        let mut node = CommandNode::new(key, description, node_type)
            .with_icon(icon)
            .with_source_id(source_id);

        if let Some(h) = hint {
            node = node.with_hint(h);
        }

        Some(node)
    }

    /// Get all root-level commands
    pub fn get_root_commands(&self) -> Vec<CommandNode> {
        self.builtin_commands.clone()
    }

    /// Get children of a namespace command
    pub fn get_children(&self, parent_key: &str) -> Vec<CommandNode> {
        // In flat namespace mode, there are no namespace hierarchies.
        // All tools are root-level commands.
        // This method is kept for backward compatibility but returns empty.
        if let Some(children) = self.children_map.get(parent_key) {
            return children.clone();
        }

        Vec::new()
    }

    /// Filter commands by key prefix (case-insensitive)
    pub fn filter_by_prefix(commands: &[CommandNode], prefix: &str) -> Vec<CommandNode> {
        log::debug!(
            "[CommandRegistry] filter_by_prefix: prefix='{}', commands_count={}",
            prefix,
            commands.len()
        );

        if prefix.is_empty() {
            return commands.to_vec();
        }

        let prefix_lower = prefix.to_lowercase();
        let filtered: Vec<CommandNode> = commands
            .iter()
            .filter(|cmd| {
                let matches = cmd.key.to_lowercase().starts_with(&prefix_lower);
                log::debug!(
                    "[CommandRegistry] Checking '{}' against prefix '{}': {}",
                    cmd.key,
                    prefix_lower,
                    matches
                );
                matches
            })
            .cloned()
            .collect();

        log::debug!(
            "[CommandRegistry] filter_by_prefix result: {} matches",
            filtered.len()
        );

        filtered
    }

    /// Execute a command by path
    ///
    /// Returns a result indicating success/failure.
    /// For Action commands, this triggers the associated capability.
    /// For Prompt commands, this loads the system prompt.
    pub fn execute_command(
        &self,
        command_path: &str,
        argument: Option<&str>,
    ) -> CommandExecutionResult {
        // Parse command path
        let path_parts: Vec<&str> = command_path
            .trim_start_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();

        if path_parts.is_empty() {
            return CommandExecutionResult::error("Empty command path");
        }

        // Find the command in root commands
        let root_key = path_parts[0];
        let command = self.builtin_commands.iter().find(|c| c.key == root_key);

        match command {
            Some(cmd) => {
                if path_parts.len() == 1 {
                    // Execute this command
                    match cmd.node_type {
                        CommandType::Action => {
                            let mut result = CommandExecutionResult::success(format!(
                                "Executing action: {}",
                                cmd.key
                            ))
                            .with_path(command_path.to_string());

                            if let Some(arg) = argument {
                                result = result.with_argument(arg.to_string());
                            }
                            result
                        }
                        CommandType::Prompt => {
                            let mut result = CommandExecutionResult::success(format!(
                                "Loading prompt: {}",
                                cmd.key
                            ))
                            .with_path(command_path.to_string());

                            if let Some(arg) = argument {
                                result = result.with_argument(arg.to_string());
                            }
                            result
                        }
                        CommandType::Namespace => {
                            CommandExecutionResult::error("Cannot execute namespace directly")
                        }
                    }
                } else {
                    // Navigate to nested command (future: MCP tools)
                    CommandExecutionResult::error(format!(
                        "Nested command not found: {}",
                        command_path
                    ))
                }
            }
            None => {
                CommandExecutionResult::error(format!("Command not found: {}", root_key))
            }
        }
    }

    /// Set the language for hint localization
    pub fn set_language(&mut self, language: &str) {
        self.language = language.to_string();
        // Re-apply hints with new language
        for node in &mut self.builtin_commands {
            if let Some(source_id) = &node.source_id {
                let is_rule_or_builtin =
                    source_id.starts_with("rule:") || source_id.starts_with("builtin:");
                if is_rule_or_builtin && self.show_hints {
                    node.hint = get_builtin_hint(&node.key, language);
                }
            }
        }
    }

    /// Set whether to show command hints
    pub fn set_show_hints(&mut self, show: bool) {
        self.show_hints = show;
        if !show {
            // Clear all hints
            for node in &mut self.builtin_commands {
                node.hint = None;
            }
        } else {
            // Re-apply hints
            for node in &mut self.builtin_commands {
                if node.hint.is_none() {
                    node.hint = get_builtin_hint(&node.key, &self.language);
                }
            }
        }
    }

    /// Add static children for a parent key (for testing/MCP stub)
    pub fn add_children(&mut self, parent_key: &str, children: Vec<CommandNode>) {
        self.children_map.insert(parent_key.to_string(), children);
    }

    /// Inject installed skills as children of the /skill command
    ///
    /// This method:
    /// 1. Ensures the /skill command exists and has_children is set to true
    /// 2. Adds all installed skills as subcommands
    pub fn inject_skills(&mut self, skills: &[SkillInfo]) {
        // Ensure /skill command exists and is marked as having children
        for node in &mut self.builtin_commands {
            if node.key == "skill" {
                node.has_children = true;
                break;
            }
        }

        // If no /skill command exists, add it as a namespace
        let has_skill = self.builtin_commands.iter().any(|n| n.key == "skill");
        if !has_skill {
            let skill_hint = if self.show_hints {
                get_builtin_hint("skill", &self.language)
            } else {
                None
            };

            let mut skill_node =
                CommandNode::new("skill", "Execute predefined skill workflows", CommandType::Prompt)
                    .with_icon("wand.and.stars")
                    .with_source_id("builtin:skill");

            skill_node.has_children = true;

            if let Some(hint) = skill_hint {
                skill_node = skill_node.with_hint(hint);
            }

            self.builtin_commands.push(skill_node);
            // Re-sort after adding
            self.builtin_commands
                .sort_by(|a, b| a.key.cmp(&b.key));
        }

        // Build skill children nodes
        let skill_children: Vec<CommandNode> = skills
            .iter()
            .map(|skill| {
                let hint = if self.show_hints {
                    Some(skill.name.clone())
                } else {
                    None
                };

                let mut node = CommandNode::new(
                    skill.id.clone(),
                    skill.description.clone(),
                    CommandType::Prompt,
                )
                .with_icon("wand.and.stars")
                .with_source_id(format!("skill:{}", skill.id));

                if let Some(h) = hint {
                    node = node.with_hint(h);
                }

                node
            })
            .collect();

        // Add skills as children of /skill
        self.children_map
            .insert("skill".to_string(), skill_children);
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> Config {
        let mut config = Config::default();
        config.rules = vec![
            RoutingRuleConfig {
                regex: "^/search".to_string(),
                provider: Some("openai".to_string()),
                capabilities: Some(vec!["search".to_string()]),
                system_prompt: Some("Search the web".to_string()),
                icon: Some("magnifyingglass".to_string()),
                hint: None,
                ..Default::default()
            },
            RoutingRuleConfig {
                regex: "^/en".to_string(),
                provider: Some("openai".to_string()),
                capabilities: None,
                system_prompt: Some("Translate to English".to_string()),
                icon: Some("globe".to_string()),
                hint: Some("译英文".to_string()),
                ..Default::default()
            },
            // Keyword rule (should be ignored)
            RoutingRuleConfig {
                regex: "翻译".to_string(),
                provider: None,
                system_prompt: Some("Translate".to_string()),
                ..Default::default()
            },
        ];
        config
    }

    #[test]
    fn test_registry_from_config() {
        let config = create_test_config();
        let registry = CommandRegistry::from_config(&config, "zh-Hans");

        let commands = registry.get_root_commands();

        // In flat namespace mode: en, search, video, chat (sorted alphabetically)
        // No /mcp namespace - MCP tools are registered directly
        assert!(commands.len() >= 2);

        let search = commands.iter().find(|c| c.key == "search");
        assert!(search.is_some());
        let search = search.unwrap();
        assert_eq!(search.node_type, CommandType::Action);
        assert_eq!(search.icon, "magnifyingglass");
        assert_eq!(search.hint, Some("网页搜索".to_string())); // Builtin hint

        let en = commands.iter().find(|c| c.key == "en");
        assert!(en.is_some());
        let en = en.unwrap();
        assert_eq!(en.node_type, CommandType::Prompt);
        assert_eq!(en.hint, Some("译英文".to_string())); // User-defined hint
    }

    #[test]
    fn test_filter_by_prefix() {
        let commands = vec![
            CommandNode::new("search", "Search", CommandType::Action),
            CommandNode::new("settings", "Settings", CommandType::Namespace),
            CommandNode::new("share", "Share", CommandType::Action),
            CommandNode::new("en", "English", CommandType::Prompt),
        ];

        let filtered = CommandRegistry::filter_by_prefix(&commands, "se");
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().any(|c| c.key == "search"));
        assert!(filtered.iter().any(|c| c.key == "settings"));

        let filtered = CommandRegistry::filter_by_prefix(&commands, "SE"); // Case insensitive
        assert_eq!(filtered.len(), 2);

        let filtered = CommandRegistry::filter_by_prefix(&commands, "");
        assert_eq!(filtered.len(), 4); // All commands

        let filtered = CommandRegistry::filter_by_prefix(&commands, "xyz");
        assert_eq!(filtered.len(), 0);
    }

    #[test]
    fn test_execute_command() {
        let config = create_test_config();
        let registry = CommandRegistry::from_config(&config, "en");

        // Execute action command
        let result = registry.execute_command("/search", Some("weather"));
        assert!(result.success);
        assert!(result.message.contains("action"));

        // Execute prompt command
        let result = registry.execute_command("/en", Some("你好"));
        assert!(result.success);
        assert!(result.message.contains("prompt"));

        // Execute unknown command
        let result = registry.execute_command("/unknown", None);
        assert!(!result.success);
        assert!(result.message.contains("not found"));
    }

    #[test]
    fn test_builtin_hints() {
        assert_eq!(
            get_builtin_hint("search", "en"),
            Some("Web search".to_string())
        );
        assert_eq!(
            get_builtin_hint("search", "zh-Hans"),
            Some("网页搜索".to_string())
        );
        assert_eq!(
            get_builtin_hint("unknown", "en"),
            None
        );
    }

    #[test]
    fn test_hints_toggle() {
        let config = create_test_config();
        let mut registry = CommandRegistry::from_config(&config, "en");

        // Hints should be visible by default
        let commands = registry.get_root_commands();
        let search = commands.iter().find(|c| c.key == "search").unwrap();
        assert!(search.hint.is_some());

        // Disable hints
        registry.set_show_hints(false);
        let commands = registry.get_root_commands();
        let search = commands.iter().find(|c| c.key == "search").unwrap();
        assert!(search.hint.is_none());

        // Re-enable hints
        registry.set_show_hints(true);
        let commands = registry.get_root_commands();
        let search = commands.iter().find(|c| c.key == "search").unwrap();
        assert!(search.hint.is_some());
    }

    #[test]
    fn test_flat_namespace_no_mcp_builtin() {
        // In flat namespace mode, /mcp is NOT a builtin command
        // MCP tools are registered directly as root commands (e.g., /git)
        let config = Config::default();
        let registry = CommandRegistry::from_config(&config, "en");

        let commands = registry.get_root_commands();
        let mcp = commands.iter().find(|c| c.key == "mcp");
        // /mcp should NOT exist in flat namespace mode
        assert!(mcp.is_none(), "/mcp namespace should not exist in flat namespace mode");
    }

    #[test]
    fn test_flat_namespace_builtins() {
        // In flat namespace mode, only 3 builtins: search, video, chat
        let config = Config::default();
        let registry = CommandRegistry::from_config(&config, "en");

        let commands = registry.get_root_commands();

        // Verify the 3 builtins exist
        assert!(commands.iter().any(|c| c.key == "search"), "search should exist");
        assert!(commands.iter().any(|c| c.key == "video"), "video should exist");
        assert!(commands.iter().any(|c| c.key == "chat"), "chat should exist");

        // Verify no /mcp or /skill namespace
        assert!(!commands.iter().any(|c| c.key == "mcp"), "/mcp should not exist");
        assert!(!commands.iter().any(|c| c.key == "skill"), "/skill should not exist");
    }
}
