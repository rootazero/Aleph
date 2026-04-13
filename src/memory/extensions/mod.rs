//! MemoryExtension — pluggable memory enhancements for first-party
//! and third-party (MCP) extensions.
//!
//! See `docs/superpowers/specs/2026-04-13-memory-evolution-spec4-extensions-design.md`.

pub mod traits;
pub mod types;

pub use traits::MemoryExtension;
pub use types::{CaptureCtx, CaptureDecision, ProduceCtx, RetrieveCtx};
