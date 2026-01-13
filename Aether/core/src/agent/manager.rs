//! Rig Agent Manager - core entry point
//!
//! This module provides the main manager for rig-based agent operations,
//! including request processing and streaming capabilities.

use super::config::RigAgentConfig;
use crate::error::{AetherError, Result};
use crate::rig_tools::{SearchTool, WebFetchTool};
use crate::store::MemoryStore;
use rig::completion::Prompt;
use rig::providers::{anthropic, openai};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

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

/// Manages the rig Agent lifecycle
pub struct RigAgentManager {
    config: RigAgentConfig,
    memory_store: Option<Arc<RwLock<MemoryStore>>>,
    search_tool: Option<SearchTool>,
    web_fetch_tool: Option<WebFetchTool>,
}

impl RigAgentManager {
    /// Create a new RigAgentManager
    pub fn new(config: RigAgentConfig) -> Self {
        Self {
            config,
            memory_store: None,
            search_tool: None,
            web_fetch_tool: None,
        }
    }

    /// Add a memory store to the manager (builder pattern)
    pub fn with_memory(mut self, store: Arc<RwLock<MemoryStore>>) -> Self {
        self.memory_store = Some(store);
        self
    }

    /// Add search tool to the manager (builder pattern)
    ///
    /// When enabled, the agent will have access to web search capabilities
    /// via the Tavily API. Requires TAVILY_API_KEY environment variable.
    pub fn with_search_tool(mut self) -> Self {
        self.search_tool = Some(SearchTool::new());
        self
    }

    /// Add web fetch tool to the manager (builder pattern)
    ///
    /// When enabled, the agent will have access to fetch and extract
    /// content from web pages.
    pub fn with_web_fetch_tool(mut self) -> Self {
        self.web_fetch_tool = Some(WebFetchTool::new());
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

    /// Check if search tool is configured
    pub fn has_search_tool(&self) -> bool {
        self.search_tool.is_some()
    }

    /// Check if web fetch tool is configured
    pub fn has_web_fetch_tool(&self) -> bool {
        self.web_fetch_tool.is_some()
    }

    /// Process an input and return a response
    ///
    /// Builds a rig-core agent based on the configured provider and model,
    /// then calls the prompt method to get a response.
    ///
    /// If tools are configured (via `with_search_tool()` or `with_web_fetch_tool()`),
    /// they will be available to the agent during processing.
    pub async fn process(&self, input: &str) -> Result<AgentResponse> {
        info!(input_len = input.len(), "Processing input");
        debug!(
            provider = %self.config.provider,
            model = %self.config.model,
            has_search_tool = self.search_tool.is_some(),
            has_web_fetch_tool = self.web_fetch_tool.is_some(),
            "Using config"
        );

        // Build agent based on provider type and call prompt
        let response = match self.config.provider.as_str() {
            "openai" | "gpt" => {
                let api_key = self
                    .config
                    .api_key
                    .as_deref()
                    .ok_or_else(|| AetherError::provider("OpenAI API key not configured"))?;

                let client = if let Some(ref base_url) = self.config.base_url {
                    openai::Client::from_url(api_key, base_url)
                } else {
                    openai::Client::new(api_key)
                };

                // Build agent with optional tools
                let mut agent_builder = client
                    .agent(&self.config.model)
                    .preamble(&self.config.system_prompt)
                    .temperature(self.config.temperature as f64)
                    .max_tokens(self.config.max_tokens as u64);

                // Add tools if configured
                if let Some(ref search_tool) = self.search_tool {
                    agent_builder = agent_builder.tool(search_tool.clone());
                }
                if let Some(ref web_fetch_tool) = self.web_fetch_tool {
                    agent_builder = agent_builder.tool(web_fetch_tool.clone());
                }

                let agent = agent_builder.build();

                agent
                    .prompt(input)
                    .await
                    .map_err(|e| AetherError::provider(format!("OpenAI error: {}", e)))?
            }
            "anthropic" | "claude" => {
                let api_key = self
                    .config
                    .api_key
                    .as_deref()
                    .ok_or_else(|| AetherError::provider("Anthropic API key not configured"))?;

                // Use ClientBuilder to support custom base_url
                let mut builder = anthropic::ClientBuilder::new(api_key);
                if let Some(ref base_url) = self.config.base_url {
                    builder = builder.base_url(base_url);
                }
                let client = builder.build();

                // Build agent with optional tools
                let mut agent_builder = client
                    .agent(&self.config.model)
                    .preamble(&self.config.system_prompt)
                    .temperature(self.config.temperature as f64)
                    .max_tokens(self.config.max_tokens as u64);

                // Add tools if configured
                if let Some(ref search_tool) = self.search_tool {
                    agent_builder = agent_builder.tool(search_tool.clone());
                }
                if let Some(ref web_fetch_tool) = self.web_fetch_tool {
                    agent_builder = agent_builder.tool(web_fetch_tool.clone());
                }

                let agent = agent_builder.build();

                agent
                    .prompt(input)
                    .await
                    .map_err(|e| AetherError::provider(format!("Anthropic error: {}", e)))?
            }
            _ => {
                // For unknown providers, require explicit configuration
                let api_key = self
                    .config
                    .api_key
                    .as_deref()
                    .ok_or_else(|| AetherError::provider("API key not configured for provider"))?;

                let base_url = self
                    .config
                    .base_url
                    .as_deref()
                    .ok_or_else(|| AetherError::provider(format!(
                        "base_url required for provider '{}'. Please configure it in your settings.",
                        self.config.provider
                    )))?;

                let client = openai::Client::from_url(api_key, base_url);

                // Build agent with optional tools
                let mut agent_builder = client
                    .agent(&self.config.model)
                    .preamble(&self.config.system_prompt)
                    .temperature(self.config.temperature as f64)
                    .max_tokens(self.config.max_tokens as u64);

                // Add tools if configured
                if let Some(ref search_tool) = self.search_tool {
                    agent_builder = agent_builder.tool(search_tool.clone());
                }
                if let Some(ref web_fetch_tool) = self.web_fetch_tool {
                    agent_builder = agent_builder.tool(web_fetch_tool.clone());
                }

                let agent = agent_builder.build();

                agent
                    .prompt(input)
                    .await
                    .map_err(|e| AetherError::provider(format!("Provider error: {}", e)))?
            }
        };

        info!(response_len = response.len(), "Response received");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manager_creation() {
        let config = RigAgentConfig::default();
        let manager = RigAgentManager::new(config);
        assert_eq!(manager.config().provider, "openai");
        assert!(!manager.has_memory());
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
        let manager =
            RigAgentManager::new(config).with_memory(Arc::new(RwLock::new(store)));

        assert!(manager.has_memory());
    }

    #[test]
    fn test_config_access() {
        let mut config = RigAgentConfig::default();
        config.temperature = 0.5;
        config.max_tokens = 2048;
        config.system_prompt = "Custom prompt".to_string();

        let manager = RigAgentManager::new(config);

        assert_eq!(manager.config().temperature, 0.5);
        assert_eq!(manager.config().max_tokens, 2048);
        assert_eq!(manager.config().system_prompt, "Custom prompt");
    }

    #[test]
    fn test_manager_with_search_tool() {
        let config = RigAgentConfig::default();
        let manager = RigAgentManager::new(config).with_search_tool();

        assert!(manager.has_search_tool());
        assert!(!manager.has_web_fetch_tool());
    }

    #[test]
    fn test_manager_with_web_fetch_tool() {
        let config = RigAgentConfig::default();
        let manager = RigAgentManager::new(config).with_web_fetch_tool();

        assert!(!manager.has_search_tool());
        assert!(manager.has_web_fetch_tool());
    }

    #[test]
    fn test_manager_with_both_tools() {
        let config = RigAgentConfig::default();
        let manager = RigAgentManager::new(config)
            .with_search_tool()
            .with_web_fetch_tool();

        assert!(manager.has_search_tool());
        assert!(manager.has_web_fetch_tool());
    }

    #[test]
    fn test_manager_builder_pattern() {
        let config = RigAgentConfig::default();

        // Test that all builder methods can be chained
        let manager = RigAgentManager::new(config)
            .with_search_tool()
            .with_web_fetch_tool();

        assert!(manager.has_search_tool());
        assert!(manager.has_web_fetch_tool());
        assert!(!manager.has_memory());
    }
}
