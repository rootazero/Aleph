// Command Types
//
// Data structures for the command completion system.

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
    pub fn as_str(&self) -> &'static str {
        match self {
            CommandType::Action => "action",
            CommandType::Prompt => "prompt",
            CommandType::Namespace => "namespace",
        }
    }

    /// Parse from string (for config files)
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "action" => Some(CommandType::Action),
            "prompt" => Some(CommandType::Prompt),
            "namespace" => Some(CommandType::Namespace),
            _ => None,
        }
    }

    /// Get default icon for this command type
    pub fn default_icon(&self) -> &'static str {
        match self {
            CommandType::Action => "bolt",
            CommandType::Prompt => "text.quote",
            CommandType::Namespace => "folder",
        }
    }
}

impl std::fmt::Display for CommandType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A node in the command tree
#[derive(Debug, Clone, PartialEq)]
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
    pub has_children: bool,

    /// Optional identifier for dynamic loading (e.g., "mcp:git", "builtin:search")
    pub source_id: Option<String>,
}

impl CommandNode {
    /// Create a new command node
    pub fn new(
        key: impl Into<String>,
        description: impl Into<String>,
        node_type: CommandType,
    ) -> Self {
        let node_type_copy = node_type;
        Self {
            key: key.into(),
            description: description.into(),
            icon: node_type_copy.default_icon().to_string(),
            hint: None,
            node_type,
            has_children: matches!(node_type, CommandType::Namespace),
            source_id: None,
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

    /// Builder: set source_id
    pub fn with_source_id(mut self, source_id: impl Into<String>) -> Self {
        self.source_id = Some(source_id.into());
        self
    }

    /// Builder: set has_children
    pub fn with_children(mut self, has_children: bool) -> Self {
        self.has_children = has_children;
        self
    }

    /// Check if this is a namespace node
    pub fn is_namespace(&self) -> bool {
        matches!(self.node_type, CommandType::Namespace)
    }

    /// Check if this is an action node
    pub fn is_action(&self) -> bool {
        matches!(self.node_type, CommandType::Action)
    }

    /// Check if this is a prompt node
    pub fn is_prompt(&self) -> bool {
        matches!(self.node_type, CommandType::Prompt)
    }
}

/// Result of executing a command
#[derive(Debug, Clone, PartialEq)]
pub struct CommandExecutionResult {
    /// Whether the command executed successfully
    pub success: bool,

    /// Human-readable message (success message or error description)
    pub message: String,

    /// Optional command path that was executed
    pub command_path: Option<String>,

    /// Optional argument that was passed
    pub argument: Option<String>,
}

impl CommandExecutionResult {
    /// Create a success result
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: message.into(),
            command_path: None,
            argument: None,
        }
    }

    /// Create an error result
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: message.into(),
            command_path: None,
            argument: None,
        }
    }

    /// Builder: set command path
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.command_path = Some(path.into());
        self
    }

    /// Builder: set argument
    pub fn with_argument(mut self, arg: impl Into<String>) -> Self {
        self.argument = Some(arg.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_type_parse() {
        assert_eq!(CommandType::parse("action"), Some(CommandType::Action));
        assert_eq!(CommandType::parse("PROMPT"), Some(CommandType::Prompt));
        assert_eq!(CommandType::parse("Namespace"), Some(CommandType::Namespace));
        assert_eq!(CommandType::parse("invalid"), None);
    }

    #[test]
    fn test_command_node_builder() {
        let node = CommandNode::new("test", "Test command", CommandType::Action)
            .with_icon("star")
            .with_hint("测试")
            .with_source_id("builtin:test");

        assert_eq!(node.key, "test");
        assert_eq!(node.icon, "star");
        assert_eq!(node.hint, Some("测试".to_string()));
        assert_eq!(node.source_id, Some("builtin:test".to_string()));
        assert!(!node.has_children);
    }

    #[test]
    fn test_namespace_has_children() {
        let ns = CommandNode::new("mcp", "MCP tools", CommandType::Namespace);
        assert!(ns.has_children);
        assert!(ns.is_namespace());
    }

    #[test]
    fn test_execution_result_builder() {
        let result = CommandExecutionResult::success("Done")
            .with_path("/search")
            .with_argument("weather");

        assert!(result.success);
        assert_eq!(result.command_path, Some("/search".to_string()));
        assert_eq!(result.argument, Some("weather".to_string()));
    }
}
