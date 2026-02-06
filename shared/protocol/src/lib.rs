//! # Aleph Protocol
//!
//! Pure type definitions for Aleph Server-Client communication.
//!
//! This crate contains only data types with no runtime dependencies,
//! making it suitable for use by any client implementation.
//!
//! ## Modules
//!
//! - [`jsonrpc`] - JSON-RPC 2.0 protocol types
//! - [`manifest`] - Client capability manifest
//! - [`policy`] - Tool execution routing policy
//! - [`events`] - Streaming event types
//! - [`thinking`] - Reasoning and confidence types

pub mod events;
pub mod jsonrpc;
pub mod manifest;
pub mod policy;
pub mod thinking;

// Re-export commonly used types at crate root
pub use events::{
    EnhancedRunSummary, RunSummary, StreamEvent, ToolErrorItem, ToolResult, ToolSummaryItem,
    UncertaintyAction,
};
pub use jsonrpc::{JsonRpcError, JsonRpcRequest, JsonRpcResponse, ToolCallContext, ToolCallParams, ToolCallResult};
pub use manifest::{ClientCapabilities, ClientEnvironment, ClientManifest, ExecutionConstraints};
pub use policy::ExecutionPolicy;
pub use thinking::{ConfidenceLevel, ReasoningStepType};
