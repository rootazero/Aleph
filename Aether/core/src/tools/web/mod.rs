//! Web Tools Module
//!
//! Provides native AgentTool implementations for web operations.
//!
//! # Available Tools
//!
//! | Tool | Description | Confirmation |
//! |------|-------------|--------------|
//! | `web_fetch` | Fetch and extract content from a URL | No |
//!
//! # Design Note
//!
//! This module provides tools for fetching and processing web content.
//! The WebFetchTool extracts readable text from HTML pages and converts
//! it to Markdown format for LLM consumption.
//!
//! # Usage
//!
//! ```rust,ignore
//! use aether_core::tools::web::{WebFetchTool, WebFetchConfig};
//! use aether_core::tools::NativeToolRegistry;
//! use std::sync::Arc;
//!
//! // Create tool with config
//! let config = WebFetchConfig::default();
//! let tool = WebFetchTool::new(config);
//!
//! // Register tool
//! let registry = NativeToolRegistry::new();
//! registry.register(Arc::new(tool)).await;
//!
//! // Execute
//! let result = registry.execute("web_fetch", r#"{"url": "https://example.com"}"#).await?;
//! ```

mod fetch;

pub use fetch::{WebFetchConfig, WebFetchTool};

use std::sync::Arc;

use super::AgentTool;

/// Create all web tools
///
/// Convenience function to create all web tools at once.
///
/// # Arguments
///
/// * `config` - Web fetch configuration
///
/// # Returns
///
/// Vector of tools. Returns empty vector if disabled in config.
pub fn create_all_tools(config: WebFetchConfig) -> Vec<Arc<dyn AgentTool>> {
    if !config.enabled {
        return vec![];
    }

    vec![Arc::new(WebFetchTool::new(config))]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_all_tools_enabled() {
        let config = WebFetchConfig::default();
        let tools = create_all_tools(config);

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "web_fetch");
    }

    #[test]
    fn test_create_all_tools_disabled() {
        let config = WebFetchConfig {
            enabled: false,
            ..Default::default()
        };
        let tools = create_all_tools(config);

        assert!(tools.is_empty());
    }

    #[test]
    fn test_all_tools_are_read_only() {
        let config = WebFetchConfig::default();
        let tools = create_all_tools(config);

        for tool in &tools {
            assert!(
                !tool.requires_confirmation(),
                "{} should not require confirmation",
                tool.name()
            );
        }
    }
}
