//! MemoryExtension — pluggable memory enhancements for first-party
//! and third-party (MCP) extensions.
//!
//! See `docs/superpowers/specs/2026-04-13-memory-evolution-spec4-extensions-design.md`.

pub mod first_party;
pub mod insert_helper;
pub mod manifest;
pub mod mcp_adapter;
pub mod registry;
pub mod scheduler;
pub mod traits;
pub mod types;

pub use first_party::EnvelopeRelevanceFloorExtension;
pub use insert_helper::insert_with_capture_filter;
pub use manifest::{MemoryHook, MemoryManifestSection};
pub use mcp_adapter::{ManagerBackedMcpCaller, McpCaller, McpMemoryExtension, UnboundMcpCaller};
pub use registry::{
    MemoryExtensionRegistry, ON_CAPTURE_TIMEOUT, ON_DELEGATION_TIMEOUT, ON_PRE_COMPRESS_TIMEOUT,
    ON_RETRIEVE_TIMEOUT, ON_SESSION_SWITCH_TIMEOUT, PRODUCE_TIMEOUT,
};
pub use scheduler::MemoryProducerScheduler;
pub use traits::MemoryExtension;
pub use types::{
    CaptureCtx, CaptureDecision, DelegationCtx, PreCompressCtx, ProduceCtx, RetrieveCtx,
    SessionSwitchCtx, SessionSwitchReason,
};
