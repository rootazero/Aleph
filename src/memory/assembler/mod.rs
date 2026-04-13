//! Working Memory Assembler — produces a portable [`MemoryEnvelope`] before
//! each LLM call. See `docs/superpowers/specs/2026-04-13-memory-evolution-spec1-assembler-design.md`.

pub mod envelope;

pub use envelope::{
    EnvelopeItem, EnvelopeMeta, EnvelopeSlot, ItemSource, MemoryEnvelope, SlotKind, SCHEMA_VERSION,
};
