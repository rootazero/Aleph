//! MemoryReflector — synthesise coherent answers from stored notes.
//!
//! See `docs/superpowers/specs/2026-04-13-memory-evolution-spec2-reflector-design.md`.

pub mod packet_adapter;
pub mod prompts;
pub mod types;

pub use types::{NoteRef, ReflectOpts, Synthesis};
