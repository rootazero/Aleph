//! Preflight cheap passes — token-saving transforms that need no LLM call.
//!
//! Each stage implements [`super::preflight::PreflightStage`]. The harness
//! wires a `PreflightPipeline` of these stages and calls it BEFORE the
//! existing `ContextCompactor`, so token savings happen even when the
//! compactor's side-channel LLM call fails.
//!
//! Borrows the staging idea from hermes-agent: `tool_result` pruning and
//! historical image stripping are cheap deterministic transforms that
//! deliver consistent savings without LLM cost.

pub mod file_op_supersede;
pub mod image_stripping;
pub mod tool_result_pruning;

pub use file_op_supersede::FileOpSupersedeStage;
pub use image_stripping::HistoricalImageStrippingStage;
pub use tool_result_pruning::ToolResultPruningStage;
