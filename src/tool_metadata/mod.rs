//! Tool Metadata Layer
//!
//! This module manages tool registration, discovery, and risk evaluation:
//!
//! - **Tool Registry**: Aggregates all tool sources (Native, MCP, Skills, Custom)
//! - **Risk Evaluation**: Tool risk assessment
//! - **Tool Index**: Semantic tool retrieval and hydration

// === Constants ===
mod constants;
pub use constants::*;

// === Slash-command aliases (single source for execution + discovery) ===
pub mod aliases;
pub use aliases::{is_shorthand_alias, resolve_shorthand, shorthand_aliases_for, SHORTHAND_ALIASES};

// === Tool Management ===
mod registry;
mod types;

// === Risk Evaluation ===
pub mod risk;

// === Re-exports: Tool Management ===
pub use registry::ResolvedCommand;
pub use registry::ToolCatalog;
pub use registry::{HealthReason, HealthSnapshot, ProbeResult, ToolHealthCache, ToolHealthProbe};
pub use types::{
    ChannelType, ConflictInfo, ConflictResolution, DispatchMode, StructuredToolMeta, ToolCategory,
    ToolDefinition, ToolDiff, ToolIndex, ToolIndexCategory, ToolIndexEntry, ToolPriority,
    ToolResult, ToolSafetyLevel, ToolSource, ToolSourceType, UnifiedTool, UnifiedToolInfo,
};

// === Re-exports: Risk Evaluation ===
pub use risk::{RiskEvaluator, RiskLevel};

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
