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

mod clients;
mod core;
mod multimodal;
mod response;
mod tools;

// Re-export public types for backward compatibility
pub use self::core::RigAgentManager;
pub use response::AgentResponse;
pub use tools::BuiltinToolConfig;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::rig::config::RigAgentConfig;

    #[tokio::test]
    async fn test_manager_creation() {
        let config = RigAgentConfig::default();
        let manager = RigAgentManager::new(config);
        assert_eq!(manager.config().provider, "openai");
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
