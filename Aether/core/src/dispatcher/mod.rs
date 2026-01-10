//! Dispatcher Layer - Intelligent Tool Routing
//!
//! This module implements the Dispatcher Layer (Aether Cortex) that provides:
//!
//! - **Unified Tool Registry**: Aggregates all tool sources (Native, MCP, Skills, Custom)
//! - **Multi-Layer Routing**: L1 (regex) → L2 (semantic) → L3 (AI) cascading
//! - **Confidence Scoring**: Returns match confidence for confirmation triggering
//! - **Dynamic Prompt Generation**: Injects tool metadata into L3 router prompts
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
//! │  │ PromptBuilder │  │  ← Dynamic L3 prompt generation
//! │  └───────┬───────┘  │
//! │          ↓          │
//! │  ┌───────────────┐  │
//! │  │ MultiLayer    │  │
//! │  │ Router        │  │  ← L1 → L2 → L3 cascade
//! │  └───────┬───────┘  │
//! │          ↓          │
//! │  ┌───────────────┐  │
//! │  │ ActionResult  │  │  ← tool, params, confidence
//! │  └───────┬───────┘  │
//! │          ↓          │
//! │  ┌───────────────┐  │
//! │  │ Confirmation  │  │  ← If confidence < threshold
//! │  └───────────────┘  │
//! └──────────┼──────────┘
//!            ↓
//!    Execution Layer
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use aethecore::dispatcher::{ToolRegistry, UnifiedTool, ToolSource, PromptBuilder};
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
//!
//! // Build L3 routing prompt
//! let prompt = PromptBuilder::build_l3_routing_prompt(&tools, None);
//! ```

mod async_confirmation;
mod builtin_defs;
mod confirmation;
mod integration;
mod l3_router;
mod prompt_builder;
mod registry;
mod types;

pub use async_confirmation::{
    AsyncConfirmationConfig, AsyncConfirmationHandler, ConfirmationState, PendingConfirmation,
    PendingConfirmationInfo, PendingConfirmationStore, UserConfirmationDecision,
};
pub use builtin_defs::{get_builtin_routing_rules, BuiltinCommandDef, BUILTIN_COMMANDS};
pub use confirmation::{
    ConfirmationAction, ConfirmationConfig, ConfirmationDecision, ToolConfirmation,
    OPTION_CANCEL, OPTION_EDIT, OPTION_EXECUTE,
};
pub use integration::{
    ConfidenceAction, ConfidenceThresholds, DispatcherAction, DispatcherConfig,
    DispatcherIntegration, DispatcherResult,
};
pub use l3_router::{L3Router, L3RoutingOptions, L3RoutingResult};
pub use prompt_builder::{L3RoutingResponse, PromptBuilder, PromptFormat, ToolFilter};
pub use registry::ToolRegistry;
pub use types::{
    ConflictInfo, ConflictResolution, RoutingLayer, ToolPriority, ToolSource, ToolSourceType,
    UnifiedTool, UnifiedToolInfo,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_source_display() {
        assert_eq!(format!("{:?}", ToolSource::Native), "Native");
        assert_eq!(
            format!("{:?}", ToolSource::Mcp { server: "github".into() }),
            "Mcp { server: \"github\" }"
        );
    }
}
