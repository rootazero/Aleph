//! Tool Index module for semantic tool retrieval
//!
//! This module provides Tool-as-Resource functionality:
//! - Configuration for retrieval thresholds
//! - Semantic purpose inference (L0/L1)
//! - Tool facts coordination with Memory system
//! - Dual-threshold retrieval with Pre-flight Hydration
//! - Hydration Pipeline for Agent Loop integration
//! - Event listeners for MCP and Skill registry changes

mod config;
mod coordinator;
mod inference;
mod pipeline;
mod retrieval;

#[cfg(test)]
mod tests;

pub use config::ToolRetrievalConfig;
pub use coordinator::{ToolIndexCoordinator, ToolMeta};
pub use inference::{InferredPurpose, SemanticPurposeInferrer};
pub use pipeline::{HydrationPipeline, HydrationPipelineConfig, HydrationResult};
pub use retrieval::{HydrationLevel, HydratedTool, ToolRetrieval};
