// Command Types
//
// Data structures for the command completion system.
//
// ## Flat Namespace Mode
//
// In flat namespace mode, all commands are at root level.
// CommandType::Namespace is deprecated - use source_type for categorization.

use crate::tool_metadata::ToolSourceType;

/// Command type determining behavior on selection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandType {
    /// Execute immediately upon selection
    Action,
    /// Load system prompt, then await user input
    Prompt,
    /// Container with child commands
    Namespace,
}

impl CommandType {
    /// Convert to string for display/logging
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Action => "action",
            Self::Prompt => "prompt",
            Self::Namespace => "namespace",
        }
    }

    /// Parse from string (for config files, case-insensitive)
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let trimmed = s.trim();
        if trimmed.eq_ignore_ascii_case("action") {
            Some(Self::Action)
        } else if trimmed.eq_ignore_ascii_case("prompt") {
            Some(Self::Prompt)
        } else if trimmed.eq_ignore_ascii_case("namespace") {
            Some(Self::Namespace)
        } else {
            None
        }
    }

    /// Get default icon for this command type
    #[must_use]
    pub const fn default_icon(&self) -> &'static str {
        match self {
            Self::Action => "bolt",
            Self::Prompt => "text.quote",
            Self::Namespace => "folder",
        }
    }
}

impl std::fmt::Display for CommandType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A node in the command tree (Flat Namespace Mode)
///
/// In flat namespace mode, all commands are at root level.
/// The `source_type` field indicates where the command comes from
/// (System, MCP, Skill, Custom) for UI badge display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandNode {
    /// Unique identifier within parent namespace (e.g., "search", "git", "commit")
    pub key: String,

    /// Human-readable description for display
    pub description: String,

    /// SF Symbol name for visual representation
    pub icon: String,

    /// Short hint text for command mode display (max ~80px width)
    pub hint: Option<String>,

    /// Command type determining behavior
    pub node_type: CommandType,

    /// Whether this node has child commands
    /// Note: In flat namespace mode, this is always false
    pub has_children: bool,

    /// Optional identifier for dynamic loading (e.g., "mcp:git", "builtin:search")
    pub source_id: Option<String>,

    /// Tool source type for UI badge display (flat namespace mode)
    ///
    /// Indicates where the command comes from:
    /// - Builtin/Native: System commands (/search, /webfetch)
    /// - Mcp: MCP server tools
    /// - Skill: Claude Agent skills
    /// - Custom: User-defined rules
    pub source_type: ToolSourceType,
}

impl CommandNode {
    /// Create a new command node with source type (flat namespace mode)
    pub fn new_with_source(
        key: impl Into<String>,
        description: impl Into<String>,
        source_type: ToolSourceType,
    ) -> Self {
        Self {
            key: key.into(),
            description: description.into(),
            icon: source_type.default_icon().to_string(),
            hint: None,
            node_type: CommandType::Prompt, // Default for flat namespace
            has_children: false,
            source_id: None,
            source_type,
        }
    }

    /// Builder: set icon
    pub fn with_icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = icon.into();
        self
    }

    /// Builder: set hint
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Builder: set `source_id`
    pub fn with_source_id(mut self, source_id: impl Into<String>) -> Self {
        self.source_id = Some(source_id.into());
        self
    }

    /// Builder: set `source_type`
    #[must_use]
    pub const fn with_source_type(mut self, source_type: ToolSourceType) -> Self {
        self.source_type = source_type;
        self
    }

    /// Builder: set `node_type` (override the default)
    #[must_use]
    pub const fn with_node_type(mut self, node_type: CommandType) -> Self {
        self.node_type = node_type;
        self
    }

    /// Check if this is an action node
    #[must_use]
    pub const fn is_action(&self) -> bool {
        matches!(self.node_type, CommandType::Action)
    }

    /// Check if this is a prompt node
    #[must_use]
    pub const fn is_prompt(&self) -> bool {
        matches!(self.node_type, CommandType::Prompt)
    }

    /// Check if this is a system builtin command
    #[must_use]
    pub const fn is_system(&self) -> bool {
        matches!(
            self.source_type,
            ToolSourceType::Builtin | ToolSourceType::Native
        )
    }

    /// Check if this is an MCP tool
    #[must_use]
    pub const fn is_mcp(&self) -> bool {
        matches!(self.source_type, ToolSourceType::Mcp)
    }

    /// Check if this is a skill
    #[must_use]
    pub const fn is_skill(&self) -> bool {
        matches!(self.source_type, ToolSourceType::Skill)
    }

    /// Check if this is a custom command
    #[must_use]
    pub const fn is_custom(&self) -> bool {
        matches!(self.source_type, ToolSourceType::Custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_type_parse() {
        assert_eq!(CommandType::parse("action"), Some(CommandType::Action));
        assert_eq!(CommandType::parse("PROMPT"), Some(CommandType::Prompt));
        assert_eq!(
            CommandType::parse("Namespace"),
            Some(CommandType::Namespace)
        );
        assert_eq!(CommandType::parse("invalid"), None);
    }

    #[test]
    fn test_command_node_builder() {
        let node = CommandNode::new_with_source("test", "Test command", ToolSourceType::Builtin)
            .with_node_type(CommandType::Action)
            .with_icon("star")
            .with_hint("测试")
            .with_source_id("builtin:test");

        assert_eq!(node.key, "test");
        assert_eq!(node.icon, "star");
        assert_eq!(node.hint, Some("测试".to_string()));
        assert_eq!(node.source_id, Some("builtin:test".to_string()));
        assert_eq!(node.source_type, ToolSourceType::Builtin);
        assert!(!node.has_children); // Flat namespace: no children
    }

    #[test]
    fn test_command_node_with_source() {
        let node = CommandNode::new_with_source("git", "Git operations", ToolSourceType::Mcp);

        assert_eq!(node.key, "git");
        assert_eq!(node.source_type, ToolSourceType::Mcp);
        assert_eq!(node.icon, "bolt.fill"); // Default MCP icon
        assert!(!node.has_children);
        assert!(node.is_mcp());
        assert!(!node.is_system());
    }

    #[test]
    fn test_source_type_checks() {
        let builtin = CommandNode::new_with_source("search", "Search", ToolSourceType::Builtin);
        assert!(builtin.is_system());
        assert!(!builtin.is_mcp());

        let mcp = CommandNode::new_with_source("git", "Git", ToolSourceType::Mcp);
        assert!(mcp.is_mcp());
        assert!(!mcp.is_system());

        let skill = CommandNode::new_with_source("refine", "Refine", ToolSourceType::Skill);
        assert!(skill.is_skill());

        let custom = CommandNode::new_with_source("en", "English", ToolSourceType::Custom);
        assert!(custom.is_custom());
    }

    #[test]
    fn test_flat_namespace_no_children() {
        // In flat namespace mode, all nodes have has_children = false
        let node = CommandNode::new_with_source("test", "Test", ToolSourceType::Custom)
            .with_node_type(CommandType::Prompt);
        assert!(!node.has_children);

        let node2 = CommandNode::new_with_source("test2", "Test2", ToolSourceType::Mcp);
        assert!(!node2.has_children);
    }
}
