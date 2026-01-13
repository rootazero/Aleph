//! System Tools Module
//!
//! Provides native AgentTool implementations for system information queries.
//!
//! # Available Tools
//!
//! | Tool | Description | Confirmation |
//! |------|-------------|--------------|
//! | `sys_info` | Get comprehensive system info | No |
//! | `active_app` | Get frontmost application | No |
//! | `active_window` | Get active window title | No |
//!
//! # Usage
//!
//! ```rust,ignore
//! use aether_core::tools::system::{SystemContext, SystemInfoTool};
//! use aether_core::tools::NativeToolRegistry;
//! use std::sync::Arc;
//!
//! // Create context with default provider
//! let ctx = SystemContext::new();
//!
//! // Register tools
//! let registry = NativeToolRegistry::new();
//! registry.register(Arc::new(SystemInfoTool::new(ctx))).await;
//!
//! // Execute
//! let result = registry.execute("sys_info", r#"{}"#).await?;
//! ```

mod info;

pub use info::{SystemContext, SystemInfoTool};

use std::sync::Arc;

use super::AgentTool;

/// Create all system tools with shared context
///
/// Convenience function to create all system tools at once.
///
/// # Returns
///
/// Vector of Arc-wrapped AgentTool implementations
pub fn create_all_tools() -> Vec<Arc<dyn AgentTool>> {
    let ctx = SystemContext::new();
    vec![Arc::new(SystemInfoTool::new(ctx))]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_all_tools() {
        let tools = create_all_tools();

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "sys_info");
    }

    #[test]
    fn test_all_tools_are_read_only() {
        let tools = create_all_tools();

        for tool in &tools {
            assert!(
                !tool.requires_confirmation(),
                "{} should not require confirmation (read-only)",
                tool.name()
            );
        }
    }

    #[test]
    fn test_all_tools_have_builtin_category() {
        use crate::tools::ToolCategory;

        let tools = create_all_tools();

        for tool in &tools {
            assert_eq!(
                tool.category(),
                ToolCategory::Builtin,
                "{} should have Builtin category",
                tool.name()
            );
        }
    }
}
