//! Dispatcher Layer - Tool Management
//!
//! This module manages tool registration, discovery, and risk evaluation:
//!
//! - **Tool Registry**: Aggregates all tool sources (Native, MCP, Skills, Custom)
//! - **Risk Evaluation**: Tool risk assessment
//! - **Tool Index**: Semantic tool retrieval and hydration

// === Constants ===
mod constants;
pub use constants::*;

// === Tool Management ===
mod registry;
mod types;

// === Risk Evaluation ===
pub mod risk;

// === Tool Index: Semantic tool retrieval ===
pub mod tool_index;

// === Re-exports: Tool Management ===
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
