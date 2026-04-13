//! MemoryExtension — pluggable memory enhancements for first-party
//! and third-party (MCP) extensions.
//!
//! See `docs/superpowers/specs/2026-04-13-memory-evolution-spec4-extensions-design.md`.

pub mod first_party;
pub mod insert_helper;
pub mod registry;
pub mod traits;
pub mod types;

pub use first_party::EnvelopeRelevanceFloorExtension;
pub use insert_helper::insert_with_capture_filter;
pub use registry::{
    MemoryExtensionRegistry, ON_CAPTURE_TIMEOUT, ON_RETRIEVE_TIMEOUT, PRODUCE_TIMEOUT,
};
pub use traits::MemoryExtension;
pub use types::{CaptureCtx, CaptureDecision, ProduceCtx, RetrieveCtx};
