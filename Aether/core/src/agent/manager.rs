//! Rig Agent Manager - core entry point
//!
//! This module provides the main manager for rig-based agent operations,
//! including request processing and streaming capabilities.

use super::config::RigAgentConfig;
use crate::error::Result;
use crate::store::MemoryStore;
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
}

impl RigAgentManager {
    /// Create a new RigAgentManager
    pub fn new(config: RigAgentConfig) -> Self {
        Self {
            config,
            memory_store: None,
        }
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

    /// Process an input and return a response
    ///
    /// This is a placeholder implementation that will be expanded
    /// to integrate with rig-core's agent functionality.
    pub async fn process(&self, input: &str) -> Result<AgentResponse> {
        info!(input_len = input.len(), "Processing input");
        debug!(provider = %self.config.provider, model = %self.config.model, "Using config");

        // Placeholder implementation - will be replaced with rig-core integration
        let response_content = format!(
            "[Placeholder] Processing: '{}' with provider: {}, model: {}",
            input, self.config.provider, self.config.model
        );

        Ok(AgentResponse::simple(response_content))
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
    async fn test_manager_process_placeholder() {
        let config = RigAgentConfig::default();
        let manager = RigAgentManager::new(config);

        let response = manager.process("Hello, world!").await.unwrap();

        // Check that response contains the input and config info
        assert!(response.content.contains("Hello, world!"));
        assert!(response.content.contains("openai"));
        assert!(response.content.contains("gpt-4o"));
        assert!(response.tools_called.is_empty());
    }

    #[tokio::test]
    async fn test_manager_process_stream() {
        let config = RigAgentConfig::default();
        let manager = RigAgentManager::new(config);

        let chunks = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let chunks_clone = chunks.clone();

        let response = manager
            .process_stream("Test input", move |chunk| {
                chunks_clone.lock().unwrap().push(chunk.to_string());
            })
            .await
            .unwrap();

        // Verify chunks were collected
        let collected_chunks = chunks.lock().unwrap();
        assert!(!collected_chunks.is_empty());

        // Verify chunks reconstruct the response
        let reconstructed: String = collected_chunks.join("");
        assert_eq!(reconstructed, response.content);
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
}
