//! Clipboard Tools Module
//!
//! Provides native AgentTool implementations for clipboard operations.
//! Clipboard content is provided by Swift layer via UniFFI callback.
//!
//! # Available Tools
//!
//! | Tool | Description | Confirmation |
//! |------|-------------|--------------|
//! | `clipboard_read` | Read clipboard content | No |
//!
//! # Design Note
//!
//! The clipboard content is managed externally (by Swift on macOS) and
//! updated via the shared `ClipboardContent` state. This tool only reads
//! the cached content, it does not directly access the system clipboard.
//!
//! # Usage
//!
//! ```rust,ignore
//! use aether_core::tools::clipboard::{ClipboardContext, ClipboardReadTool, ClipboardContent};
//! use aether_core::tools::NativeToolRegistry;
//! use std::sync::Arc;
//!
//! // Create context
//! let ctx = ClipboardContext::new();
//!
//! // Update content (from Swift callback)
//! ctx.update_content(ClipboardContent::text("Hello World".to_string())).await;
//!
//! // Register tool
//! let registry = NativeToolRegistry::new();
//! registry.register(Arc::new(ClipboardReadTool::new(ctx))).await;
//!
//! // Execute
//! let result = registry.execute("clipboard_read", r#"{"format": "text"}"#).await?;
//! ```

mod read;

pub use read::{ClipboardContent, ClipboardContext, ClipboardReadTool};

use std::sync::Arc;

use super::AgentTool;

/// Create all clipboard tools with shared context
///
/// Convenience function to create all clipboard tools at once.
///
/// # Returns
///
/// Vector of Arc-wrapped AgentTool implementations
pub fn create_all_tools() -> (Vec<Arc<dyn AgentTool>>, ClipboardContext) {
    let ctx = ClipboardContext::new();
    let tools: Vec<Arc<dyn AgentTool>> = vec![Arc::new(ClipboardReadTool::new(ctx.clone()))];
    (tools, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_all_tools() {
        let (tools, _ctx) = create_all_tools();

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "clipboard_read");
    }

    #[test]
    fn test_all_tools_are_read_only() {
        let (tools, _ctx) = create_all_tools();

        for tool in &tools {
            assert!(
                !tool.requires_confirmation(),
                "{} should not require confirmation (read-only)",
                tool.name()
            );
        }
    }

    #[test]
    fn test_all_tools_have_clipboard_category() {
        use crate::tools::ToolCategory;

        let (tools, _ctx) = create_all_tools();

        for tool in &tools {
            assert_eq!(
                tool.category(),
                ToolCategory::Native,
                "{} should have Clipboard category",
                tool.name()
            );
        }
    }
}
