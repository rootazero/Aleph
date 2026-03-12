//! Intent types — lightweight type definitions retained from the old intent detection module.
//!
//! The detection/classification pipeline has been removed in favor of LLM-native
//! tool selection via the minimal agent loop. Only shared type definitions remain.

pub mod types;

// Re-export type definitions used by other modules
pub use types::{DetectionLayer, DirectToolSource, ExecuteMetadata, IntentResult};
pub use types::TaskCategory;
