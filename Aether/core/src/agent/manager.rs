//! Rig Agent Manager - core entry point with hot-reload support
//!
//! This module provides the main manager for rig-based agent operations,
//! including request processing, streaming capabilities, and dynamic tool management.
//!
//! # Hot-Reload Support
//!
//! The manager uses `ToolServerHandle` to support runtime tool addition/removal:
//! - MCP tools can be added when user connects new MCP servers
//! - Skills can be added when user installs new skills
//! - Tools can be removed when user disconnects servers or uninstalls skills

use super::config::RigAgentConfig;
use crate::core::MediaAttachment;
use crate::error::{AetherError, Result};
use crate::rig_tools::{SearchTool, WebFetchTool, YouTubeTool};
use crate::store::MemoryStore;
use rig::client::CompletionClient;
use rig::completion::message::{DocumentSourceKind, Image, ImageMediaType, Text, UserContent};
use rig::completion::{Message, Prompt};
use rig::providers::{anthropic, openai};
use rig::tool::server::{ToolServer, ToolServerHandle};
use rig::tool::{ToolDyn, ToolSet};
use rig::OneOrMany;
use std::sync::{Arc, RwLock};
use tracing::{debug, info, warn};

/// Response from agent processing
#[derive(Debug, Clone)]
pub struct AgentResponse {
    /// Generated response text
    pub content: String,
    /// Tools that were called during processing
    pub tools_called: Vec<String>,
}

impl AgentResponse {
    /// Create a new AgentResponse
    pub fn new(content: String, tools_called: Vec<String>) -> Self {
        Self {
            content,
            tools_called,
        }
    }

    /// Create a simple response with no tools called
    pub fn simple(content: String) -> Self {
        Self {
            content,
            tools_called: Vec::new(),
        }
    }
}

/// Manages the rig Agent lifecycle with hot-reload support
///
/// Uses `ToolServerHandle` for dynamic tool management, allowing:
/// - Runtime addition of MCP and Skill tools
/// - Runtime removal of tools
/// - All operations are thread-safe and async
pub struct RigAgentManager {
    config: RigAgentConfig,
    memory_store: Option<Arc<RwLock<MemoryStore>>>,
    /// Tool server handle for hot-reload support
    tool_server_handle: ToolServerHandle,
    /// Names of currently registered tools (for tracking)
    registered_tools: Arc<RwLock<Vec<String>>>,
}

/// Built-in tool names
const BUILTIN_TOOLS: &[&str] = &["search", "web_fetch", "youtube"];

/// Create a tool server with built-in tools
fn create_builtin_tool_server() -> ToolServer {
    ToolServer::new()
        .tool(SearchTool::new())
        .tool(WebFetchTool::new())
        .tool(YouTubeTool::new())
}

/// Create initial registered tools list
fn create_builtin_tools_list() -> Vec<String> {
    BUILTIN_TOOLS.iter().map(|s| s.to_string()).collect()
}

impl RigAgentManager {
    /// Create a new RigAgentManager with built-in tools
    ///
    /// Built-in tools (search, web_fetch, youtube) are registered automatically.
    pub fn new(config: RigAgentConfig) -> Self {
        let tool_server_handle = create_builtin_tool_server().run();
        let registered_tools = Arc::new(RwLock::new(create_builtin_tools_list()));

        Self {
            config,
            memory_store: None,
            tool_server_handle,
            registered_tools,
        }
    }

    /// Create a new RigAgentManager with a shared ToolServerHandle
    ///
    /// This constructor allows sharing a ToolServerHandle across multiple
    /// manager instances, enabling hot-reload of tools.
    ///
    /// # Arguments
    /// * `config` - Agent configuration
    /// * `tool_server_handle` - Shared ToolServerHandle for tool management
    /// * `registered_tools` - Shared list of registered tool names
    pub fn with_shared_handle(
        config: RigAgentConfig,
        tool_server_handle: ToolServerHandle,
        registered_tools: Arc<RwLock<Vec<String>>>,
    ) -> Self {
        Self {
            config,
            memory_store: None,
            tool_server_handle,
            registered_tools,
        }
    }

    /// Create a shared ToolServerHandle with built-in tools
    ///
    /// This method creates a ToolServer with built-in tools and returns
    /// the handle along with the list of registered tool names.
    /// Use this to create a shared handle that can be passed to multiple
    /// RigAgentManager instances via `with_shared_handle()`.
    pub fn create_shared_handle() -> (ToolServerHandle, Arc<RwLock<Vec<String>>>) {
        let tool_server_handle = create_builtin_tool_server().run();
        let registered_tools = Arc::new(RwLock::new(create_builtin_tools_list()));
        (tool_server_handle, registered_tools)
    }

    /// Add a memory store to the manager (builder pattern)
    pub fn with_memory(mut self, store: Arc<RwLock<MemoryStore>>) -> Self {
        self.memory_store = Some(store);
        self
    }

    /// Get the current configuration
    pub fn config(&self) -> &RigAgentConfig {
        &self.config
    }

    /// Check if memory store is configured
    pub fn has_memory(&self) -> bool {
        self.memory_store.is_some()
    }

    /// Get the tool server handle for external use
    pub fn tool_server_handle(&self) -> &ToolServerHandle {
        &self.tool_server_handle
    }

    // ========================================================================
    // HOT-RELOAD: Dynamic Tool Management
    // ========================================================================

    /// Add a tool dynamically (hot-reload)
    ///
    /// This method can be called at runtime to add new tools (MCP, Skills, etc.)
    /// without restarting the agent.
    ///
    /// # Arguments
    /// * `tool` - The tool to add (must implement rig::tool::Tool)
    ///
    /// # Returns
    /// * `Ok(())` - Tool added successfully
    /// * `Err` - Failed to add tool
    pub async fn add_tool(&self, tool: impl ToolDyn + 'static) -> Result<()> {
        let tool_name = tool.name();
        info!(tool_name = %tool_name, "Adding tool dynamically");

        self.tool_server_handle
            .add_tool(tool)
            .await
            .map_err(|e| AetherError::tool(format!("Failed to add tool '{}': {}", tool_name, e)))?;

        // Track the tool name
        let mut tools = self.registered_tools.write().unwrap();
        if !tools.contains(&tool_name) {
            tools.push(tool_name.clone());
        }

        info!(tool_name = %tool_name, "Tool added successfully");
        Ok(())
    }

    /// Add multiple tools dynamically (hot-reload)
    ///
    /// More efficient than calling `add_tool` multiple times.
    pub async fn add_tools(&self, toolset: ToolSet) -> Result<()> {
        info!("Adding toolset dynamically");

        self.tool_server_handle
            .append_toolset(toolset)
            .await
            .map_err(|e| AetherError::tool(format!("Failed to add toolset: {}", e)))?;

        info!("Toolset added successfully");
        Ok(())
    }

    /// Remove a tool dynamically (hot-reload)
    ///
    /// This method can be called at runtime to remove tools.
    ///
    /// # Arguments
    /// * `tool_name` - Name of the tool to remove
    ///
    /// # Returns
    /// * `Ok(())` - Tool removed successfully
    /// * `Err` - Failed to remove tool
    pub async fn remove_tool(&self, tool_name: &str) -> Result<()> {
        info!(tool_name = %tool_name, "Removing tool dynamically");

        self.tool_server_handle
            .remove_tool(tool_name)
            .await
            .map_err(|e| {
                AetherError::tool(format!("Failed to remove tool '{}': {}", tool_name, e))
            })?;

        // Update tracking
        let mut tools = self.registered_tools.write().unwrap();
        tools.retain(|t| t != tool_name);

        info!(tool_name = %tool_name, "Tool removed successfully");
        Ok(())
    }

    /// Get list of currently registered tool names
    pub async fn list_tools(&self) -> Vec<String> {
        self.registered_tools.read().unwrap().clone()
    }

    /// Check if a tool is registered
    pub async fn has_tool(&self, tool_name: &str) -> bool {
        self.registered_tools.read().unwrap().contains(&tool_name.to_string())
    }

    // ========================================================================
    // REQUEST PROCESSING
    // ========================================================================

    /// Process an input and return a response
    ///
    /// Builds a rig-core agent based on the configured provider and model,
    /// using the shared ToolServerHandle for all tools (built-in + dynamic).
    pub async fn process(&self, input: &str) -> Result<AgentResponse> {
        info!(input_len = input.len(), "Processing input");
        debug!(
            provider = %self.config.provider,
            model = %self.config.model,
            "Using config with tool server"
        );

        // Build agent based on provider type and call prompt
        // Use multi_turn() to allow tool calling loops (prevents MaxDepthError)
        let response = match self.config.provider.as_str() {
            "openai" | "gpt" => {
                let client = self.create_openai_client()?;
                let agent = client
                    .agent(&self.config.model)
                    .preamble(&self.config.system_prompt)
                    .temperature(self.config.temperature as f64)
                    .max_tokens(self.config.max_tokens as u64)
                    .tool_server_handle(self.tool_server_handle.clone())
                    .build();
                agent
                    .prompt(input)
                    .multi_turn(self.config.max_turns)
                    .await
                    .map_err(|e| AetherError::provider(format!("OpenAI error: {}", e)))?
            }
            "anthropic" | "claude" => {
                let client = self.create_anthropic_client()?;
                let agent = client
                    .agent(&self.config.model)
                    .preamble(&self.config.system_prompt)
                    .temperature(self.config.temperature as f64)
                    .max_tokens(self.config.max_tokens as u64)
                    .tool_server_handle(self.tool_server_handle.clone())
                    .build();
                agent
                    .prompt(input)
                    .multi_turn(self.config.max_turns)
                    .await
                    .map_err(|e| AetherError::provider(format!("Anthropic error: {}", e)))?
            }
            _ => {
                // For unknown providers, use OpenAI-compatible client with custom base_url
                let client = self.create_custom_client()?;
                let agent = client
                    .agent(&self.config.model)
                    .preamble(&self.config.system_prompt)
                    .temperature(self.config.temperature as f64)
                    .max_tokens(self.config.max_tokens as u64)
                    .tool_server_handle(self.tool_server_handle.clone())
                    .build();
                agent
                    .prompt(input)
                    .multi_turn(self.config.max_turns)
                    .await
                    .map_err(|e| AetherError::provider(format!("Provider error: {}", e)))?
            }
        };

        info!(response_len = response.len(), "Response received");
        Ok(AgentResponse::simple(response))
    }

    /// Process an input with attachments and return a response
    ///
    /// Supports multimodal content (images) via rig-core's native Message API.
    /// Falls back to text-only process() if no attachments are provided.
    pub async fn process_with_attachments(
        &self,
        input: &str,
        attachments: Option<&[MediaAttachment]>,
    ) -> Result<AgentResponse> {
        // If no attachments, delegate to existing process()
        if attachments.map_or(true, |a| a.is_empty()) {
            return self.process(input).await;
        }

        let attachments = attachments.unwrap();
        info!(
            input_len = input.len(),
            attachment_count = attachments.len(),
            "Processing multimodal input"
        );
        debug!(
            provider = %self.config.provider,
            model = %self.config.model,
            "Using config with tool server for multimodal"
        );

        // Build multimodal message
        let message = self.build_multimodal_message(input, attachments);

        // Use agent.prompt(message) - Message implements Into<Message>
        // Use multi_turn() to allow tool calling loops (prevents MaxDepthError)
        let response = match self.config.provider.as_str() {
            "openai" | "gpt" => {
                let client = self.create_openai_client()?;
                let agent = client
                    .agent(&self.config.model)
                    .preamble(&self.config.system_prompt)
                    .temperature(self.config.temperature as f64)
                    .max_tokens(self.config.max_tokens as u64)
                    .tool_server_handle(self.tool_server_handle.clone())
                    .build();
                agent
                    .prompt(message)
                    .multi_turn(self.config.max_turns)
                    .await
                    .map_err(|e| AetherError::provider(format!("OpenAI error: {}", e)))?
            }
            "anthropic" | "claude" => {
                let client = self.create_anthropic_client()?;
                let agent = client
                    .agent(&self.config.model)
                    .preamble(&self.config.system_prompt)
                    .temperature(self.config.temperature as f64)
                    .max_tokens(self.config.max_tokens as u64)
                    .tool_server_handle(self.tool_server_handle.clone())
                    .build();
                agent
                    .prompt(message)
                    .multi_turn(self.config.max_turns)
                    .await
                    .map_err(|e| AetherError::provider(format!("Anthropic error: {}", e)))?
            }
            _ => {
                // For unknown providers, use OpenAI-compatible client with custom base_url
                let client = self.create_custom_client()?;
                let agent = client
                    .agent(&self.config.model)
                    .preamble(&self.config.system_prompt)
                    .temperature(self.config.temperature as f64)
                    .max_tokens(self.config.max_tokens as u64)
                    .tool_server_handle(self.tool_server_handle.clone())
                    .build();
                agent
                    .prompt(message)
                    .multi_turn(self.config.max_turns)
                    .await
                    .map_err(|e| AetherError::provider(format!("Provider error: {}", e)))?
            }
        };

        info!(response_len = response.len(), "Multimodal response received");
        Ok(AgentResponse::simple(response))
    }

    /// Build a multimodal Message from text input and attachments
    ///
    /// Handles both image and document attachments based on their encoding:
    /// - encoding == "base64": Binary content (images) - sent as Image content
    /// - encoding == "utf8": Text content (documents) - sent as Text content with header
    fn build_multimodal_message(&self, input: &str, attachments: &[MediaAttachment]) -> Message {
        let mut content_items: Vec<UserContent> = Vec::new();

        // Add text content first (even if empty, to have at least one item)
        content_items.push(UserContent::Text(Text {
            text: if input.is_empty() {
                "Describe this content in detail.".to_string()
            } else {
                input.to_string()
            },
        }));

        // Process attachments based on encoding
        for attachment in attachments {
            match attachment.encoding.as_str() {
                "base64" => {
                    // Binary content (images) - only process if media_type is image
                    if attachment.media_type == "image" {
                        let media_type = match attachment.mime_type.as_str() {
                            "image/png" => Some(ImageMediaType::PNG),
                            "image/jpeg" => Some(ImageMediaType::JPEG),
                            "image/gif" => Some(ImageMediaType::GIF),
                            "image/webp" => Some(ImageMediaType::WEBP),
                            _ => None,
                        };
                        content_items.push(UserContent::Image(Image {
                            data: DocumentSourceKind::base64(&attachment.data),
                            media_type,
                            detail: None,
                            additional_params: None,
                        }));
                    }
                }
                "utf8" => {
                    // Text content (documents) - add as text block with header
                    let filename = attachment.filename.as_deref().unwrap_or("document");
                    let doc_content = format!(
                        "\n\n--- {} ---\n{}",
                        filename,
                        attachment.data
                    );
                    content_items.push(UserContent::Text(Text {
                        text: doc_content,
                    }));
                }
                _ => {
                    // Unknown encoding - log and skip
                    warn!(
                        encoding = %attachment.encoding,
                        media_type = %attachment.media_type,
                        "Unknown attachment encoding, skipping"
                    );
                }
            }
        }

        // Build Message with OneOrMany (guaranteed non-empty due to text above)
        Message::User {
            content: OneOrMany::many(content_items).expect("content_items is guaranteed non-empty"),
        }
    }

    /// Create OpenAI client
    fn create_openai_client(&self) -> Result<openai::Client> {
        let api_key = self
            .config
            .api_key
            .as_deref()
            .ok_or_else(|| AetherError::provider("OpenAI API key not configured"))?;

        if let Some(ref base_url) = self.config.base_url {
            openai::Client::builder()
                .api_key(api_key)
                .base_url(base_url)
                .build()
                .map_err(|e| AetherError::provider(format!("Failed to create OpenAI client: {}", e)))
        } else {
            openai::Client::new(api_key)
                .map_err(|e| AetherError::provider(format!("Failed to create OpenAI client: {}", e)))
        }
    }

    /// Create Anthropic client
    fn create_anthropic_client(&self) -> Result<anthropic::Client> {
        let api_key = self
            .config
            .api_key
            .as_deref()
            .ok_or_else(|| AetherError::provider("Anthropic API key not configured"))?;

        if let Some(ref base_url) = self.config.base_url {
            anthropic::Client::builder()
                .api_key(api_key)
                .base_url(base_url)
                .build()
                .map_err(|e| {
                    AetherError::provider(format!("Failed to create Anthropic client: {}", e))
                })
        } else {
            anthropic::Client::new(api_key)
                .map_err(|e| {
                    AetherError::provider(format!("Failed to create Anthropic client: {}", e))
                })
        }
    }

    /// Create custom OpenAI-compatible client
    fn create_custom_client(&self) -> Result<openai::Client> {
        let api_key = self
            .config
            .api_key
            .as_deref()
            .ok_or_else(|| AetherError::provider("API key not configured for provider"))?;

        let base_url = self.config.base_url.as_deref().ok_or_else(|| {
            AetherError::provider(format!(
                "base_url required for provider '{}'. Please configure it in your settings.",
                self.config.provider
            ))
        })?;

        openai::Client::builder()
            .api_key(api_key)
            .base_url(base_url)
            .build()
            .map_err(|e| AetherError::provider(format!("Failed to create client: {}", e)))
    }

    /// Process an input with streaming callback
    ///
    /// Calls the callback for each chunk of the response.
    /// This is a placeholder that simulates streaming by splitting the response.
    pub async fn process_stream<F>(&self, input: &str, on_chunk: F) -> Result<AgentResponse>
    where
        F: Fn(&str) + Send + Sync,
    {
        info!(input_len = input.len(), "Processing input with streaming");

        // Get the full response first
        let response = self.process(input).await?;

        // Simulate streaming by splitting on whitespace
        let words: Vec<&str> = response.content.split_whitespace().collect();
        for (i, word) in words.iter().enumerate() {
            if i > 0 {
                on_chunk(" ");
            }
            on_chunk(word);
        }

        debug!(chunks = words.len(), "Streaming completed");
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_manager_creation() {
        let config = RigAgentConfig::default();
        let manager = RigAgentManager::new(config);
        assert_eq!(manager.config().provider, "openai");
        assert!(!manager.has_memory());
    }

    #[tokio::test]
    async fn test_manager_has_builtin_tools() {
        let config = RigAgentConfig::default();
        let manager = RigAgentManager::new(config);

        let tools = manager.list_tools().await;
        assert!(tools.contains(&"search".to_string()));
        assert!(tools.contains(&"web_fetch".to_string()));
        assert!(tools.contains(&"youtube".to_string()));
    }

    #[tokio::test]
    async fn test_manager_process_requires_api_key() {
        // Test that process() fails gracefully when no API key is configured
        let config = RigAgentConfig::default();
        let manager = RigAgentManager::new(config);

        let result = manager.process("Hello, world!").await;

        // Should fail because no API key is configured
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("API key"),
            "Error should mention API key: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_manager_process_anthropic_requires_api_key() {
        // Test that Anthropic provider also requires API key
        let mut config = RigAgentConfig::default();
        config.provider = "anthropic".to_string();
        config.model = "claude-3-5-sonnet-20241022".to_string();
        let manager = RigAgentManager::new(config);

        let result = manager.process("Hello!").await;

        // Should fail because no API key is configured
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("API key"),
            "Error should mention API key: {}",
            err
        );
    }

    #[test]
    fn test_agent_response_creation() {
        let response = AgentResponse::new(
            "Hello".to_string(),
            vec!["tool1".to_string(), "tool2".to_string()],
        );
        assert_eq!(response.content, "Hello");
        assert_eq!(response.tools_called.len(), 2);

        let simple = AgentResponse::simple("Simple".to_string());
        assert_eq!(simple.content, "Simple");
        assert!(simple.tools_called.is_empty());
    }

    #[tokio::test]
    async fn test_manager_with_memory() {
        use crate::store::MemoryStore;
        use tempfile::tempdir;

        let config = RigAgentConfig::default();

        // Create a temporary directory for the test database
        let temp_dir = tempdir().unwrap();
        let db_path = temp_dir.path().join("test_memory.db");
        let store = MemoryStore::new(db_path.to_str().unwrap()).await.unwrap();
        let manager = RigAgentManager::new(config).with_memory(Arc::new(RwLock::new(store)));

        assert!(manager.has_memory());
    }

    #[tokio::test]
    async fn test_config_access() {
        let mut config = RigAgentConfig::default();
        config.temperature = 0.5;
        config.max_tokens = 2048;
        config.system_prompt = "Custom prompt".to_string();

        let manager = RigAgentManager::new(config);

        assert_eq!(manager.config().temperature, 0.5);
        assert_eq!(manager.config().max_tokens, 2048);
        assert_eq!(manager.config().system_prompt, "Custom prompt");
    }
}
