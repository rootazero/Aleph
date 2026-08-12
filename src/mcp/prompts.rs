//! MCP Prompt Types
//!
//! Prompt template types for MCP servers. Prompts are reusable templates
//! that can be parameterized and used as starting points for AI interactions.
//!
//! MCP prompts are similar to prompt libraries - servers can expose templates
//! that clients can list, parameterize, and use in conversations.

use serde::{Deserialize, Serialize};

pub use crate::mcp::protocol::PromptRole;

/// MCP prompt definition from a server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPrompt {
    /// Unique prompt name
    pub name: String,
    /// Human-readable description
    pub description: Option<String>,
    /// Arguments this prompt accepts
    pub arguments: Vec<McpPromptArgument>,
}

/// Argument definition for an MCP prompt
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPromptArgument {
    /// Argument name
    pub name: String,
    /// Argument description
    pub description: Option<String>,
    /// Whether this argument is required
    #[serde(default)]
    pub required: bool,
}

/// A message in a prompt response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptMessage {
    /// Role of the message. Uses the wire-level [`PromptRole`] enum so a
    /// new variant (e.g. `Tool`, added in later revisions) round-trips
    /// rather than silently collapsing to "".
    pub role: PromptRole,
    /// Content of the message
    pub content: PromptContent,
}

/// Content types in a prompt message
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PromptContent {
    /// Text content
    #[serde(rename = "text")]
    Text { text: String },
    /// Image content (base64)
    #[serde(rename = "image")]
    Image { data: String, mime_type: String },
    /// Resource reference
    #[serde(rename = "resource")]
    Resource { uri: String, text: Option<String> },
}

/// Result of getting a prompt with arguments
#[derive(Debug, Clone)]
pub struct PromptResult {
    /// Optional description override
    pub description: Option<String>,
    /// Messages comprising the prompt
    pub messages: Vec<PromptMessage>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_prompt_serialization() {
        let prompt = McpPrompt {
            name: "test-prompt".to_string(),
            description: Some("A test prompt".to_string()),
            arguments: vec![McpPromptArgument {
                name: "query".to_string(),
                description: Some("Search query".to_string()),
                required: true,
            }],
        };

        let json = serde_json::to_string(&prompt).unwrap();
        assert!(json.contains("test-prompt"));
        assert!(json.contains("query"));

        let deserialized: McpPrompt = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "test-prompt");
        assert_eq!(deserialized.arguments.len(), 1);
    }
}
