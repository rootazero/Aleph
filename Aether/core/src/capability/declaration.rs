//! Capability declarations for AI-first intent detection.
//!
//! This module defines the structures used to describe available capabilities
//! to the AI model, enabling it to understand when and how to use each capability.

use serde::{Deserialize, Serialize};

/// Information about an MCP tool for capability declaration.
///
/// This is a simplified version of McpTool for capability declaration purposes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolInfo {
    /// Tool name (e.g., "screen_capture", "file_read")
    pub name: String,
    /// Tool description for AI understanding
    pub description: String,
    /// Input schema as JSON value
    pub input_schema: serde_json::Value,
    /// Whether this tool requires user confirmation before execution
    pub requires_confirmation: bool,
}

/// Declaration of a capability for AI understanding.
///
/// This structure is used to build the system prompt that informs the AI
/// about available capabilities and how to invoke them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityDeclaration {
    /// Unique identifier (e.g., "search", "video")
    pub id: String,
    /// Human-readable name (e.g., "Web Search")
    pub name: String,
    /// Description for AI to understand when to use this capability
    pub description: String,
    /// Parameters this capability accepts
    pub parameters: Vec<CapabilityParameter>,
    /// Example queries that should trigger this capability
    pub examples: Vec<String>,
    /// Whether this capability is currently available
    pub available: bool,
    /// MCP tools information (only populated for MCP capability)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_tools: Option<Vec<McpToolInfo>>,
}

impl CapabilityDeclaration {
    /// Create a new capability declaration.
    pub fn new(id: impl Into<String>, name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            parameters: Vec::new(),
            examples: Vec::new(),
            available: true,
            mcp_tools: None,
        }
    }

    /// Add a parameter to this capability.
    pub fn with_parameter(mut self, param: CapabilityParameter) -> Self {
        self.parameters.push(param);
        self
    }

    /// Add an example to this capability.
    pub fn with_example(mut self, example: impl Into<String>) -> Self {
        self.examples.push(example.into());
        self
    }

    /// Set whether this capability is available.
    pub fn with_available(mut self, available: bool) -> Self {
        self.available = available;
        self
    }

    /// Create the Search capability declaration.
    pub fn search() -> Self {
        Self::new(
            "search",
            "Web Search",
            "Search the web for current information including weather, news, prices, facts, and real-time data. Use this when the user asks about current events, weather, stock prices, or any information that requires up-to-date data.",
        )
        .with_parameter(CapabilityParameter::new(
            "query",
            "string",
            "The search query to execute",
            true,
        ))
        .with_example("What's the weather in Tokyo?")
        .with_example("Latest news about AI")
        .with_example("Bitcoin price today")
        .with_example("今天北京天气怎么样")
        .with_example("最新的科技新闻")
    }

    /// Create the Video capability declaration.
    pub fn video() -> Self {
        Self::new(
            "video",
            "Video Transcript",
            "Extract and analyze transcripts from YouTube videos. Use this when the user provides a YouTube URL and wants to summarize, analyze, or ask questions about the video content.",
        )
        .with_parameter(CapabilityParameter::new(
            "url",
            "url",
            "The YouTube video URL",
            true,
        ))
        .with_example("Summarize this video: https://youtube.com/watch?v=xxx")
        .with_example("What is this video about? https://youtu.be/xxx")
        .with_example("总结一下这个视频 https://youtube.com/watch?v=xxx")
    }

    /// Create the MCP capability declaration.
    ///
    /// When MCP tools are available, this capability allows the AI to invoke
    /// system tools like screen_capture, file operations, git commands, etc.
    pub fn mcp() -> Self {
        Self::new(
            "mcp",
            "MCP Tools",
            "Execute Model Context Protocol (MCP) tools for system operations. Use this when the user wants to interact with the system, such as taking screenshots, reading/writing files, running git commands, or executing shell commands.",
        )
        .with_parameter(CapabilityParameter::new(
            "tool",
            "string",
            "The MCP tool name to invoke (e.g., 'screen_capture', 'file_read', 'git_status')",
            true,
        ))
        .with_parameter(CapabilityParameter::new(
            "args",
            "object",
            "Arguments to pass to the MCP tool (tool-specific parameters)",
            false,
        ))
        .with_example("Take a screenshot")
        .with_example("截屏")
        .with_example("Read file /path/to/file")
        .with_example("Show git status")
        .with_example("List files in current directory")
    }

    /// Create the MCP capability declaration with a specific tool list.
    ///
    /// This variant includes the list of available tools so the AI knows
    /// exactly which tools it can invoke.
    pub fn mcp_with_tools(tools: Vec<McpToolInfo>) -> Self {
        let tool_list: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        let description = if tools.is_empty() {
            "No MCP tools currently available.".to_string()
        } else {
            format!(
                "Execute Model Context Protocol (MCP) tools for system operations. Available tools: {}. Use these tools when the user wants to interact with the system.",
                tool_list.join(", ")
            )
        };

        let mut decl = Self::new("mcp", "MCP Tools", description)
            .with_parameter(CapabilityParameter::new(
                "tool",
                "string",
                &format!("The MCP tool name to invoke. Must be one of: {}", tool_list.join(", ")),
                true,
            ))
            .with_parameter(CapabilityParameter::new(
                "args",
                "object",
                "Arguments to pass to the MCP tool (tool-specific parameters)",
                false,
            ));

        // Add examples for common tools
        for tool in &tools {
            if tool.name == "screen_capture" {
                decl = decl.with_example("Take a screenshot");
                decl = decl.with_example("截屏");
            } else if tool.name.contains("file_read") {
                decl = decl.with_example("Read file /path/to/file");
            } else if tool.name.contains("git") {
                decl = decl.with_example("Show git status");
            }
        }

        // Store the full tool info for prompt generation
        decl.mcp_tools = Some(tools);
        decl
    }

    /// Create the Skill capability declaration (reserved for future).
    pub fn skill() -> Self {
        Self::new(
            "skill",
            "Skill Workflow",
            "Execute complex multi-step workflows using specialized skills.",
        )
        .with_parameter(CapabilityParameter::new(
            "skill_name",
            "string",
            "The skill to execute",
            true,
        ))
        .with_parameter(CapabilityParameter::new(
            "args",
            "object",
            "Arguments for the skill",
            false,
        ))
        .with_available(false) // Not yet implemented
    }
}

/// Parameter definition for a capability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityParameter {
    /// Parameter name
    pub name: String,
    /// Parameter type ("string", "url", "number", "object")
    pub param_type: String,
    /// Description of what this parameter does
    pub description: String,
    /// Whether this parameter is required
    pub required: bool,
}

impl CapabilityParameter {
    /// Create a new capability parameter.
    pub fn new(
        name: impl Into<String>,
        param_type: impl Into<String>,
        description: impl Into<String>,
        required: bool,
    ) -> Self {
        Self {
            name: name.into(),
            param_type: param_type.into(),
            description: description.into(),
            required,
        }
    }
}

/// Registry of all available capabilities.
///
/// This structure manages the set of capabilities that are available
/// for the AI to use in the current session.
#[derive(Debug, Clone, Default)]
pub struct CapabilityRegistry {
    /// List of registered capabilities
    capabilities: Vec<CapabilityDeclaration>,
}

impl CapabilityRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a capability.
    pub fn register(&mut self, capability: CapabilityDeclaration) {
        self.capabilities.push(capability);
    }

    /// Get all available capabilities.
    pub fn available(&self) -> Vec<&CapabilityDeclaration> {
        self.capabilities.iter().filter(|c| c.available).collect()
    }

    /// Get all capabilities (including unavailable).
    pub fn all(&self) -> &[CapabilityDeclaration] {
        &self.capabilities
    }

    /// Check if a capability is available by ID.
    pub fn is_available(&self, id: &str) -> bool {
        self.capabilities.iter().any(|c| c.id == id && c.available)
    }

    /// Get a capability by ID.
    pub fn get(&self, id: &str) -> Option<&CapabilityDeclaration> {
        self.capabilities.iter().find(|c| c.id == id)
    }

    /// Build a registry with default capabilities based on configuration.
    pub fn with_defaults(search_enabled: bool, video_enabled: bool) -> Self {
        Self::with_all_capabilities(search_enabled, video_enabled, None)
    }

    /// Build a registry with all capabilities including MCP tools.
    ///
    /// # Arguments
    ///
    /// * `search_enabled` - Whether search capability is available
    /// * `video_enabled` - Whether video capability is available
    /// * `mcp_tools` - Optional list of MCP tools to register
    pub fn with_all_capabilities(
        search_enabled: bool,
        video_enabled: bool,
        mcp_tools: Option<Vec<McpToolInfo>>,
    ) -> Self {
        let mut registry = Self::new();

        if search_enabled {
            registry.register(CapabilityDeclaration::search());
        }

        if video_enabled {
            registry.register(CapabilityDeclaration::video());
        }

        // Register MCP capability if tools are provided
        if let Some(tools) = mcp_tools {
            if !tools.is_empty() {
                registry.register(CapabilityDeclaration::mcp_with_tools(tools));
            }
        }

        // Future capabilities (always registered but marked unavailable)
        // registry.register(CapabilityDeclaration::skill());

        registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capability_declaration_builder() {
        let cap = CapabilityDeclaration::new("test", "Test Cap", "A test capability")
            .with_parameter(CapabilityParameter::new("query", "string", "Search query", true))
            .with_example("test example")
            .with_available(true);

        assert_eq!(cap.id, "test");
        assert_eq!(cap.name, "Test Cap");
        assert_eq!(cap.parameters.len(), 1);
        assert_eq!(cap.examples.len(), 1);
        assert!(cap.available);
    }

    #[test]
    fn test_search_declaration() {
        let search = CapabilityDeclaration::search();
        assert_eq!(search.id, "search");
        assert!(!search.parameters.is_empty());
        assert!(!search.examples.is_empty());
        assert!(search.available);
    }

    #[test]
    fn test_video_declaration() {
        let video = CapabilityDeclaration::video();
        assert_eq!(video.id, "video");
        assert!(!video.parameters.is_empty());
        assert!(video.available);
    }

    #[test]
    fn test_registry() {
        let mut registry = CapabilityRegistry::new();
        registry.register(CapabilityDeclaration::search());
        registry.register(CapabilityDeclaration::video());
        registry.register(CapabilityDeclaration::mcp());

        assert_eq!(registry.all().len(), 3);
        assert_eq!(registry.available().len(), 3); // All are available
        assert!(registry.is_available("search"));
        assert!(registry.is_available("mcp"));
    }

    #[test]
    fn test_registry_with_defaults() {
        let registry = CapabilityRegistry::with_defaults(true, true);
        assert!(registry.is_available("search"));
        assert!(registry.is_available("video"));
    }
}
