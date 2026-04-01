//! Dispatcher Layer - Tool Management
//!
//! This module manages tool registration, discovery, and confirmation:
//!
//! - **Tool Registry**: Aggregates all tool sources (Native, MCP, Skills, Custom)
//! - **Confirmation System**: User confirmation for tool execution
//! - **Risk Evaluation**: Tool risk assessment
//! - **Tool Index**: Semantic tool retrieval and hydration

// === Constants ===
mod constants;
pub use constants::*;

// === Tool Management ===
mod async_confirmation;
mod confirmation;
mod integration;
mod registry;
mod types;

// === Risk Evaluation ===
pub mod risk;

// === Tool Index: Semantic tool retrieval ===
pub mod tool_index;

// === Re-exports: Tool Management ===
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
pub use registry::ResolvedCommand;
pub use registry::ToolRegistry;
pub use types::{
    ChannelType, ConflictInfo, ConflictResolution, DispatchMode, RoutingLayer, StructuredToolMeta,
    ToolCategory, ToolDefinition, ToolDiff, ToolIndex, ToolIndexCategory, ToolIndexEntry,
    ToolPriority, ToolResult, ToolSafetyLevel, ToolSource, ToolSourceType, UnifiedTool,
    UnifiedToolInfo,
};

// === Re-exports: Risk Evaluation ===
pub use risk::{RiskEvaluator, RiskLevel};

// === Re-exports: Tool Index (Semantic Retrieval) ===
pub use tool_index::{
    HydratedTool, HydrationLevel, HydrationPipeline, HydrationPipelineConfig, HydrationResult,
    InferredPurpose, SemanticPurposeInferrer, ToolIndexCoordinator, ToolMeta, ToolRetrieval,
    ToolRetrievalConfig,
};

#[cfg(all(test, feature = "loom"))]
mod loom_concurrency;

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
