//! Core RigAgentManager implementation

use super::clients::{create_anthropic_client, create_custom_client, create_openai_client};
use super::multimodal::build_multimodal_message;
use super::response::AgentResponse;
use super::tools::{create_builtin_tool_server, create_builtin_tools_list, BuiltinToolConfig};
use crate::agents::rig::config::RigAgentConfig;
use crate::core::MediaAttachment;
use crate::error::{AetherError, Result};
use rig::client::CompletionClient;
use rig::completion::{Message, Prompt};
use rig::tool::server::ToolServerHandle;
use rig::tool::{ToolDyn, ToolSet};
use std::sync::{Arc, RwLock};
use tracing::{debug, info};

/// Manages the rig Agent lifecycle with hot-reload support
///
/// Uses `ToolServerHandle` for dynamic tool management, allowing:
/// - Runtime addition of MCP and Skill tools
/// - Runtime removal of tools
/// - All operations are thread-safe and async
pub struct RigAgentManager {
    config: RigAgentConfig,
    /// Tool server handle for hot-reload support
    tool_server_handle: ToolServerHandle,
    /// Names of currently registered tools (for tracking)
    registered_tools: Arc<RwLock<Vec<String>>>,
}

impl RigAgentManager {
    /// Create a new RigAgentManager with built-in tools
    ///
    /// Built-in tools (search, web_fetch, youtube) are registered automatically.
    pub fn new(config: RigAgentConfig) -> Self {
        let tool_server_handle = create_builtin_tool_server(None).run();
        let registered_tools = Arc::new(RwLock::new(create_builtin_tools_list()));

        Self {
            config,
            tool_server_handle,
            registered_tools,
        }
    }

    /// Create a new RigAgentManager with built-in tools and custom tool configuration
    ///
    /// Built-in tools are configured with the provided BuiltinToolConfig.
    pub fn new_with_tool_config(config: RigAgentConfig, tool_config: BuiltinToolConfig) -> Self {
        let tool_server_handle = create_builtin_tool_server(Some(&tool_config)).run();
        let registered_tools = Arc::new(RwLock::new(create_builtin_tools_list()));

        Self {
            config,
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
        let tool_server_handle = create_builtin_tool_server(None).run();
        let registered_tools = Arc::new(RwLock::new(create_builtin_tools_list()));
        (tool_server_handle, registered_tools)
    }

    /// Create a shared ToolServerHandle with built-in tools and custom configuration
    ///
    /// This method creates a ToolServer with configured built-in tools and returns
    /// the handle along with the list of registered tool names.
    /// Use this to create a shared handle that can be passed to multiple
    /// RigAgentManager instances via `with_shared_handle()`.
    pub fn create_shared_handle_with_config(
        tool_config: BuiltinToolConfig,
    ) -> (ToolServerHandle, Arc<RwLock<Vec<String>>>) {
        let tool_server_handle = create_builtin_tool_server(Some(&tool_config)).run();
        let registered_tools = Arc::new(RwLock::new(create_builtin_tools_list()));
        (tool_server_handle, registered_tools)
    }

    /// Get the current configuration
    pub fn config(&self) -> &RigAgentConfig {
        &self.config
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
        self.registered_tools
            .read()
            .unwrap()
            .contains(&tool_name.to_string())
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
                let client = create_openai_client(&self.config)?;
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
                let client = create_anthropic_client(&self.config)?;
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
                // Use completions_api() for legacy /chat/completions endpoint
                // (most OpenAI-compatible proxies don't support the new /responses endpoint)
                let client = create_custom_client(&self.config)?;
                let agent = client
                    .completion_model(&self.config.model)
                    .completions_api()
                    .into_agent_builder()
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

    /// Process an input with conversation history
    ///
    /// Uses rig-core's `.chat()` or `.with_history()` to maintain multi-turn conversation.
    /// The history is passed in and the response is returned for the caller to store.
    ///
    /// # Arguments
    /// * `input` - Current user input
    /// * `history` - Previous conversation messages (will be mutated to add current exchange)
    ///
    /// # Returns
    /// * `Ok(AgentResponse)` - Response with the assistant's message
    pub async fn process_with_history(
        &self,
        input: &str,
        history: &mut Vec<Message>,
    ) -> Result<AgentResponse> {
        info!(
            input_len = input.len(),
            history_len = history.len(),
            "Processing input with history"
        );
        debug!(
            provider = %self.config.provider,
            model = %self.config.model,
            "Using config with tool server and history"
        );

        // Build agent and process with history
        let response = match self.config.provider.as_str() {
            "openai" | "gpt" => {
                let client = create_openai_client(&self.config)?;
                let agent = client
                    .agent(&self.config.model)
                    .preamble(&self.config.system_prompt)
                    .temperature(self.config.temperature as f64)
                    .max_tokens(self.config.max_tokens as u64)
                    .tool_server_handle(self.tool_server_handle.clone())
                    .build();
                agent
                    .prompt(input)
                    .with_history(history)
                    .multi_turn(self.config.max_turns)
                    .await
                    .map_err(|e| AetherError::provider(format!("OpenAI error: {}", e)))?
            }
            "anthropic" | "claude" => {
                let client = create_anthropic_client(&self.config)?;
                let agent = client
                    .agent(&self.config.model)
                    .preamble(&self.config.system_prompt)
                    .temperature(self.config.temperature as f64)
                    .max_tokens(self.config.max_tokens as u64)
                    .tool_server_handle(self.tool_server_handle.clone())
                    .build();
                agent
                    .prompt(input)
                    .with_history(history)
                    .multi_turn(self.config.max_turns)
                    .await
                    .map_err(|e| AetherError::provider(format!("Anthropic error: {}", e)))?
            }
            _ => {
                // For unknown providers, use OpenAI-compatible client with custom base_url
                // Use completions_api() for legacy /chat/completions endpoint
                let client = create_custom_client(&self.config)?;
                let agent = client
                    .completion_model(&self.config.model)
                    .completions_api()
                    .into_agent_builder()
                    .preamble(&self.config.system_prompt)
                    .temperature(self.config.temperature as f64)
                    .max_tokens(self.config.max_tokens as u64)
                    .tool_server_handle(self.tool_server_handle.clone())
                    .build();
                agent
                    .prompt(input)
                    .with_history(history)
                    .multi_turn(self.config.max_turns)
                    .await
                    .map_err(|e| AetherError::provider(format!("Provider error: {}", e)))?
            }
        };

        info!(
            response_len = response.len(),
            "Response with history received"
        );
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
        if attachments.is_none_or(|a| a.is_empty()) {
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
        let message = build_multimodal_message(input, attachments);

        // Use agent.prompt(message) - Message implements Into<Message>
        // Use multi_turn() to allow tool calling loops (prevents MaxDepthError)
        let response = match self.config.provider.as_str() {
            "openai" | "gpt" => {
                let client = create_openai_client(&self.config)?;
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
                let client = create_anthropic_client(&self.config)?;
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
                // Use completions_api() for legacy /chat/completions endpoint
                let client = create_custom_client(&self.config)?;
                let agent = client
                    .completion_model(&self.config.model)
                    .completions_api()
                    .into_agent_builder()
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

        info!(
            response_len = response.len(),
            "Multimodal response received"
        );
        Ok(AgentResponse::simple(response))
    }

    /// Process an input with both conversation history and attachments
    ///
    /// Combines multi-turn conversation support with multimodal content (images).
    /// This is the recommended method for chat interfaces that support image uploads.
    ///
    /// # Arguments
    /// * `input` - Current user input text
    /// * `history` - Previous conversation messages (will be mutated to add current exchange)
    /// * `attachments` - Optional media attachments (images, documents)
    ///
    /// # Returns
    /// * `Ok(AgentResponse)` - Response with the assistant's message
    pub async fn process_with_history_and_attachments(
        &self,
        input: &str,
        history: &mut Vec<Message>,
        attachments: Option<&[MediaAttachment]>,
    ) -> Result<AgentResponse> {
        // If no attachments, delegate to existing process_with_history()
        if attachments.is_none_or(|a| a.is_empty()) {
            return self.process_with_history(input, history).await;
        }

        let attachments = attachments.unwrap();
        info!(
            input_len = input.len(),
            history_len = history.len(),
            attachment_count = attachments.len(),
            "Processing multimodal input with history"
        );
        debug!(
            provider = %self.config.provider,
            model = %self.config.model,
            "Using config with tool server for multimodal + history"
        );

        // Build multimodal message
        let message = build_multimodal_message(input, attachments);

        // Use agent.prompt(message).with_history(history) for multimodal + multi-turn
        let response = match self.config.provider.as_str() {
            "openai" | "gpt" => {
                let client = create_openai_client(&self.config)?;
                let agent = client
                    .agent(&self.config.model)
                    .preamble(&self.config.system_prompt)
                    .temperature(self.config.temperature as f64)
                    .max_tokens(self.config.max_tokens as u64)
                    .tool_server_handle(self.tool_server_handle.clone())
                    .build();
                agent
                    .prompt(message)
                    .with_history(history)
                    .multi_turn(self.config.max_turns)
                    .await
                    .map_err(|e| AetherError::provider(format!("OpenAI error: {}", e)))?
            }
            "anthropic" | "claude" => {
                let client = create_anthropic_client(&self.config)?;
                let agent = client
                    .agent(&self.config.model)
                    .preamble(&self.config.system_prompt)
                    .temperature(self.config.temperature as f64)
                    .max_tokens(self.config.max_tokens as u64)
                    .tool_server_handle(self.tool_server_handle.clone())
                    .build();
                agent
                    .prompt(message)
                    .with_history(history)
                    .multi_turn(self.config.max_turns)
                    .await
                    .map_err(|e| AetherError::provider(format!("Anthropic error: {}", e)))?
            }
            _ => {
                // For unknown providers, use OpenAI-compatible client with custom base_url
                let client = create_custom_client(&self.config)?;
                let agent = client
                    .completion_model(&self.config.model)
                    .completions_api()
                    .into_agent_builder()
                    .preamble(&self.config.system_prompt)
                    .temperature(self.config.temperature as f64)
                    .max_tokens(self.config.max_tokens as u64)
                    .tool_server_handle(self.tool_server_handle.clone())
                    .build();
                agent
                    .prompt(message)
                    .with_history(history)
                    .multi_turn(self.config.max_turns)
                    .await
                    .map_err(|e| AetherError::provider(format!("Provider error: {}", e)))?
            }
        };

        info!(
            response_len = response.len(),
            "Multimodal + history response received"
        );
        Ok(AgentResponse::simple(response))
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
