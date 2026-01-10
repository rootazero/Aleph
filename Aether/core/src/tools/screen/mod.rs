//! Screen Capture Tools Module
//!
//! Provides native AgentTool implementations for screen and window capture.
//!
//! # Available Tools
//!
//! | Tool | Description | Confirmation |
//! |------|-------------|--------------|
//! | `screen_capture` | Capture screen or window | Yes (always) |
//! | `list_monitors` | List available monitors | No |
//! | `list_windows` | List visible windows | No |
//!
//! # Privacy Note
//!
//! Screen capture is privacy-sensitive and ALWAYS requires user confirmation.
//! Listing monitors and windows does not require confirmation as they only
//! return metadata.
//!
//! # Usage
//!
//! ```rust,ignore
//! use aether_core::tools::screen::{ScreenConfig, ScreenContext, ScreenCaptureTool};
//! use aether_core::tools::NativeToolRegistry;
//! use std::sync::Arc;
//!
//! // Create context
//! let config = ScreenConfig::default();
//! let ctx = ScreenContext::new(config);
//!
//! // Register tools
//! let registry = NativeToolRegistry::new();
//! registry.register(Arc::new(ScreenCaptureTool::new(ctx))).await;
//!
//! // Execute (with confirmation)
//! let result = registry.execute("screen_capture", r#"{"target": "screen"}"#).await?;
//! ```

mod capture;

pub use capture::{ScreenCaptureTool, ScreenConfig, ScreenContext};

use std::sync::Arc;

use super::AgentTool;

/// Create all screen tools with shared context
///
/// Convenience function to create all screen tools at once.
///
/// # Arguments
///
/// * `config` - Screen capture configuration
///
/// # Returns
///
/// Vector of Arc-wrapped AgentTool implementations.
/// Note: Returns an empty vector if screen capture is disabled in config.
pub fn create_all_tools(config: ScreenConfig) -> Vec<Arc<dyn AgentTool>> {
    if !config.enabled {
        return vec![];
    }

    let ctx = ScreenContext::new(config);
    vec![Arc::new(ScreenCaptureTool::new(ctx))]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_all_tools_enabled() {
        let config = ScreenConfig::default(); // enabled by default
        let tools = create_all_tools(config);

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "screen_capture");
    }

    #[test]
    fn test_create_all_tools_disabled() {
        let config = ScreenConfig {
            enabled: false,
            ..Default::default()
        };
        let tools = create_all_tools(config);

        assert!(tools.is_empty());
    }

    #[test]
    fn test_screen_capture_requires_confirmation() {
        let config = ScreenConfig::default();
        let tools = create_all_tools(config);

        for tool in &tools {
            if tool.name() == "screen_capture" {
                assert!(
                    tool.requires_confirmation(),
                    "screen_capture should always require confirmation"
                );
            }
        }
    }

    #[test]
    fn test_all_tools_have_screen_category() {
        use crate::tools::ToolCategory;

        let config = ScreenConfig::default();
        let tools = create_all_tools(config);

        for tool in &tools {
            assert_eq!(
                tool.category(),
                ToolCategory::Screen,
                "{} should have Screen category",
                tool.name()
            );
        }
    }
}
