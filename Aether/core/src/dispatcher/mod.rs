//! Dispatcher Layer - Tool Registry and Confirmation
//!
//! This module implements the Dispatcher Layer that provides:
//!
//! - **Unified Tool Registry**: Aggregates all tool sources (Native, MCP, Skills, Custom)
//! - **Confirmation System**: User confirmation for tool execution
//! - **Async Confirmation**: Background confirmation handling
//!
//! # Architecture
//!
//! ```text
//! User Input
//!      ↓
//! ┌─────────────────────┐
//! │   Dispatcher Layer  │
//! │                     │
//! │  ┌───────────────┐  │
//! │  │ ToolRegistry  │  │  ← Aggregates Native/MCP/Skills/Custom
//! │  └───────┬───────┘  │
//! │          ↓          │
//! │  ┌───────────────┐  │
//! │  │ Confirmation  │  │  ← User confirmation if needed
//! │  └───────────────┘  │
//! └──────────┼──────────┘
//!            ↓
//!    rig-core Agent
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use aethecore::dispatcher::{ToolRegistry, UnifiedTool, ToolSource};
//!
//! // Create registry
//! let registry = ToolRegistry::new();
//!
//! // Refresh from all sources
//! registry.refresh_all().await;
//!
//! // Query tools
//! let tools = registry.list_all().await;
//! for tool in tools {
//!     println!("{}: {} [{:?}]", tool.name, tool.description, tool.source);
//! }
//! ```

mod async_confirmation;
mod confirmation;
mod integration;
mod registry;
mod types;

pub use async_confirmation::{
    AsyncConfirmationConfig, AsyncConfirmationHandler, ConfirmationState, PendingConfirmation,
    PendingConfirmationInfo, PendingConfirmationStore, UserConfirmationDecision,
};
pub use confirmation::{
    ConfirmationAction, ConfirmationConfig, ConfirmationDecision, ToolConfirmation, OPTION_CANCEL,
    OPTION_EDIT, OPTION_EXECUTE,
};
pub use integration::{
    ConfidenceAction, ConfidenceThresholds, DispatcherAction, DispatcherConfig,
    DispatcherIntegration, DispatcherResult,
};
pub use registry::ToolRegistry;
pub use types::{
    ConflictInfo, ConflictResolution, RoutingLayer, ToolCategory, ToolDefinition, ToolPriority,
    ToolResult, ToolSafetyLevel, ToolSource, ToolSourceType, UnifiedTool, UnifiedToolInfo,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_source_display() {
        assert_eq!(format!("{:?}", ToolSource::Native), "Native");
        assert_eq!(
            format!(
                "{:?}",
                ToolSource::Mcp {
                    server: "github".into()
                }
            ),
            "Mcp { server: \"github\" }"
        );
    }
}
