//! Tool Metadata Layer
//!
//! This module manages tool registration and discovery:
//!
//! - **Tool Registry**: Aggregates all tool sources (Native, MCP, Skills, Custom)
//! - **Tool Index**: Semantic tool retrieval and hydration
//!
//! Note: the legacy `risk::RiskEvaluator` regex classifier was removed — R8 reserves
//! regex for machine formats (JSON / URLs / file globs); intent classification belongs
//! to the LLM via prompt.

mod constants;
pub use constants::*;

pub mod aliases;
pub use aliases::{
    is_shorthand_alias, resolve_shorthand, shorthand_aliases_for, RUNTIME_ONLY_ALIAS_TARGETS,
    SHORTHAND_ALIASES,
};

mod registry;
mod types;

pub use registry::ResolvedCommand;
pub use registry::ToolCatalog;
pub use registry::{HealthReason, HealthSnapshot, ProbeResult, ToolHealthCache, ToolHealthProbe};
pub use types::{
    ChannelType, ConflictInfo, ConflictResolution, DispatchMode, ToolCategory, ToolDefinition,
    ToolPriority, ToolSafetyLevel, ToolSource, ToolSourceType, UnifiedTool,
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
