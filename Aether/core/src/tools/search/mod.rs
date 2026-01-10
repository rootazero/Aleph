//! Web Search Tools Module
//!
//! Provides native AgentTool implementations for web search.
//!
//! # Available Tools
//!
//! | Tool | Description | Confirmation |
//! |------|-------------|--------------|
//! | `web_search` | Search the web | No |
//!
//! # Design Note
//!
//! This tool wraps the existing SearchRegistry and provides a simplified
//! interface for LLM function calling. PII scrubbing is applied to queries
//! before sending to external search APIs.
//!
//! # Usage
//!
//! ```rust,ignore
//! use aether_core::tools::search::{SearchContext, WebSearchTool, SearchConfig};
//! use aether_core::tools::NativeToolRegistry;
//! use std::sync::Arc;
//!
//! // Create context
//! let config = SearchConfig::default();
//! let ctx = SearchContext::new(config);
//!
//! // Set up registry (after SearchRegistry is initialized)
//! ctx.set_registry(search_registry).await;
//!
//! // Register tool
//! let registry = NativeToolRegistry::new();
//! registry.register(Arc::new(WebSearchTool::new(ctx))).await;
//!
//! // Execute
//! let result = registry.execute("web_search", r#"{"query": "rust async"}"#).await?;
//! ```

mod web;

pub use web::{SearchConfig, SearchContext, WebSearchTool};

use std::sync::Arc;

use super::AgentTool;

/// Create all search tools with shared context
///
/// Convenience function to create all search tools at once.
///
/// # Arguments
///
/// * `config` - Search configuration
///
/// # Returns
///
/// Tuple of (tools, context) - context is returned so caller can set the registry.
/// Note: Returns empty vector if search is disabled in config.
pub fn create_all_tools(config: SearchConfig) -> (Vec<Arc<dyn AgentTool>>, SearchContext) {
    let ctx = SearchContext::new(config.clone());

    if !config.enabled {
        return (vec![], ctx);
    }

    let tools: Vec<Arc<dyn AgentTool>> = vec![Arc::new(WebSearchTool::new(ctx.clone()))];
    (tools, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_all_tools_enabled() {
        let config = SearchConfig::default();
        let (tools, _ctx) = create_all_tools(config);

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "web_search");
    }

    #[test]
    fn test_create_all_tools_disabled() {
        let config = SearchConfig {
            enabled: false,
            ..Default::default()
        };
        let (tools, _ctx) = create_all_tools(config);

        assert!(tools.is_empty());
    }

    #[test]
    fn test_all_tools_are_read_only() {
        let config = SearchConfig::default();
        let (tools, _ctx) = create_all_tools(config);

        for tool in &tools {
            assert!(
                !tool.requires_confirmation(),
                "{} should not require confirmation",
                tool.name()
            );
        }
    }

    #[test]
    fn test_all_tools_have_search_category() {
        use crate::tools::ToolCategory;

        let config = SearchConfig::default();
        let (tools, _ctx) = create_all_tools(config);

        for tool in &tools {
            assert_eq!(
                tool.category(),
                ToolCategory::Search,
                "{} should have Search category",
                tool.name()
            );
        }
    }
}
