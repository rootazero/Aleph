//! Tool Index module for semantic tool retrieval
//!
//! This module provides Tool-as-Resource functionality:
//! - Configuration for retrieval thresholds
//! - Semantic purpose inference (L0/L1)
//! - Tool facts coordination with Memory system
//! - Dual-threshold retrieval with Pre-flight Hydration

mod config;
mod coordinator;
mod inference;

pub use config::ToolRetrievalConfig;
pub use coordinator::{ToolIndexCoordinator, ToolMeta};
pub use inference::{InferredPurpose, SemanticPurposeInferrer};
