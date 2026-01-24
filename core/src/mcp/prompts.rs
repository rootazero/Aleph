//! MCP Prompt Manager
//!
//! Manages prompt templates from MCP servers. Prompts are reusable templates
//! that can be parameterized and used as starting points for AI interactions.
//!
//! MCP prompts are similar to prompt libraries - servers can expose templates
//! that clients can list, parameterize, and use in conversations.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::mcp::client::McpClient;

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
    /// Role of the message (user, assistant, system)
    pub role: String,
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

impl PromptContent {
    /// Create text content
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// Check if this is text content
    pub fn is_text(&self) -> bool {
        matches!(self, Self::Text { .. })
    }

    /// Get text content if available
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            _ => None,
        }
    }
}

/// Result of getting a prompt with arguments
#[derive(Debug, Clone)]
pub struct PromptResult {
    /// Optional description override
    pub description: Option<String>,
    /// Messages comprising the prompt
    pub messages: Vec<PromptMessage>,
}

/// Manages MCP prompts across all connected servers
///
/// The prompt manager provides a unified interface for accessing prompt
/// templates from any connected MCP server. It aggregates prompts and
/// handles routing get requests to the appropriate server.
pub struct McpPromptManager {
    client: Arc<McpClient>,
}

impl McpPromptManager {
    /// Create a new prompt manager
    ///
    /// # Arguments
    ///
    /// * `client` - The MCP client that manages server connections
    pub fn new(client: Arc<McpClient>) -> Self {
        Self { client }
    }

    /// List prompts from a specific server
    ///
    /// # Arguments
    ///
    /// * `server` - The server name to query
    ///
    /// # Returns
    ///
    /// A list of prompts available from the server
    pub async fn list(&self, server: &str) -> Result<Vec<McpPrompt>> {
        // TODO: Implement prompts/list call to specific server
        // This requires adding prompts/list support to McpServerConnection
        tracing::debug!(server = %server, "Listing prompts (stub - not yet implemented)");
        Ok(Vec::new())
    }

    /// Get a prompt by name with optional arguments
    ///
    /// # Arguments
    ///
    /// * `server` - The server name that owns the prompt
    /// * `name` - The prompt name
    /// * `arguments` - Optional arguments to parameterize the prompt
    ///
    /// # Returns
    ///
    /// The expanded prompt with messages
    pub async fn get(
        &self,
        server: &str,
        name: &str,
        arguments: Option<HashMap<String, Value>>,
    ) -> Result<PromptResult> {
        // TODO: Implement prompts/get call
        // This requires adding prompts/get support to McpServerConnection
        tracing::debug!(
            server = %server,
            name = %name,
            has_args = arguments.is_some(),
            "Getting prompt (stub - not yet implemented)"
        );
        Ok(PromptResult {
            description: None,
            messages: Vec::new(),
        })
    }

    /// List all prompts from all connected servers
    ///
    /// Aggregates prompts from all servers, returning a map from
    /// server name to its prompts.
    ///
    /// # Returns
    ///
    /// A map of server names to their prompt lists
    pub async fn list_all(&self) -> Result<HashMap<String, Vec<McpPrompt>>> {
        let mut all_prompts = HashMap::new();

        let server_names = self.client.service_names().await;
        for server in server_names {
            match self.list(&server).await {
                Ok(prompts) => {
                    all_prompts.insert(server, prompts);
                }
                Err(e) => {
                    tracing::warn!(
                        server = %server,
                        error = %e,
                        "Failed to list prompts from server"
                    );
                }
            }
        }

        Ok(all_prompts)
    }

    /// Find a prompt by name across all servers
    ///
    /// Searches all connected servers for a prompt with the given name.
    /// Returns the first match found.
    ///
    /// # Arguments
    ///
    /// * `name` - The prompt name to search for
    ///
    /// # Returns
    ///
    /// The prompt and its server name, if found
    pub async fn find(&self, name: &str) -> Option<(String, McpPrompt)> {
        let all_prompts = self.list_all().await.ok()?;

        for (server, prompts) in all_prompts {
            for prompt in prompts {
                if prompt.name == name {
                    return Some((server, prompt));
                }
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_prompt_manager_creation() {
        let client = Arc::new(McpClient::new());
        let manager = McpPromptManager::new(client);

        let prompts = manager.list_all().await;
        assert!(prompts.is_ok());
        assert!(prompts.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_prompt_manager_list_empty() {
        let client = Arc::new(McpClient::new());
        let manager = McpPromptManager::new(client);

        let prompts = manager.list("nonexistent-server").await;
        assert!(prompts.is_ok());
        assert!(prompts.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_prompt_manager_get_stub() {
        let client = Arc::new(McpClient::new());
        let manager = McpPromptManager::new(client);

        let result = manager.get("server", "test-prompt", None).await;
        assert!(result.is_ok());
        let prompt_result = result.unwrap();
        assert!(prompt_result.messages.is_empty());
    }

    #[tokio::test]
    async fn test_prompt_manager_find_not_found() {
        let client = Arc::new(McpClient::new());
        let manager = McpPromptManager::new(client);

        let result = manager.find("nonexistent-prompt").await;
        assert!(result.is_none());
    }

    #[test]
    fn test_prompt_content_text() {
        let content = PromptContent::text("Hello, world!");
        assert!(content.is_text());
        assert_eq!(content.as_text(), Some("Hello, world!"));
    }

    #[test]
    fn test_prompt_content_image() {
        let content = PromptContent::Image {
            data: "base64data".to_string(),
            mime_type: "image/png".to_string(),
        };
        assert!(!content.is_text());
        assert!(content.as_text().is_none());
    }

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
