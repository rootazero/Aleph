/// Context capture data structures for memory anchors
mod compression;
mod enums;
mod fact;
mod paths;

#[cfg(test)]
mod tests;

// Re-export all public items so external code can use `crate::memory::context::*`
pub use compression::CompressionResult;
pub use enums::{
    CognitiveLayer, FactSource, FactSpecificity, MemoryCategory, MemoryLayer, NoteType,
    TemporalScope,
};
pub use fact::MemoryFact;
pub use paths::compute_parent_path;
